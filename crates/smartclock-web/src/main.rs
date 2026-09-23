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

/// The sky plot, on a page of its own.
///
/// Separate because it is the one view that costs the receiver a status
/// screen -- 1.5 s of a 19200 link, four fast polls -- so it is read
/// while someone is looking at it rather than on the daemon's schedule.
const SKY: &str = include_str!("sky.html");

/// The Allan deviation, on a page of its own.
///
/// Separate because it is not a time series: it is one curve computed
/// over a whole range, on two log axes, and it shares neither the
/// history page's buckets nor its cursor.
const DEVIATION: &str = include_str!("adev.html");

/// Shared by every page.
const STYLE: &str = include_str!("style.css");

/// The status strip, shared by every page.
///
/// The state of the receiver is the context for whatever else a page
/// shows: a sky plot or a stability curve read without knowing the
/// unit is in holdover is read wrongly.
const STATUS: &str = include_str!("status.js");

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
            "/sky" => Response::ok("text/html; charset=utf-8", SKY.to_owned()),
            "/adev" => Response::ok("text/html; charset=utf-8", DEVIATION.to_owned()),
            "/style.css" => Response::ok("text/css; charset=utf-8", STYLE.to_owned()),
            "/status.js" => Response::ok("text/javascript; charset=utf-8", STATUS.to_owned()),
            "/api/snapshot" => json(cache.snapshot(&socket)),
            "/api/info" => json(cache.info(&socket)),
            // Uncached, and the only endpoint that goes to the wire on
            // request: it is what the sky page is paying for.
            "/api/sky" => json(sky(&socket)),
            "/api/history" => json(series(&database, query)),
            "/api/journal" => json(journal(&database, query)),
            "/api/receivers" => json(receivers(&database)),
            "/api/adev" => json(deviation(&database, query)),
            _ => Response::not_found(),
        }
    })
}

/// Percent-decode one query-string value.
///
/// The parser here splits the raw target on `&` and `=` and did no
/// decoding at all, which was invisible while the only parameter that
/// mattered was a single column name with nothing to encode.  A list
/// broke it immediately: `URLSearchParams` writes the separator as
/// `%2C`, so the server was handed one column called
/// `efc_percent%2Ctemperature_c` and said, correctly, that it does not
/// serve it.
///
/// Bytes rather than chars, because a percent escape encodes a byte and
/// a multi-byte character arrives as several of them.  Anything that
/// is not a well-formed escape is kept as written: a stray `%` in a
/// value is not worth refusing a request over.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                // A literal plus is `%2B`; a bare one is a space in
                // form encoding, and a space is not a column name.
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Wrap a result as JSON, reporting a failure as data rather than as an
/// HTTP error: the page can then say what went wrong in the place the
/// value would have been, instead of silently showing nothing.
/// Read one status screen through the daemon.
///
/// Never cached: the point of the call is that it is fresh, and the
/// cost of it is why nothing else asks for one.
fn sky(socket: &Path) -> Result<serde_json::Value> {
    Ok(Daemon::connect(socket)?.ask(Op::Sky)?)
}

fn json(result: Result<serde_json::Value>) -> Response {
    let value = match result {
        Ok(value) => value,
        Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
    };
    Response::ok("application/json; charset=utf-8", value.to_string())
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
            Ok(Daemon::connect(socket)?.ask(Op::Latest)?)
        })
    }

    fn info(&self, socket: &Path) -> Result<serde_json::Value> {
        Self::get(&self.info, || Ok(Daemon::connect(socket)?.info()?))
    }
}

/// How many of each journal stream the page is given.
///
/// Enough to see a pattern, few enough that the page stays a page.
/// The whole of the receiver's log is 222 entries, so this shows most
/// of one without paging.
const JOURNAL_ROWS: usize = 100;

/// The receiver's own record-keeping: its diagnostic log, the
/// transitions taken from its event registers, and its error queue.
fn journal(database: &Path, query: &str) -> Result<serde_json::Value> {
    let log = Log::open(database)?;
    let Some(receiver) = chosen_receiver(&log, query)? else {
        return Ok(serde_json::json!({ "entries": [], "events": [], "errors": [] }));
    };
    Ok(serde_json::to_value(log.journal(receiver, JOURNAL_ROWS)?)?)
}

/// The Allan deviation of the 1 PPS interval over a range.
///
/// Its own endpoint rather than another column of `/api/history`: a
/// deviation is not a time series and cannot be bucketed like one.  The
/// whole run at full rate is what the estimator needs, since averaging
/// readings together before it sees them is precisely the operation it
/// exists to perform.
fn deviation(database: &Path, query: &str) -> Result<serde_json::Value> {
    let (mut from, mut to) = (None, None);
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "from" => from = value.parse::<i64>().ok(),
            "to" => to = value.parse::<i64>().ok(),
            _ => {}
        }
    }
    let log = Log::open(database)?;
    let Some(receiver) = chosen_receiver(&log, query)? else {
        anyhow::bail!("this log names no receiver, so there is nothing to measure");
    };
    let (_, last) = log.extent(receiver)?;
    #[expect(clippy::cast_possible_truncation, reason = "unix seconds fit an i64")]
    let to = to.unwrap_or(last as i64);
    let from = from.unwrap_or_else(|| to.saturating_sub(DEFAULT_WINDOW));
    Ok(serde_json::to_value(log.phase(receiver, from, to)?)?)
}

/// Every receiver the log holds, for the page's selector.
fn receivers(database: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(Log::open(database)?.receivers()?)?)
}

/// Which receiver a request is about.
///
/// `?receiver=<id>` when the page has one selected, and otherwise the
/// most recently seen -- the only one on a bench with one unit, and
/// the attached one on a bench where they are swapped.  An id that
/// names no receiver is refused rather than quietly serving a
/// different unit's history under its name.
fn chosen_receiver(log: &Log, query: &str) -> Result<Option<i64>> {
    let asked = query.split('&').find_map(|pair| {
        pair.split_once('=')
            .filter(|(key, _)| *key == "receiver")
            .and_then(|(_, value)| value.parse::<i64>().ok())
    });
    let Some(asked) = asked else {
        return log.newest_receiver();
    };
    anyhow::ensure!(
        log.receivers()?.iter().any(|r| r.id == asked),
        "this log holds no receiver {asked}"
    );
    Ok(Some(asked))
}

/// How much history a request that does not say gets.
const DEFAULT_WINDOW: i64 = 3600;

/// `?columns=efc_percent,temperature_c&from=...&to=...&points=1500`
///
/// Absolute unix times rather than a named window, so the page can ask
/// for whatever range it has zoomed to.  Several columns rather than
/// one, so stacked charts share a bucketing and therefore an x axis;
/// `column=` singular is still accepted, since a bookmarked link from
/// before this predates the plural.
fn series(database: &Path, query: &str) -> Result<serde_json::Value> {
    let mut columns: Vec<String> = Vec::new();
    let (mut from, mut to, mut points) = (None, None, 1500usize);
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            // Split after decoding, not before: a `+` is a space by
            // the time it gets here, and `%2C` is a comma.
            "columns" | "column" => columns.extend(
                decode(value)
                    .split([',', ' '])
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
            ),
            "from" => from = value.parse::<i64>().ok(),
            "to" => to = value.parse::<i64>().ok(),
            "points" => points = value.parse().unwrap_or(1500),
            _ => {}
        }
    }
    if columns.is_empty() {
        columns.push("efc_percent".to_owned());
    }

    let log = Log::open(database)?;
    let Some(receiver) = chosen_receiver(&log, query)? else {
        anyhow::bail!("this log names no receiver, so there is nothing to plot");
    };
    let (first, last) = log.extent(receiver)?;
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

    let series = log.series(receiver, &columns, from, to, points)?;
    Ok(serde_json::json!({
        "from": from,
        "to": to,
        "first": first,
        "last": last,
        // What may be asked for, so the page builds its menu from the
        // server rather than from a copy that can drift.
        "plottable": PLOTTABLE,
        "receiver": receiver,
        // uPlot wants parallel arrays, not an array of points.  One
        // `at` for all of them: that is the alignment, stated once.
        "at": series.at,
        "plots": series.plots,
    }))
}

#[cfg(test)]
mod tests {
    use super::PAGE;
    use super::decode;

    /// No two top-level functions in the page share a name.
    ///
    /// JavaScript lets a second `function f()` replace the first
    /// without a word, and the page has no linter to say otherwise: it
    /// is a string in this binary.  That cost an evening once.  A tick
    /// formatter called `tick` silently replaced the status strip's
    /// poller, also called `tick`, so boot invoked the formatter with
    /// no arguments, threw on undefined, and left the strip reading
    /// "connecting..." while every chart drew correctly -- a failure
    /// that looked like a daemon problem and was not.
    #[test]
    fn the_page_declares_each_function_once() {
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for line in PAGE.lines() {
            // Top level only: a nested function is scoped and may
            // legitimately reuse a name.
            let Some(rest) = line
                .strip_prefix("function ")
                .or_else(|| line.strip_prefix("async function "))
            else {
                continue;
            };
            let Some(name) = rest.split('(').next() else {
                continue;
            };
            *seen.entry(name.trim()).or_default() += 1;
        }
        let repeated: Vec<&str> = seen
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(name, _)| *name)
            .collect();
        assert!(
            repeated.is_empty(),
            "declared more than once in index.html: {repeated:?}"
        );
        // And the guard is worth nothing if the scan found nothing.
        assert!(
            seen.len() > 10,
            "only found {} functions to check",
            seen.len()
        );
    }

    /// What a browser actually sends for a list.
    ///
    /// `URLSearchParams` encodes the separator, so the server saw one
    /// column named `efc_percent%2Ctemperature_c` and refused it.  The
    /// single-column parameter this replaced had nothing to encode,
    /// which is why the missing decode went unnoticed.
    #[test]
    fn a_percent_encoded_list_decodes() {
        assert_eq!(
            decode("efc_percent%2Ctemperature_c"),
            "efc_percent,temperature_c"
        );
    }

    #[test]
    fn a_plus_is_a_space_and_a_percent_2b_is_a_plus() {
        assert_eq!(decode("one+two"), "one two");
        assert_eq!(decode("one%2Btwo"), "one+two");
    }

    /// A malformed escape is kept rather than refused.  A stray percent
    /// in a value is not worth failing a request over, and the column
    /// check downstream rejects anything that is not a real column
    /// anyway.
    #[test]
    fn a_broken_escape_survives() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%zz"), "%zz");
        assert_eq!(decode("%2"), "%2");
    }

    #[test]
    fn multibyte_characters_survive_the_round_trip() {
        assert_eq!(decode("%C2%B5s"), "\u{b5}s");
    }
}
