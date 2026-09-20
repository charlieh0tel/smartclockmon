//! Session framing driven by recorded transcripts, so the awkward parts
//! of the wire protocol are covered without hardware.

use std::time::Duration;

use smartclock::error::Error;
use smartclock::session::Config;
use smartclock::session::Prompt;
use smartclock::session::Session;
use smartclock::transport::replay::ReplayTransport;

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

fn session(steps: &[(&str, &str)]) -> Session<ReplayTransport> {
    session_with(steps, Config::default())
}

fn session_with(steps: &[(&str, &str)], config: Config) -> Session<ReplayTransport> {
    let jsonl = transcript(steps);
    let transport = ReplayTransport::from_jsonl(&jsonl).expect("load transcript");
    Session::new(transport, config)
}

#[test]
fn a_query_returns_its_reply_without_echo_or_prompt() {
    let mut s = session(&[
        ("tx", ":SYNChronization:TINTerval?\r\n"),
        ("rx", ":SYNChronization:TINTerval?\r\n+7.2E-9\r\nscpi> "),
    ]);
    let reply = s.query(":SYNChronization:TINTerval?").expect("query");
    assert_eq!(reply.lines, vec!["+7.2E-9"]);
    assert_eq!(reply.prompt, Prompt::Ready);
    assert_eq!(reply.one_line("real").expect("one line"), "+7.2E-9");
}

#[test]
fn a_multi_line_reply_survives_framing() {
    // The status screen is the reason framing reads to a prompt rather
    // than to a line terminator.
    let screen = "---- Receiver Status ----\r\nSYNCHRONIZATION\r\nACQUISITION\r\n";
    let mut s = session(&[
        ("tx", ":SYSTem:STATus?\r\n"),
        ("rx", &format!(":SYSTem:STATus?\r\n{screen}scpi> ")),
    ]);
    let reply = s.query(":SYSTem:STATus?").expect("query");
    assert_eq!(reply.lines.len(), 3);
    assert_eq!(reply.lines[0], "---- Receiver Status ----");
}

#[test]
fn a_reply_split_across_reads_is_reassembled() {
    // At 19200 a long reply arrives in pieces; a prompt must not be
    // detected until all of it has landed.
    let mut s = session(&[
        ("tx", ":SYSTem:STATus:LENGth?\r\n"),
        ("rx", ":SYSTem:STATus:LEN"),
        ("rx", "Gth?\r\n+24"),
        ("rx", "\r\nsc"),
        ("rx", "pi> "),
    ]);
    let reply = s.query(":SYSTem:STATus:LENGth?").expect("query");
    assert_eq!(reply.lines, vec!["+24"]);
}

#[test]
fn an_error_prompt_becomes_a_device_error_from_the_queue() {
    let mut s = session(&[
        ("tx", ":BOGUS?\r\n"),
        ("rx", ":BOGUS?\r\nE-113> "),
        ("tx", ":SYSTem:ERRor?\r\n"),
        (
            "rx",
            ":SYSTem:ERRor?\r\n-113,\"Undefined header\"\r\nscpi> ",
        ),
    ]);
    match s.query(":BOGUS?") {
        Err(Error::Device { code, message }) => {
            assert_eq!(code, -113);
            assert_eq!(message, "Undefined header");
        }
        other => panic!("expected a device error, got {other:?}"),
    }
}

#[test]
fn an_error_prompt_with_an_empty_queue_is_reported_as_unexplained() {
    // The two have drifted out of step; saying so beats inventing a code.
    let mut s = session(&[
        ("tx", ":BOGUS?\r\n"),
        ("rx", ":BOGUS?\r\nE-113> "),
        ("tx", ":SYSTem:ERRor?\r\n"),
        ("rx", ":SYSTem:ERRor?\r\n0,\"No error\"\r\nscpi> "),
    ]);
    assert!(matches!(
        s.query(":BOGUS?"),
        Err(Error::UnexplainedError { .. })
    ));
}

#[test]
fn a_command_with_no_reply_yields_no_lines() {
    let mut s = session(&[
        ("tx", ":SYNChronization:HOLDover:INITiate\r\n"),
        ("rx", ":SYNChronization:HOLDover:INITiate\r\nscpi> "),
    ]);
    let reply = s
        .query(":SYNChronization:HOLDover:INITiate")
        .expect("query");
    assert!(reply.lines.is_empty());
}

#[test]
fn silence_times_out_and_reports_what_did_arrive() {
    // A short timeout: this test is about the deadline firing, and the
    // default five seconds would just make the suite slow.
    let config = Config {
        timeout: Duration::from_millis(50),
        ..Config::default()
    };
    let mut s = session_with(
        &[
            ("tx", ":GPS:POSition?\r\n"),
            ("rx", ":GPS:POSition?\r\npart"),
        ],
        config,
    );
    match s.query(":GPS:POSition?") {
        Err(Error::Timeout { seen, .. }) => assert!(seen.contains("part"), "seen was {seen:?}"),
        other => panic!("expected a timeout, got {other:?}"),
    }
}

#[test]
fn sync_discards_whatever_preceded_the_prompt() {
    // Connecting mid-reply from a previous user must not poison the
    // first real command.
    let mut s = session(&[
        ("tx", "\r\n"),
        ("rx", "leftovers from someone else\r\nscpi> "),
    ]);
    assert_eq!(s.sync().expect("sync"), Prompt::Ready);
}

#[test]
fn replay_rejects_a_command_the_transcript_does_not_contain() {
    let mut s = session(&[
        ("tx", ":GPS:POSition?\r\n"),
        ("rx", ":GPS:POSition?\r\nscpi> "),
    ]);
    assert!(s.query(":SOMETHING:ELSE?").is_err());
}
