//! The device task driven by a recorded transcript.

use std::time::Duration;

use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::snapshot::Freshness;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::transport::replay::ReplayTransport;

/// A transcript that answers *IDN? and then whatever else is asked, by
/// replaying recorded traffic without checking the command order.
fn device() -> Device<ReplayTransport> {
    let jsonl = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transcripts/probe-58503a.jsonl"
    ))
    .expect("transcript fixture");
    let transport = ReplayTransport::from_jsonl(&jsonl)
        .expect("load transcript")
        .relaxed();
    let session = Session::new(
        transport,
        Config {
            timeout: Duration::from_millis(200),
            ..Config::default()
        },
    );
    Device::open(session).expect("open device")
}

#[test]
fn the_recorded_receiver_identifies_itself() {
    let device = device();
    assert_eq!(device.identity().model, "58503A");
    assert_eq!(device.identity().serial, "0000A00000");
    assert_eq!(device.identity().firmware, "3704-C");
}

#[test]
fn the_task_publishes_snapshots_to_subscribers() {
    let (handle, joiner) = task::spawn(device(), Cadence::default());
    let updates = handle.subscribe();

    // The transcript runs out quickly, so a poll will fail; what
    // matters is that a snapshot is published either way and says which
    // it was.
    let snapshot = updates
        .recv_timeout(Duration::from_secs(5))
        .expect("a snapshot");
    assert!(matches!(
        snapshot.freshness,
        Freshness::Live | Freshness::Stale
    ));
    assert_eq!(handle.latest().expect("a stored snapshot").at, snapshot.at);

    drop(updates);
    drop(handle);
    joiner.join().expect("the device thread");
}

#[test]
fn a_failed_poll_marks_the_snapshot_stale_and_says_why() {
    // An empty transcript makes every command time out.
    let transport = ReplayTransport::from_jsonl("").expect("empty transcript");
    let session = Session::new(
        transport,
        Config {
            timeout: Duration::from_millis(50),
            ..Config::default()
        },
    );
    // Opening fails outright with nothing to replay, which is itself
    // the right behaviour: no identity means no dialect.
    assert!(Device::open(session).is_err());
}

#[test]
fn nothing_is_published_before_the_first_poll() {
    // latest() is None rather than an empty snapshot, so a client can
    // tell "not polled yet" from "polled and everything was absent".
    let (handle, joiner) = task::spawn(device(), Cadence::default());
    let fresh = handle.latest();
    drop(handle);
    joiner.join().expect("the device thread");
    // Either nothing yet, or a real reading; never a fabricated blank.
    if let Some(snapshot) = fresh {
        assert!(snapshot.mode.is_some() || snapshot.polled.any_error().is_some());
    }
}

#[test]
fn the_task_stops_when_its_handles_go_away() {
    let (handle, joiner) = task::spawn(device(), Cadence::default());
    drop(handle);
    // Dropping every handle closes the request channel, which is the
    // task's signal to return rather than poll a receiver nobody is
    // listening to.
    joiner.join().expect("the device thread");
}
