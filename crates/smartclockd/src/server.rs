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

use crate::proto::Message;
use crate::proto::Op;
use crate::proto::Request;
use crate::proto::VERSION;

/// What the daemon tells a client about itself.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    /// The receiver's identity string.
    pub identity: String,
    /// Which command tree is in use.
    pub dialect: Dialect,
    /// Where the snapshot log lives, so a client can open it read-only.
    pub database: String,
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
        send(&writer, &Message::event(snapshot))?;
    }

    let updates = handle.subscribe();
    let pusher = Arc::clone(&writer);
    thread::Builder::new()
        .name("smartclockd-push".to_owned())
        .spawn(move || {
            for snapshot in updates {
                if send(&pusher, &Message::event(snapshot)).is_err() {
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
        send(&writer, &reply)?;
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
            }),
        ),
        Op::Query { scpi } => query(id, &scpi, handle, info.dialect),
    }
}

/// Run a client's command, refusing anything that is not read-only.
///
/// The gate is the command table, not a list kept here, so a command
/// reclassified there is reclassified for clients too.  An unknown
/// command is refused rather than passed through: phase 3 serves
/// queries only, and a spelling the table does not know could be
/// anything.
fn query(id: String, scpi: &str, handle: &Handle, dialect: Dialect) -> Message {
    let known = dialect
        .specs()
        .iter()
        .find(|s| s.scpi.eq_ignore_ascii_case(scpi.trim()));
    match known {
        Some(spec) if spec.class == Class::Query => match handle.request(spec.scpi) {
            Ok(reply) => Message::ok(id, serde_json::json!({ "lines": reply.lines })),
            Err(e) => Message::err(id, e),
        },
        Some(spec) => Message::err(
            id,
            format!(
                "{} is {:?}; this daemon serves queries only",
                spec.scpi, spec.class
            ),
        ),
        None => Message::err(id, format!("{scpi} is not a command this dialect knows")),
    }
}

fn send<W: Write>(writer: &Arc<Mutex<W>>, message: &Message) -> Result<()> {
    let line = serde_json::to_string(message)?;
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned writer"))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
