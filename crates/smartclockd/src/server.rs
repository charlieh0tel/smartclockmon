//! The local socket server.
//!
//! Authorization is socket permissions and nothing else: systemd's
//! `RuntimeDirectory` and its mode decide who can open the socket, and
//! anyone who can may issue whatever the configuration allows.  On a
//! single-operator machine that is the whole of it -- no peer
//! credentials, no tokens.  The cost is that the daemon cannot tell two
//! clients apart, so the audit trail records what was done but not by
//! whom.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use anyhow::Context as _;
use anyhow::Result;
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::Stream;
use interprocess::local_socket::prelude::*;
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::task::Handle;

use crate::audit::Audit;
use crate::proto::Message;
use crate::proto::Op;
use crate::proto::Request;
use crate::proto::VERSION;

/// What a client is permitted to do.
///
/// Set once from the command line and never from the socket.  A daemon
/// started without a flag cannot be talked into the commands it gates,
/// which is the point: enabling one is a deliberate act outside the
/// client, and it survives any amount of fat-fingering at the socket.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Policy {
    /// Allow commands the table marks `control`.
    pub(crate) control: bool,
    /// Allow commands the table marks `dangerous`.
    pub(crate) dangerous: bool,
    /// Allow raw SCPI that the table does not recognise at all.
    pub(crate) raw: bool,
}

impl Policy {
    /// Whether a class of command is permitted.
    fn allows(self, class: Class) -> bool {
        match class {
            Class::Query => true,
            Class::Control => self.control,
            Class::Dangerous => self.dangerous,
        }
    }

    /// The flag that would permit a class.
    fn flag(class: Class) -> &'static str {
        match class {
            Class::Query => "",
            Class::Control => "--allow-control",
            Class::Dangerous => "--allow-dangerous",
        }
    }
}

/// What the daemon tells a client about itself.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    /// The receiver's identity string.
    pub identity: String,
    /// Which command tree is in use.
    pub dialect: Dialect,
    /// Where the snapshot log lives, so a client can open it read-only.
    pub database: String,
    /// What clients may do.
    pub policy: Policy,
    /// Where to record commands that were not scheduled polls.
    pub audit: Audit,
}

/// Listen for clients until the process ends.
pub(crate) fn serve(
    name: interprocess::local_socket::Name<'static>,
    handle: Handle,
    info: Info,
) -> Result<()> {
    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .context("binding the local socket")?;

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            // One client failing to connect is not a reason to stop
            // serving the others.
            Err(e) => {
                eprintln!("smartclockd: rejected a connection: {e}");
                continue;
            }
        };
        let handle = handle.clone();
        let info = info.clone();
        thread::Builder::new()
            .name("smartclockd-client".to_owned())
            .spawn(move || {
                if let Err(e) = talk(stream, &handle, &info) {
                    eprintln!("smartclockd: client ended: {e}");
                }
            })
            .context("spawning a client thread")?;
    }
    Ok(())
}

/// Serve one client until it goes away.
///
/// Snapshots are pushed from their own thread, so a client slow to read
/// cannot hold up its own requests.  Both threads write to the same
/// half behind a mutex; the messages are single short lines, so holding
/// it is brief and it keeps them from interleaving mid-line.
fn talk(stream: Stream, handle: &Handle, info: &Info) -> Result<()> {
    let (recv, send_half) = stream.split();
    let writer = Arc::new(Mutex::new(send_half));

    // A client gets the current state immediately, so it can render
    // before the next poll rather than showing an empty screen.
    if let Some(snapshot) = handle.latest() {
        write_line(&writer, &Message::event(snapshot))?;
    }

    let updates = handle.subscribe();
    let pusher = Arc::clone(&writer);
    thread::Builder::new()
        .name("smartclockd-push".to_owned())
        .spawn(move || {
            for snapshot in updates {
                if write_line(&pusher, &Message::event(snapshot)).is_err() {
                    return;
                }
            }
        })
        .context("spawning the push thread")?;

    for line in BufReader::new(recv).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(request, handle, info),
            Err(e) => Message::err(String::new(), format!("malformed request: {e}")),
        };
        write_line(&writer, &reply)?;
    }
    Ok(())
}

fn handle_request(request: Request, handle: &Handle, info: &Info) -> Message {
    if request.v != VERSION {
        return Message::err(
            request.id,
            format!("this daemon speaks protocol {VERSION}, not {}", request.v),
        );
    }
    let id = request.id;
    match request.op {
        Op::Latest => match handle.latest() {
            Some(snapshot) => match serde_json::to_value(snapshot) {
                Ok(value) => Message::ok(id, value),
                Err(e) => Message::err(id, e),
            },
            None => Message::err(id, "nothing has been polled yet"),
        },
        Op::Info => Message::ok(
            id,
            serde_json::json!({
                "identity": info.identity,
                "dialect": format!("{:?}", info.dialect),
                "database": info.database,
                "allow_control": info.policy.control,
                "allow_dangerous": info.policy.dangerous,
                "allow_raw": info.policy.raw,
            }),
        ),
        Op::Query { scpi } => send(id, &scpi, handle, info),
    }
}

/// Run a client's command, if the policy permits it.
///
/// The gate is the command table, not a list kept here, so a command
/// reclassified there is reclassified for clients too.  A spelling the
/// table does not know is refused unless raw is allowed, since an
/// unknown command could be anything.
fn send(id: String, scpi: &str, handle: &Handle, info: &Info) -> Message {
    let scpi = scpi.trim();
    let known = info
        .dialect
        .specs()
        .iter()
        .find(|s| s.scpi.eq_ignore_ascii_case(scpi));

    let (class, to_send) = match known {
        Some(spec) => (spec.class, spec.scpi),
        None if info.policy.raw => (raw_class(scpi), scpi),
        None => {
            return Message::err(
                id,
                format!("{scpi} is not a command this dialect knows, and --allow-raw is off"),
            );
        }
    };

    if !info.policy.allows(class) {
        return Message::err(
            id,
            format!(
                "{scpi} is {class:?}; this daemon was started without {}",
                Policy::flag(class)
            ),
        );
    }

    let outcome = handle.request(to_send);
    let note = match &outcome {
        Ok(reply) if reply.lines.is_empty() => "ok".to_owned(),
        Ok(reply) => format!("ok: {}", reply.lines.join(" | ")),
        Err(e) => format!("failed: {e}"),
    };
    info.audit.record(to_send, class, &note);

    // A command that changed something should not wait up to a minute
    // to show in the snapshots.
    if class != Class::Query && outcome.is_ok() {
        handle.refresh();
    }

    match outcome {
        Ok(reply) => Message::ok(id, serde_json::json!({ "lines": reply.lines })),
        Err(e) => Message::err(id, e),
    }
}

/// Guess a class for a command the table does not know.
///
/// Raw passthrough is off by default and this only applies when it is
/// on, but even then an unrecognised command is treated as at least
/// control, and anything resembling the ones that can strand the link
/// as dangerous.  Guessing generously costs a client one more flag;
/// guessing kindly could erase the receiver.
fn raw_class(scpi: &str) -> Class {
    let upper = scpi.to_ascii_uppercase();
    const STRANDS: [&str; 4] = ["COMM", "PRES", "ERAS", "LANG"];
    if STRANDS.iter().any(|needle| upper.contains(needle)) {
        Class::Dangerous
    } else if scpi.ends_with('?') {
        Class::Query
    } else {
        Class::Control
    }
}

fn write_line<W: Write>(writer: &Arc<Mutex<W>>, message: &Message) -> Result<()> {
    let line = serde_json::to_string(message)?;
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned writer"))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
