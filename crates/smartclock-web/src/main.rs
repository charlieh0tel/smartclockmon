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
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

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
    let cache = Cache::default();
    smartclock_http::serve(&cli.listen, move |target| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/" => Response::ok("text/html; charset=utf-8", PAGE.to_owned()),
            "/api/snapshot" => json(cache.snapshot(&socket)),
            "/api/info" => json(cache.info(&socket)),
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

/// Drop fields the page does not display.
///
/// This server has no authentication of its own -- the daemon's has
/// always been the group on its socket, and a TCP port has no group --
/// so whatever it serves is readable by anyone who can reach the port.
/// The surveyed position gives the antenna's location to within a few
/// metres, and the policy flags say which command classes are unlocked
/// on a daemon that can strand a serial link.  Neither appears on the
/// page, so neither needs to leave the machine.  The monitor and the
/// command line tool still see both: they come through the socket,
/// where group membership still means something.
fn withhold(value: &mut serde_json::Value, fields: &[&str]) {
    if let Some(object) = value.as_object_mut() {
        for field in fields {
            object.remove(*field);
        }
    }
}

/// How long an answer from the daemon is reused.
///
/// The daemon polls the fast tier once a second, so a reading fetched
/// more often than this is the same reading.  Without the cache every
/// open page cost a daemon connection per second, and the daemon
/// admits sixteen clients: a couple of browser tabs could take every
/// slot and lock the operator's own monitor out of a receiver they
/// have local access to.  A slot is released by the daemon's push
/// thread on its next snapshot, so the crowding outlasts the request
/// that caused it.
const CACHE_FOR: Duration = Duration::from_millis(900);

/// The daemon's answers, kept briefly.
///
/// One mutex rather than one connection: a held connection has to be
/// reconnected when the daemon restarts, and this way a request that
/// finds the cache warm does not touch the socket at all.
#[derive(Default)]
struct Cache {
    latest: Mutex<Option<(Instant, serde_json::Value)>>,
    info: Mutex<Option<(Instant, serde_json::Value)>>,
}

impl Cache {
    /// Answer from the cache, or ask the daemon and keep what it says.
    ///
    /// A failure is not cached: the daemon coming back should show up
    /// on the next request, not a second later.
    fn get<F>(
        cell: &Mutex<Option<(Instant, serde_json::Value)>>,
        ask: F,
    ) -> Result<serde_json::Value>
    where
        F: FnOnce() -> Result<serde_json::Value>,
    {
        let mut held = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, value)) = held.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return Ok(value.clone());
        }
        let value = ask()?;
        *held = Some((Instant::now(), value.clone()));
        Ok(value)
    }

    fn snapshot(&self, socket: &Path) -> Result<serde_json::Value> {
        Self::get(&self.latest, || {
            let mut value = Daemon::connect(socket)?.ask(Op::Latest)?;
            withhold(&mut value, &["position"]);
            Ok(value)
        })
    }

    fn info(&self, socket: &Path) -> Result<serde_json::Value> {
        Self::get(&self.info, || {
            let mut value = Daemon::connect(socket)?.info()?;
            withhold(
                &mut value,
                &["allow_control", "allow_dangerous", "allow_raw", "database"],
            );
            Ok(value)
        })
    }
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
