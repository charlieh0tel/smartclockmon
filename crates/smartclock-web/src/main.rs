//! A browser view of a SmartClock receiver.
//!
//! Serves one page and some JSON.  The page does the drawing, because
//! a browser already knows how to draw and the alternative is
//! reinventing a chart in Rust, badly.  The server's whole job is to
//! answer three questions: what is the receiver doing now, what has it
//! been doing, and what is this daemon attached to.
//!
//! Read-only.  There is no way to send a command from here, which is
//! deliberate: the socket's group membership is the whole of the
//! daemon's authorization, and a page on a TCP port has none of it.

mod history;

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Write as _;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::thread;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use smartclock::client::Daemon;

use crate::history::Log;
use crate::history::PLOTTABLE;

/// The page, built in rather than read from disk: one file to install,
/// and a running server cannot be made to serve something else by
/// writing to a directory it happens to have.
const PAGE: &str = include_str!("index.html");

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// The daemon's socket, for live readings.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_SOCKET",
        default_value = "/run/smartclockd/socket"
    )]
    socket: PathBuf,

    /// The daemon's log, for history.  Opened read-only.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_DATABASE",
        default_value = "/var/lib/smartclockd/snapshots.sqlite"
    )]
    database: PathBuf,

    /// Address to serve on.
    ///
    /// Localhost by default.  The page shows where the receiver is to
    /// within a few metres and has no authentication of its own, so
    /// reaching it from elsewhere should mean an SSH tunnel or a
    /// deliberate change here, not an accident.
    #[arg(long, env = "SMARTCLOCK_WEB_LISTEN", default_value = "127.0.0.1:9980")]
    listen: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let listener =
        TcpListener::bind(&cli.listen).with_context(|| format!("binding {}", cli.listen))?;
    eprintln!("smartclock-web: serving http://{}/", cli.listen);

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("smartclock-web: rejected a connection: {e}");
                continue;
            }
        };
        let socket = cli.socket.clone();
        let database = cli.database.clone();
        let spawned = thread::Builder::new()
            .name("smartclock-web-client".to_owned())
            .spawn(move || serve(&stream, &socket, &database));
        if let Err(e) = spawned {
            eprintln!("smartclock-web: could not serve a request: {e}");
        }
    }
    Ok(())
}

/// Answer one request.
fn serve(stream: &TcpStream, socket: &Path, database: &Path) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let (status, kind, body) = match path {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.to_owned()),
        "/api/snapshot" => json(snapshot(socket)),
        "/api/info" => json(info(socket)),
        "/api/history" => json(series(database, query)),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_owned(),
        ),
    };
    let mut writer = stream;
    let _ = write!(
        writer,
        "HTTP/1.1 {status}\r\n\
         Content-Type: {kind}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Wrap a result as JSON, reporting a failure as data rather than as an
/// HTTP error: the page can then say what went wrong in the place the
/// value would have been, instead of silently showing nothing.
fn json(result: Result<serde_json::Value>) -> (&'static str, &'static str, String) {
    let value = match result {
        Ok(value) => value,
        Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
    };
    (
        "200 OK",
        "application/json; charset=utf-8",
        value.to_string(),
    )
}

fn snapshot(socket: &Path) -> Result<serde_json::Value> {
    let mut daemon = Daemon::connect(socket)?;
    Ok(daemon.ask(smartclock::protocol::Op::Latest)?)
}

fn info(socket: &Path) -> Result<serde_json::Value> {
    let mut daemon = Daemon::connect(socket)?;
    Ok(daemon.info()?)
}

/// `?column=efc_percent&from=...&to=...&points=1500`
///
/// Absolute unix times rather than a named window, so the page can ask
/// for whatever range it has zoomed to.
fn series(database: &Path, query: &str) -> Result<serde_json::Value> {
    let mut column = "efc_percent".to_owned();
    let (mut from, mut to, mut points) = (None, None, 1500usize);
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "column" => column = value.to_owned(),
            "from" => from = value.parse::<i64>().ok(),
            "to" => to = value.parse::<i64>().ok(),
            "points" => points = value.parse().unwrap_or(1500),
            _ => {}
        }
    }

    let log = Log::open(database)?;
    let (first, last) = log.extent()?;
    // Default to the last hour of whatever exists, so a page loaded
    // against a log that stopped yesterday still shows something.
    #[expect(clippy::cast_possible_truncation, reason = "unix seconds fit an i64")]
    let to = to.unwrap_or(last as i64);
    let from = from.unwrap_or(to - 3600);

    let buckets = log.series(&column, from, to, points)?;
    Ok(serde_json::json!({
        "column": column,
        "from": from,
        "to": to,
        "first": first,
        "last": last,
        "columns": PLOTTABLE,
        // uPlot wants parallel arrays, not an array of points.
        "at": buckets.iter().map(|b| b.at).collect::<Vec<_>>(),
        "mean": buckets.iter().map(|b| b.mean).collect::<Vec<_>>(),
        "min": buckets.iter().map(|b| b.min).collect::<Vec<_>>(),
        "max": buckets.iter().map(|b| b.max).collect::<Vec<_>>(),
    }))
}
