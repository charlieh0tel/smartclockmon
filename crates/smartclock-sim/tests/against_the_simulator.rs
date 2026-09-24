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
    assert_eq!(device.identity().serial, "0000A00000");
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
    assert!(snapshot.tracking.is_some());
    assert!(snapshot.date.is_some());
    // No tier reads the status screen, so the satellite table it alone
    // carries is absent until something asks for one.
    assert!(snapshot.screen.is_none());
    let screen = device.screen().expect("a screen on request");
    assert_eq!(screen.satellites.len(), 9);
    assert_eq!(screen.tracking, Some(6));
}

#[test]
fn the_satellite_counts_come_from_queries_and_agree_with_the_screen() {
    let now = jiff::Timestamp::now();
    let mut snapshot = Snapshot::new(now);
    let mut device = device(Receiver::default());
    device
        .poll(Tier::Medium, &mut snapshot, now)
        .expect("medium");
    let screen = device.screen().expect("a screen on request");
    assert_eq!(snapshot.tracking, screen.tracking);
    // The screen prints what is visible but untracked; the query
    // answers what is visible at all.
    assert_eq!(
        snapshot.visible.zip(snapshot.tracking).map(|(v, t)| v - t),
        screen.not_tracking
    );
}

#[test]
fn a_pass_taken_a_step_at_a_time_reads_what_the_whole_pass_reads() {
    let now = jiff::Timestamp::now();
    let mut whole = Snapshot::new(now);
    device(Receiver::default())
        .poll(Tier::Medium, &mut whole, now)
        .expect("the medium tier in one pass");

    let mut stepped = Snapshot::new(now);
    let mut device = device(Receiver::default());
    for step in 0..smartclock::device::step_count(Tier::Medium) {
        device
            .poll_step(Tier::Medium, step, &mut stepped, now)
            .expect("a medium step");
        // The pass is only as fresh as its slowest field, so it is
        // stamped once, at the end.
        let done = step + 1 == smartclock::device::step_count(Tier::Medium);
        assert_eq!(stepped.polled.medium.at.is_some(), done, "at step {step}");
    }
    assert_eq!(stepped.temperature, whole.temperature);
    assert_eq!(stepped.efc_dac, whole.efc_dac);
    assert_eq!(stepped.operation, whole.operation);
    assert_eq!(stepped.holdover_predicted, whole.holdover_predicted);
    assert_eq!(stepped.screen, whole.screen);

    // A caller that has lost its place reads nothing rather than
    // wedging on an index the tier does not have.
    device
        .poll_step(
            Tier::Medium,
            smartclock::device::step_count(Tier::Medium),
            &mut stepped,
            now,
        )
        .expect("a step past the end");
}

#[test]
fn a_value_the_receiver_declines_reads_as_absent_not_as_a_failure() {
    // Present holdover error does not exist while locked; the receiver
    // answers -230, and that must not fail the whole tier.  It is not
    // asked for while locked either -- see below -- so this covers the
    // tolerance rather than the asking.
    let mut device = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    device
        .poll(Tier::Medium, &mut snapshot, jiff::Timestamp::now())
        .expect("the medium tier");
    assert_eq!(snapshot.holdover_present, None);
    assert!(snapshot.holdover_predicted.is_some());
}

#[test]
fn a_question_with_a_known_answer_is_not_asked() {
    // Present holdover error exists only in holdover.  Asking while
    // locked is answered with -230, and a refusal is not free: the
    // session reads the error queue to explain it, so a question whose
    // answer is already known cost two round trips of the tier that is
    // against its wire budget, on every medium poll, for ever.
    //
    // Checked by the error queue rather than by timing: a refusal
    // leaves an entry behind, so a queue still empty after a medium
    // poll is the evidence that nothing was refused.
    let mut locked = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    locked
        .poll(Tier::Medium, &mut snapshot, jiff::Timestamp::now())
        .expect("the medium tier");
    let queue = locked
        .session()
        .query(":SYSTem:ERRor?")
        .expect("read the error queue");
    assert_eq!(
        queue.lines,
        vec!["+0,\"No error\"".to_owned()],
        "a locked receiver should have been asked nothing it would refuse"
    );

    // In holdover it is a real question, and gets asked.
    let mut holding = device(Receiver::faulty());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    holding
        .poll(Tier::Medium, &mut snapshot, jiff::Timestamp::now())
        .expect("the medium tier");
    assert!(
        snapshot.holdover_present.is_some(),
        "in holdover the value exists and should be read"
    );
}

#[test]
fn the_powerup_register_is_read_on_the_slow_tier() {
    // Its three bits are set during startup and then stay, so it does
    // not belong on the tier that is short of wire time.
    let mut device = device(Receiver::default());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    device
        .poll(Tier::Medium, &mut snapshot, jiff::Timestamp::now())
        .expect("the medium tier");
    assert!(snapshot.powerup.is_none(), "not the medium tier's work");
    device
        .poll(Tier::Slow, &mut snapshot, jiff::Timestamp::now())
        .expect("the slow tier");
    assert!(snapshot.powerup.is_some());
}

#[test]
fn one_refusal_does_not_cost_the_rest_of_the_tier() {
    // A receiver that has never had a fix refuses its date with -230.
    // A bare `?` on that line meant the slow tier never completed once
    // on a cold Z3805A: the log count, the tempco and the powerup
    // register sit after the date, and all three reported themselves
    // missing when they had simply never been asked.
    let mut device = device(Receiver::cold());
    let mut snapshot = Snapshot::new(jiff::Timestamp::now());
    device
        .poll(Tier::Slow, &mut snapshot, jiff::Timestamp::now())
        .expect("the slow tier must finish");
    assert!(
        snapshot.log_count.is_some(),
        "the log count sits after the date and must still be read"
    );
    assert!(snapshot.powerup.is_some(), "so does the powerup register");
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
fn a_failed_poll_marks_the_snapshot_stale_and_says_why() {
    // TFOM comes back unreadable, so every fast step fails.  What is
    // published must say so: Stale, with the fast tier's error naming
    // the reply it could not read.
    let mut receiver = Receiver::default();
    receiver
        .garbled
        .push(":SYNChronization:TFOMerit?".to_owned());
    let (handle, joiner) = task::spawn(device(receiver), Cadence::default());
    let updates = handle.subscribe();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    let failed = loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let snapshot = updates
            .recv_timeout(left)
            .expect("a snapshot saying the fast tier failed");
        if snapshot.polled.fast.error.is_some() {
            break snapshot;
        }
    };
    assert_eq!(failed.freshness, Freshness::Stale);
    let why = failed.polled.fast.error.expect("an error");
    assert!(
        why.contains("#not a reading#"),
        "the reason does not say what failed: {why}"
    );

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_step_that_fails_partway_publishes_none_of_what_it_read() {
    // The fast step reads mode, TFOM, FFOM and the interval before it
    // reaches EFC.  With EFC unparsable every fast step fails there, so
    // no snapshot may carry a mode: one that did would be holding values
    // from a step that never completed, under a timestamp they were not
    // read at.
    let mut receiver = Receiver::default();
    receiver
        .garbled
        .push(":DIAGnostic:ROSCillator:EFControl:RELative?".to_owned());
    let (handle, joiner) = task::spawn(
        device(receiver),
        Cadence {
            fast: Duration::from_millis(20),
            medium: Duration::from_millis(50),
            slow: Duration::from_millis(80),
        },
    );
    let updates = handle.subscribe();
    let mut seen = 0;
    let until = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < until {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let Ok(snapshot) = updates.recv_timeout(left) else {
            break;
        };
        seen += 1;
        assert_eq!(
            snapshot.mode, None,
            "a failed fast step published a mode it read partway"
        );
        assert!(snapshot.polled.fast.at.is_none());
    }
    assert!(seen > 5, "only {seen} snapshots were published");

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_sky_read_is_returned_and_not_carried_forward() {
    // One sky read must reach the caller, reach the subscribers once so
    // the log records that sky, and then be gone.  Stored as the
    // latest, it was copied into every later snapshot -- and every one
    // of them logged the same satellites again as if newly observed.
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            fast: Duration::from_millis(20),
            medium: Duration::from_millis(50),
            slow: Duration::from_millis(80),
        },
    );
    let updates = handle.subscribe();
    let screen = handle.sky().expect("a screen on request");
    assert_eq!(screen.satellites.len(), 9);

    // Delivered once, then never again.
    let mut carrying = 0;
    let mut after = 0;
    let until = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < until && after < 20 {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let Ok(snapshot) = updates.recv_timeout(left) else {
            break;
        };
        if snapshot.screen.is_some() {
            carrying += 1;
        } else if carrying > 0 {
            after += 1;
        }
    }
    assert_eq!(carrying, 1, "the screen was delivered {carrying} times");
    assert!(after > 0, "no snapshots followed the sky read");
    assert!(
        handle.latest().expect("a snapshot").screen.is_none(),
        "the screen was kept as the latest"
    );

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
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
        // Every tier stamped, not merely one field from each: the
        // tiers finish their passes at different times, so a snapshot
        // carrying a screen can still predate the first medium pass.
        if Tier::ALL
            .iter()
            .all(|t| snapshot.polled.get(*t).at.is_some())
        {
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
fn a_published_snapshot_names_the_receiver_it_was_read_from() {
    let simulated = Receiver::default();
    let identity = simulated.identity.clone();
    let (handle, joiner) = task::spawn(device(simulated), Cadence::default());
    let snapshot = handle
        .subscribe()
        .recv_timeout(Duration::from_secs(5))
        .expect("a snapshot");
    assert_eq!(snapshot.receiver, Some(identity));
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

#[test]
fn a_steady_stream_of_refreshes_does_not_starve_the_slow_tier() {
    // Refresh sets every deadline to the same instant, so ties are the
    // normal case rather than the exception.  Breaking them by tier
    // order meant fast and medium took their turns and another refresh
    // arrived before slow ever got one -- and a control command
    // triggers a refresh, so a client issuing them steadily was enough
    // to stop position and date being read at all.
    let (handle, joiner) = task::spawn(
        device(Receiver::default()),
        Cadence {
            fast: Duration::from_millis(20),
            medium: Duration::from_millis(20),
            slow: Duration::from_millis(20),
        },
    );
    let updates = handle.subscribe();

    let refresher = handle.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = Arc::clone(&stop);
    let spammer = std::thread::spawn(move || {
        // No pause: the queue must never be empty, or a gap lets the
        // other tiers through and the starvation cannot show.
        while !stopper.load(Ordering::Relaxed) {
            refresher.refresh();
        }
    });

    // The slow tier carries position and date, so its timestamp moving
    // is the proof it ran.
    let mut slow_ran = false;
    let until = std::time::Instant::now() + Duration::from_secs(3);
    let mut first = None;
    while std::time::Instant::now() < until {
        let left = until.saturating_duration_since(std::time::Instant::now());
        let Ok(snapshot) = updates.recv_timeout(left) else {
            break;
        };
        match (first, snapshot.polled.slow.at) {
            (None, at) => first = Some(at),
            (Some(before), at) if at != before => {
                slow_ran = true;
                break;
            }
            _ => {}
        }
    }

    stop.store(true, Ordering::Relaxed);
    spammer.join().expect("the refresher");
    assert!(
        slow_ran,
        "the slow tier never ran while refreshes kept arriving"
    );

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

/// A command is explained by its own error, not by whatever was in the
/// queue before it.
///
/// The queue is first in, first out, so reading one entry returns the
/// oldest unread error.  With something already waiting, a routine
/// refusal used to report itself as that older error -- and consume it,
/// so the real one was never seen at all.  Both halves are checked
/// here: the right code comes back, and the older error is still
/// available rather than gone.
#[test]
fn a_command_whose_error_was_lost_to_a_full_queue_says_so() {
    // A full queue replaces its last entry with -350 and discards the
    // newest, so a command failing then leaves only the -350 behind.
    // That is the queue's state, not the command's error.
    let mut receiver = Receiver::default();
    for _ in 0..MAX_ERRORS + 8 {
        assert!(!receiver.respond(":NO:SUCH:COMMAND?").accepted);
    }
    let mut device = device(receiver);
    match device.session().query(":NO:SUCH:COMMAND?") {
        Err(smartclock::error::Error::ErrorLost { .. }) => {}
        other => panic!("expected the error to be reported lost, got {other:?}"),
    }
    let strays = device.session().take_stray_errors();
    assert!(
        strays.iter().any(|e| e.code == -350),
        "the overflow marker should be kept as a stray: {strays:?}"
    );
}

#[test]
fn an_error_is_attributed_to_the_command_that_caused_it() {
    let mut receiver = Receiver::default();
    // Something the receiver raised on its own, unread.
    receiver.queue_error(-313, "Calibration memory lost");
    let mut device = device(receiver);

    // Present holdover error does not exist while locked; the receiver
    // refuses with -230.  That refusal, not the -313, is what this
    // command means.
    let refused = device
        .session()
        .query(":SYNChronization:HOLDover:TUNCertainty:PRESent?")
        .expect_err("locked, so this is refused");
    assert!(
        format!("{refused}").contains("-230"),
        "explained by the wrong error: {refused}"
    );

    let strays = device.session().take_stray_errors();
    assert_eq!(strays.len(), 1, "the older error should be kept, not eaten");
    assert_eq!(strays[0].code, -313);
    assert!(
        device.session().take_stray_errors().is_empty(),
        "taking them should clear them"
    );
}
