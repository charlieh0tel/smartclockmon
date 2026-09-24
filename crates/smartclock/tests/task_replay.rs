//! The device task driven by a recorded transcript.

use std::time::Duration;

use smartclock::device::Device;
use smartclock::session::Config;
use smartclock::session::Reply;
use smartclock::session::Session;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::sync::mpsc::sync_channel;
use std::time::Instant;

use smartclock::snapshot::Freshness;
use smartclock::snapshot::Tier;
use smartclock::task;
use smartclock::task::Cadence;
use smartclock::task::DeviceTask;
use smartclock::task::Request;
use smartclock::task::Shared;
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
fn nothing_is_published_before_the_first_poll() {
    // latest() is None rather than an empty snapshot, so a client can
    // tell "not polled yet" from "polled and everything was absent".
    // Subscribed before the task starts, so nothing it publishes can be
    // missed: the first snapshot is the first thing it ever published,
    // and it must come from a poll.
    let shared = Shared::new();
    let updates = shared.subscribe();
    let (requests_tx, requests_rx) = channel();
    let mut task = DeviceTask::new(device(), Cadence::default(), shared.clone(), requests_rx);
    assert!(shared.latest().is_none());
    let runner = std::thread::spawn(move || {
        task.run();
    });

    let first = updates
        .recv_timeout(Duration::from_secs(5))
        .expect("a snapshot");
    let polled = Tier::ALL
        .iter()
        .any(|&tier| first.polled.get(tier).at.is_some() || first.polled.get(tier).error.is_some());
    assert!(
        polled,
        "the first snapshot published came from no poll: {first:?}"
    );

    drop(updates);
    drop(requests_tx);
    runner.join().expect("the device thread");
}

/// Build a transcript from (direction, bytes) pairs.
fn transcript(steps: &[(&str, &str)]) -> String {
    steps
        .iter()
        .enumerate()
        .map(|(i, (dir, data))| {
            let data = serde_json::to_string(data).expect("encode");
            format!(r#"{{"t":{i}.0,"dir":"{dir}","data":{data}}}"#)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Queue a command for a task that has not started yet.
fn queue(requests: &Sender<Request>, scpi: &str) -> Receiver<smartclock::error::Result<Reply>> {
    let (answer, reply) = sync_channel(1);
    requests
        .send(Request::Command {
            scpi: scpi.to_owned(),
            deadline: Instant::now() + Duration::from_secs(10),
            answer,
        })
        .expect("queue a command");
    reply
}

#[test]
fn a_failed_command_does_not_misattribute_the_next_answer() {
    // TFOM's prompt arrives late, after the session has given up on it,
    // and would be read as the end of whatever is asked next: FFOM
    // would come back as TFOM's +3.  The task has to resynchronise
    // after the failure, and the transcript is strict, so any other
    // order of writes fails the replay.
    let jsonl = transcript(&[
        ("tx", "\r\n"),
        ("rx", "\r\nscpi> "),
        ("tx", "*IDN?\r\n"),
        (
            "rx",
            "*IDN?\r\nHEWLETT-PACKARD,58503A,0000A00000,3704-C\r\nscpi> ",
        ),
        ("tx", ":SYNChronization:TFOMerit?\r\n"),
        ("rx", ":SYNChronization:TFOMerit?\r\n+3\r\n"),
        ("tx", "\r\n"),
        ("rx", "scpi> \r\nscpi> "),
        ("tx", ":SYNChronization:FFOMerit?\r\n"),
        ("rx", ":SYNChronization:FFOMerit?\r\n+1\r\nscpi> "),
    ]);
    let session = Session::new(
        ReplayTransport::from_jsonl(&jsonl).expect("load transcript"),
        Config {
            timeout: Duration::from_millis(200),
            ..Config::default()
        },
    );
    let device = Device::open(session).expect("open device");

    // Both queued before the task starts, so they are served in order
    // ahead of any poll.
    let (requests_tx, requests_rx) = channel();
    let tfom = queue(&requests_tx, ":SYNChronization:TFOMerit?");
    let ffom = queue(&requests_tx, ":SYNChronization:FFOMerit?");
    let mut task = DeviceTask::new(device, Cadence::default(), Shared::new(), requests_rx);
    std::thread::spawn(move || {
        task.run();
    });

    let wait = Duration::from_secs(5);
    assert!(tfom.recv_timeout(wait).expect("an answer").is_err());
    let ffom = ffom
        .recv_timeout(wait)
        .expect("an answer")
        .expect("FFOM answered");
    assert_eq!(ffom.lines, vec!["+1"]);
    drop(requests_tx);
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
