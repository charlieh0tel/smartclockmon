//! The layers a recorded transcript cannot reach.
//!
//! Everything below `Session` can be replayed from a capture, but the
//! device and the task poll on their own schedule and in their own
//! order, so a transcript never lines up.  These run against a
//! simulator that answers whatever it is asked.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::snapshot::Tier;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::types::SmartClockMode;
use smartclock_sim::receiver::MAX_ERRORS;
use smartclock_sim::receiver::Receiver;
use smartclock_sim::transport::SimTransport;

fn device(receiver: Receiver) -> Device<SimTransport> {
    let session = Session::new(
        SimTransport::new(receiver),
        Config {
            timeout: Duration::from_millis(500),
            ..Config::default()
        },
    );
    Device::open(session).expect("open the simulated receiver")
}

#[test]
fn the_session_frames_against_a_receiver_that_echoes_and_prompts() {
    // The whole point: echo, the "scpi > " prompt with its space before
    // the bracket, and a reply read to the prompt rather than to a line
    // terminator.
    let device = device(Receiver::default());
    assert_eq!(device.identity().model, "58503A");
    assert_eq!(device.identity().serial, "3710A01056");
}

#[test]
fn a_bad_command_becomes_a_typed_error_from_the_queue() {
    let mut device = device(Receiver::default());
    match device.session().query(":NO:SUCH:COMMAND?") {
        Err(smartclock::error::Error::Device { code, message }) => {
            assert_eq!(code, -113);
            assert_eq!(message, "Undefined header");
        }
        other => panic!("expected a device error, got {other:?}"),
    }
    // And the session recovers, which is the part that matters: an
    // error prompt must not desynchronise everything after it.
    assert_eq!(device.tfom().expect("tfom").get(), 3);
}

#[test]
fn every_tier_polls_into_a_snapshot() {
    let mut device = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    for tier in Tier::ALL {
        device
            .poll(tier, &mut snapshot, jiff::Timestamp::now())
            .unwrap_or_else(|e| panic!("polling {tier:?}: {e}"));
    }
    assert_eq!(snapshot.freshness, Freshness::Live);
    assert_eq!(snapshot.mode, Some(SmartClockMode::Locked));
    assert!(snapshot.efc.is_some());
    assert!(snapshot.temperature.is_some());
    assert!(snapshot.efc_dac.is_some());
    // The satellite table exists only on the status screen, so finding
    // one proves the medium tier scraped it.
    let screen = snapshot.screen.as_ref().expect("a status screen");
    assert_eq!(screen.satellites.len(), 9);
    assert_eq!(screen.tracking, Some(6));
    assert!(snapshot.date.is_some());
}

#[test]
fn a_value_the_receiver_declines_reads_as_absent_not_as_a_failure() {
    // Present holdover error does not exist while locked; the receiver
    // answers -230, and that must not fail the whole tier.
    let mut device = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    device
        .poll(Tier::Medium, &mut snapshot, jiff::Timestamp::now())
        .expect("the medium tier");
    assert_eq!(snapshot.holdover_present, None);
    assert!(snapshot.holdover_predicted.is_some());
}

#[test]
fn a_faulty_receiver_reports_its_faults() {
    let mut device = device(Receiver::faulty());
    let condition = device.hardware_condition().expect("hardware");
    assert!(!condition.is_healthy());
    let faults: Vec<&str> = condition.faults().map(|f| f.describe()).collect();
    assert_eq!(faults, vec!["EFC near full scale"]);
    assert_eq!(device.mode().expect("mode"), SmartClockMode::Holdover);
    // Near the rail, which is the state this project watches for and
    // which the development unit has never been in.
    assert!(device.efc().expect("efc").range_used() > 0.9);
}

#[test]
fn control_changes_what_the_receiver_then_reports() {
    let mut device = device(Receiver::default());
    assert_eq!(device.mode().expect("mode"), SmartClockMode::Locked);

    device.control().initiate_holdover().expect("initiate");
    assert_eq!(device.mode().expect("mode"), SmartClockMode::Holdover);
    // What was absent while locked is present in holdover.
    assert!(device.holdover_present().expect("present").is_some());

    device.control().recover_from_holdover().expect("recover");
    assert_eq!(device.mode().expect("mode"), SmartClockMode::Locked);
}

#[test]
fn the_task_polls_and_publishes_without_hardware() {
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            fast: Duration::from_millis(20),
            medium: Duration::from_millis(50),
            slow: Duration::from_millis(80),
        },
    );
    let updates = handle.subscribe();

    let mut complete = None;
    for _ in 0..200 {
        let Ok(snapshot) = updates.recv_timeout(Duration::from_secs(2)) else {
            break;
        };
        if snapshot.screen.is_some() && snapshot.date.is_some() {
            complete = Some(snapshot);
            break;
        }
    }
    let snapshot = complete.expect("a snapshot from every tier");
    assert_eq!(snapshot.freshness, Freshness::Live);
    assert!(snapshot.polled.fast.at.is_some());
    assert!(snapshot.polled.medium.at.is_some());
    assert!(snapshot.polled.slow.at.is_some());

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_command_submitted_to_the_task_is_served_between_polls() {
    let (handle, joiner) = task::spawn(device(Receiver::default()), Cadence::default());
    let reply = handle
        .request(":SYNChronization:TFOMerit?")
        .expect("the command");
    assert_eq!(reply.lines, vec!["+3"]);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn the_values_move_between_polls() {
    // A simulator returning constants would hide a flat trend, a chart
    // that cannot scale itself, and an average indistinguishable from a
    // single sample.
    let mut device = device(Receiver::default());
    let mut seen = std::collections::HashSet::new();
    for _ in 0..12 {
        let ns = device
            .time_interval()
            .expect("interval")
            .map(|v| v.as_nanos() as i64);
        seen.insert(ns);
    }
    assert!(seen.len() > 1, "the interval never changed: {seen:?}");
}

#[test]
fn a_working_tier_does_not_relabel_a_failing_one_as_current() {
    // The bug this replaces: one timestamp and one freshness flag for
    // the whole snapshot meant the one-second tier, succeeding, kept
    // re-stamping a status screen that had not been read in minutes.
    let mut device = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());

    let early = jiff::Timestamp::now();
    device
        .poll(Tier::Medium, &mut snapshot, early)
        .expect("the medium tier");
    assert_eq!(snapshot.polled.medium.at, Some(early));

    // A later fast poll must not move the medium tier's clock.
    std::thread::sleep(Duration::from_millis(20));
    let later = jiff::Timestamp::now();
    device
        .poll(Tier::Fast, &mut snapshot, later)
        .expect("the fast tier");
    assert_eq!(snapshot.polled.fast.at, Some(later));
    assert_eq!(
        snapshot.polled.medium.at,
        Some(early),
        "a fast poll relabelled the medium tier's fields as current"
    );
    let age = snapshot
        .polled
        .age(Tier::Medium, later)
        .expect("an age for the medium tier");
    assert!(age > 0.0, "the medium tier reported itself as just read");
}

#[test]
fn a_tier_that_works_does_not_clear_another_tiers_error() {
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    snapshot.polled.failed(Tier::Medium, "the screen timed out");

    let mut device = device(Receiver::default());
    device
        .poll(Tier::Fast, &mut snapshot, jiff::Timestamp::now())
        .expect("the fast tier");

    assert!(snapshot.polled.fast.error.is_none());
    assert_eq!(
        snapshot.polled.medium.error.as_deref(),
        Some("the screen timed out"),
        "a successful fast poll hid the medium tier's failure"
    );
    assert!(snapshot.polled.any_error().is_some());
}

#[test]
fn a_failed_command_does_not_misattribute_the_next_polls_answer() {
    // The failure this guards: a client command that errors leaves the
    // receiver's reply in flight, and the next poll reads it as its
    // own.  TFOM and FFOM take the same shape, so the wrong one is
    // recorded as a measurement rather than rejected.
    let (handle, joiner) = task::spawn(device(Receiver::default()), Cadence::default());

    assert!(
        handle.request(":NO:SUCH:COMMAND?").is_err(),
        "the simulator should refuse an undefined header"
    );

    // Whatever the failure left behind must not become the next answer.
    for _ in 0..4 {
        let tfom = handle
            .request(":SYNChronization:TFOMerit?")
            .expect("a reading after the failure");
        assert_eq!(tfom.lines, vec!["+3"], "a reply was read out of step");
        let ffom = handle
            .request(":SYNChronization:FFOMerit?")
            .expect("a reading after the failure");
        assert_eq!(ffom.lines, vec!["+1"], "a reply was read out of step");
    }

    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_subscriber_that_stops_reading_is_dropped_not_indulged() {
    // An unbounded channel only fails once the receiver is dropped, so
    // a client that merely stopped reading queued a full snapshot per
    // second in daemon memory for as long as it held the connection.
    // Falling behind should cost the subscriber its subscription.
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            fast: Duration::from_millis(5),
            medium: Duration::from_secs(60),
            slow: Duration::from_secs(60),
        },
    );
    let stalled = handle.subscribe();
    let reading = handle.subscribe();

    // Let the stalled subscriber overflow while the other keeps up.
    let mut seen = 0;
    for _ in 0..60 {
        if reading.recv_timeout(Duration::from_secs(2)).is_ok() {
            seen += 1;
        }
    }
    assert!(seen > 20, "the attentive subscriber only saw {seen}");

    // The stalled one holds at most its backlog, not one per poll.
    let queued = stalled.try_iter().count();
    assert!(
        queued <= 16,
        "a subscriber that never read accumulated {queued} snapshots"
    );

    drop(reading);
    drop(stalled);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_tier_as_slow_as_its_period_does_not_starve_client_commands() {
    // Polls used to take absolute priority, so a tier always overdue
    // meant the request queue was never reached: every caller blocked
    // forever and the queue grew without bound.
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            // Far faster than the polls can actually complete.
            fast: Duration::from_nanos(1),
            medium: Duration::from_nanos(1),
            slow: Duration::from_nanos(1),
        },
    );
    for _ in 0..3 {
        let reply = handle
            .request(":SYNChronization:TFOMerit?")
            .expect("a command served between polls");
        assert_eq!(reply.lines, vec!["+3"]);
    }
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_command_waits_a_bounded_time_for_its_answer() {
    // Nothing drains the queue while the link is down, so waiting
    // forever meant a caller blocked until the receiver came back --
    // possibly hours -- with its thread and its socket held open.
    let (handle, joiner) = task::spawn(device(Receiver::default()), Cadence::default());
    let reply = handle
        .request_within(":SYNChronization:TFOMerit?", Duration::from_secs(5))
        .expect("a prompt answer");
    assert_eq!(reply.lines, vec!["+3"]);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_flood_of_bad_commands_does_not_grow_the_error_queue() {
    // Driven at the receiver rather than through a Session, because a
    // Session reads :SYSTem:ERRor? after every refusal to build its
    // typed error, and so never lets the queue fill.  A client that
    // sends bad commands and never asks why is the case that can grow
    // it, and the one a bound has to survive.
    let flood = 200;
    let mut receiver = Receiver::default();
    for _ in 0..flood {
        let answer = receiver.respond(":NO:SUCH:COMMAND?");
        assert!(!answer.accepted, "a bad command must be refused");
    }

    // Drain it, counting.  A queue that grew by one per rejection would
    // hand back a few hundred of these.
    let mut drained = Vec::new();
    loop {
        let answer = receiver.respond(":SYSTem:ERRor?");
        let line = answer.lines.first().expect("one line per error").clone();
        if line.starts_with("+0,") {
            break;
        }
        assert!(
            drained.len() <= flood,
            "the queue is not draining: {} entries and counting",
            drained.len()
        );
        drained.push(line);
    }

    assert_eq!(
        drained.len(),
        MAX_ERRORS,
        "the queue should stop at its bound, not grow with the flood"
    );
    // The earliest error survives and the last slot carries the
    // overflow marker, as 488.2 asks.
    assert!(
        drained[0].starts_with("-113,"),
        "expected the first rejection to be kept, got {}",
        drained[0]
    );
    assert!(
        drained[MAX_ERRORS - 1].starts_with("-350,"),
        "expected -350 in the last slot, got {}",
        drained[MAX_ERRORS - 1]
    );

    // Still answering afterwards.
    assert_eq!(
        receiver.respond(":SYNChronization:TFOMerit?").lines,
        vec!["+3"]
    );
}

#[test]
fn client_commands_cannot_starve_the_poll_schedule() {
    // The fix for polls starving requests overshot into the reverse:
    // an unbounded drain meant a few clients each keeping one request
    // outstanding published no snapshots at all, logged nothing, and
    // left the last snapshot labelled Live.
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            fast: Duration::from_millis(10),
            medium: Duration::from_secs(3600),
            slow: Duration::from_secs(3600),
        },
    );
    let updates = handle.subscribe();

    let busy = Arc::new(AtomicBool::new(true));
    let clients: Vec<_> = (0..32)
        .map(|_| {
            let handle = handle.clone();
            let busy = Arc::clone(&busy);
            std::thread::spawn(move || {
                let mut served = 0;
                while busy.load(Ordering::Relaxed) {
                    if handle.request(":SYNChronization:TFOMerit?").is_ok() {
                        served += 1;
                    }
                }
                served
            })
        })
        .collect();

    // Read throughout -- a subscriber that stops reading for long
    // enough falls SUBSCRIBER_BACKLOG behind and is dropped, which
    // looks exactly like starvation from here -- but only count after
    // the clients are up, since the first poll happens before they
    // are and would otherwise carry the test on its own.
    let start = std::time::Instant::now();
    let counting_from = start + Duration::from_millis(500);
    let until = start + Duration::from_millis(2000);
    let mut snapshots = 0;
    while std::time::Instant::now() < until {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if updates.recv_timeout(left).is_ok() && std::time::Instant::now() >= counting_from {
            snapshots += 1;
        }
    }
    busy.store(false, Ordering::Relaxed);
    let served: usize = clients
        .into_iter()
        .map(|c| c.join().expect("a client"))
        .sum();

    // Both sides have to make progress.  Before the bound this was 0
    // snapshots; polls taking absolute priority made it 0 commands.
    assert!(
        snapshots > 10,
        "only {snapshots} snapshots in 1.5 s while clients were busy; the schedule is starved"
    );
    assert!(served > 0, "no client command served in two seconds");

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_command_whose_caller_gave_up_is_not_sent() {
    // A timeout used to mean only that the caller stopped listening:
    // the command sat in the queue and was run whenever the task got to
    // it, so a holdover the operator was told had failed began seconds
    // later, and the audit trail recorded it as a failure.
    let (handle, joiner) = task::spawn(device(Receiver::default()), Cadence::default());

    let gave_up = handle.request_within(
        ":SYNChronization:HOLDover:INITiate",
        Duration::from_nanos(1),
    );
    assert!(gave_up.is_err(), "a one-nanosecond wait should time out");

    // Give the task long enough to have run it, had it been going to.
    std::thread::sleep(Duration::from_millis(300));
    let reply = handle
        .request(":SYNChronization:STATe?")
        .expect("the receiver answers");
    assert_eq!(
        reply.lines,
        vec!["LOCK"],
        "the receiver left lock, so the abandoned command was sent after all"
    );

    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_receiver_that_only_refuses_is_not_a_dead_link() {
    // Only a failure reopening could fix may count toward giving up on
    // the port.  Counting receiver refusals too meant one unexpected
    // reply put the daemon in a five-second reconnect loop against
    // healthy hardware, logging nothing.  FAILURES_BEFORE_RECONNECT is
    // 3, so twenty polls is well past it.
    let (handle, joiner) = task::spawn(
        device(Receiver::refusing()),
        Cadence {
            fast: Duration::from_millis(5),
            medium: Duration::from_secs(3600),
            slow: Duration::from_secs(3600),
        },
    );
    let updates = handle.subscribe();

    let mut seen = 0;
    for _ in 0..20 {
        match updates.recv_timeout(Duration::from_secs(2)) {
            Ok(snapshot) => {
                assert_eq!(snapshot.freshness, Freshness::Stale);
                assert!(
                    snapshot.polled.fast.error.is_some(),
                    "the tier that failed should say so"
                );
                seen += 1;
            }
            Err(_) => break,
        }
    }

    // Still polling, still reporting.  Count a refusal as a link
    // failure and the task gives up after three, the thread ends, and
    // both of these fail.
    assert_eq!(seen, 20, "it stopped publishing after {seen} snapshots");
    assert!(
        !joiner.is_finished(),
        "the task gave up on a link whose only fault was the receiver saying no"
    );

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}
