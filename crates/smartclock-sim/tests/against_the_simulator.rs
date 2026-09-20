//! The layers a recorded transcript cannot reach.
//!
//! Everything below `Session` can be replayed from a capture, but the
//! device and the task poll on their own schedule and in their own
//! order, so a transcript never lines up.  These run against a
//! simulator that answers whatever it is asked.

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
    assert!(snapshot.polled.fast.is_some());
    assert!(snapshot.polled.medium.is_some());
    assert!(snapshot.polled.slow.is_some());

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
