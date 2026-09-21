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

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use smartclock::client::Daemon;
use smartclock::protocol::Op;
use smartclock_http::Response;

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
    eprintln!("smartclock-web: serving http://{}/", cli.listen);
    let socket = cli.socket.clone();
    let database = cli.database.clone();
    smartclock_http::serve(&cli.listen, move |target| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/" => Response::ok("text/html; charset=utf-8", PAGE.to_owned()),
            "/api/snapshot" => json(snapshot(&socket)),
            "/api/info" => json(info(&socket)),
            "/api/history" => json(series(&database, query)),
            _ => Response::not_found(),
        }
    })
}

/// Wrap a result as JSON, reporting a failure as data rather than as an
/// HTTP error: the page can then say what went wrong in the place the
/// value would have been, instead of silently showing nothing.
fn json(result: Result<serde_json::Value>) -> Response {
    let value = match result {
        Ok(value) => value,
        Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
    };
    Response::ok("application/json; charset=utf-8", value.to_string())
}

fn snapshot(socket: &Path) -> Result<serde_json::Value> {
    let mut daemon = Daemon::connect(socket)?;
    Ok(daemon.ask(Op::Latest)?)
}

fn info(socket: &Path) -> Result<serde_json::Value> {
    let mut daemon = Daemon::connect(socket)?;
    Ok(daemon.info()?)
}

/// How much history a request that does not say gets.
const DEFAULT_WINDOW: i64 = 3600;

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
    // unwrap_or would evaluate this even when `from` was given, and
    // `?to=-9223372036854775808` then overflows before the value is
    // used at all -- which in a debug build kills the thread before
    // any response is written, so the caller gets a dropped socket
    // rather than the JSON error this function promises.
    let from = from.unwrap_or_else(|| to.saturating_sub(DEFAULT_WINDOW));

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
