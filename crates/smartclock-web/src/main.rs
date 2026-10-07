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

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use jiff::Timestamp;
use smartclock::client;
use smartclock::client::Daemon;
use smartclock::client::Daemons;
use smartclock::parse::Identity;
use smartclock::protocol::Op;
use smartclock_http::Response;
use smartclock_log::reader::Journal;
use smartclock_log::reader::Log;
use smartclock_log::reader::MAX_PHASE_ROWS;
use smartclock_log::reader::Receiver;
use smartclock_log::schema::PLOTTABLE;
use smartclock_log::schema::measured;
use smartclock_log::sensors::SensorLog;

/// The page, built in rather than read from disk: one file to install,
/// and a running server cannot be made to serve something else by
/// writing to a directory it happens to have.
const PAGE: &str = include_str!("index.html");

/// The receiver's status screen and the sky plot drawn from it, on a
/// page of their own.
///
/// Separate because it is the one view that costs the receiver a status
/// screen -- 1.5 s of a 19200 link, four fast polls -- so it is read
/// while someone is looking at it rather than on the daemon's schedule.
const STATUS_PAGE: &str = include_str!("status.html");

/// The Allan deviation, on a page of its own.
///
/// Separate because it is not a time series: it is one curve computed
/// over a whole range, on two log axes, and it shares neither the
/// history page's buckets nor its cursor.
const DEVIATION: &str = include_str!("adev.html");

/// Every receiver the logs hold, overlaid over one range: history a
/// chart per column, and their stability curves on one plot.
const COMPARE: &str = include_str!("compare.html");

/// Shared by every page.
const STYLE: &str = include_str!("style.css");

/// What every page shares: the status strip, the receiver picker, the
/// range control, and the lifecycle that decides when a page reads and
/// what it does when the receiver changes or its daemon goes away.
const COMMON: &str = include_str!("common.js");
const CHARTS: &str = include_str!("charts.js");

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// Where the daemons' sockets are: one instance per subdirectory,
    /// `<run-dir>/<instance>/socket`.  The live strip follows the
    /// selected receiver to whichever daemon is attached to it.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_RUN_DIR",
        default_value = "/run/smartclockd"
    )]
    run_dir: PathBuf,

    /// One daemon's socket in place of the directory.
    #[arg(long, env = "SMARTCLOCK_WEB_SOCKET")]
    socket: Option<PathBuf>,

    /// The daemon's logs, for history: one `.sqlite` file per receiver
    /// in this directory.  Opened read-only.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_LOG_DIR",
        default_value = "/var/lib/smartclockd"
    )]
    log_dir: PathBuf,

    /// One log file in place of the directory, holding whichever
    /// receivers the daemon logged into it.
    #[arg(long, env = "SMARTCLOCK_WEB_DATABASE")]
    database: Option<PathBuf>,

    /// Address to serve on.
    ///
    /// Localhost by default.  The page shows where the receiver is to
    /// within a few meters and has no authentication of its own, so
    /// reaching it from elsewhere should mean an SSH tunnel or a
    /// deliberate change here, not an accident.
    #[arg(long, env = "SMARTCLOCK_WEB_LISTEN", default_value = "127.0.0.1:9980")]
    listen: String,

    /// The sensor service's log, for the sensors' history.  Opened
    /// read-only; a host without one shows no sensors.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_SENSOR_LOG",
        default_value = "/var/lib/smartclock-sensord/sensors.sqlite"
    )]
    sensor_log: PathBuf,

    /// The sensor service's socket, for the sensors' latest readings.
    #[arg(
        long,
        env = "SMARTCLOCK_WEB_SENSOR_SOCKET",
        default_value = "/run/smartclock-sensord/socket"
    )]
    sensor_socket: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    eprintln!("smartclock-web: serving http://{}/", cli.listen);
    let daemons = match cli.socket.clone() {
        Some(file) => Daemons::File(file),
        None => Daemons::Dir(cli.run_dir.clone()),
    };
    let logs = match cli.database.clone() {
        Some(file) => Logs::File(file),
        None => Logs::Dir(cli.log_dir.clone()),
    };
    let cache = Cache::default();
    let (sensor_log, sensor_socket) = (cli.sensor_log.clone(), cli.sensor_socket.clone());
    smartclock_http::serve(&cli.listen, move |target| {
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        match path {
            "/" => Response::ok("text/html; charset=utf-8", PAGE.to_owned()),
            "/status" => Response::ok("text/html; charset=utf-8", STATUS_PAGE.to_owned()),
            "/adev" => Response::ok("text/html; charset=utf-8", DEVIATION.to_owned()),
            "/compare" => Response::ok("text/html; charset=utf-8", COMPARE.to_owned()),
            "/style.css" => Response::ok("text/css; charset=utf-8", STYLE.to_owned()),
            "/common.js" => Response::ok("text/javascript; charset=utf-8", COMMON.to_owned()),
            "/charts.js" => Response::ok("text/javascript; charset=utf-8", CHARTS.to_owned()),
            "/api/about" => json(Ok(about())),
            "/api/snapshot" => {
                json(choose(&daemons, &cache, query).and_then(|s| cache.snapshot(&s)))
            }
            "/api/info" => json(choose(&daemons, &cache, query).and_then(|s| cache.info(&s))),
            // Uncached, and the only endpoint that goes to the wire on
            // request: it is what the status page is paying for.
            "/api/status" => json(choose(&daemons, &cache, query).and_then(|s| status(&s))),
            "/api/history" => json(series(&logs, query)),
            "/api/journal" => json(journal(&logs, query)),
            "/api/facts" => json(facts(&logs, query)),
            "/api/notes" => json(notes(&logs, query)),
            "/api/receivers" => json(receivers(&logs, &daemons, &cache)),
            "/api/adev" => json(deviation(&logs, query)),
            "/api/sensors" => json(sensors(&sensor_log)),
            "/api/sensors/history" => json(sensor_history(&sensor_log, query)),
            "/api/sensors/latest" => json(sensors_latest(&sensor_socket)),
            _ => Response::not_found(),
        }
    })?;
    Ok(())
}

/// Read one status screen through the daemon.
///
/// Never cached: the point of the call is that it is fresh, and the
/// cost of it is why nothing else asks for one.
fn status(socket: &Path) -> Result<serde_json::Value> {
    Ok(Daemon::connect(socket)?.ask(Op::Status)?)
}

/// The first year of the project's copyright.
const COPYRIGHT_FROM: u16 = 2026;

/// What every page's colophon shows, from the package metadata: the
/// copyright, the license, this build, and the repository.
fn about() -> serde_json::Value {
    let holders: Vec<&str> = env!("CARGO_PKG_AUTHORS")
        .split(':')
        .map(|author| author.split(" <").next().unwrap_or(author).trim())
        .collect();
    serde_json::json!({
        "copyright": format!("{COPYRIGHT_FROM} {}", holders.join(", ")),
        "license": env!("CARGO_PKG_LICENSE"),
        "version": smartclock::VERSION,
        "repository": env!("CARGO_PKG_REPOSITORY"),
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

/// How long an answer from the daemon is reused.
///
/// The daemon polls the fast tier once a second, so a reading fetched
/// more often than this is the same reading.  Without the cache every
/// open page cost a daemon connection per second, and the daemon
/// admits sixteen clients: a couple of browser tabs could take every
/// slot and lock the operator's own monitor out of a receiver they
/// have local access to.  A slot is released by the daemon's push
/// thread up to a second after its client leaves, so the crowding
/// outlasts the request that caused it.
const CACHE_FOR: Duration = Duration::from_millis(900);

/// How long a cached answer may take.
///
/// The daemon answers these from what it already holds, so anything
/// near this is a daemon that has stopped answering, and the page is
/// better told so than kept waiting.
const ASK_BUDGET: Duration = Duration::from_secs(5);

/// The daemon's answers, kept briefly.
///
/// Locks rather than connections: a held connection has to be
/// reconnected when the daemon restarts, and this way a request that
/// finds the cache warm does not touch the socket at all.
///
/// A daemon's lock is held while it is asked, on purpose: requests
/// arriving together make one connection, not one each, which is what
/// keeps a few open tabs from taking the daemon's client slots.  One
/// lock per daemon, though, not one for them all, or a daemon that had
/// stopped answering held up every request about every other.
#[derive(Default)]
struct Cache {
    latest: Cells,
    info: Cells,
}

/// Each daemon's last answer, behind a lock of its own.
type Cells = Mutex<HashMap<PathBuf, Arc<Mutex<Option<Kept>>>>>;

/// One answer from a daemon, or why there was none, and when.
type Kept = (Instant, std::result::Result<serde_json::Value, String>);

impl Cache {
    /// Answer from the cache, or ask the daemon and keep what it says.
    ///
    /// A failure is kept as briefly as an answer.  Not keeping it made
    /// every request queued behind a daemon that had stopped answering
    /// ask again in turn, each waiting out the client's deadline, so
    /// the last of them waited for all of them.  Kept, they share the
    /// one failure, and a daemon coming back shows at most `CACHE_FOR`
    /// later.
    fn get<F>(cells: &Cells, socket: &Path, ask: F) -> Result<serde_json::Value>
    where
        F: FnOnce() -> Result<serde_json::Value>,
    {
        let cell = {
            let mut cells = cells
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(cells.entry(socket.to_path_buf()).or_default())
        };
        let mut held = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, kept)) = held.as_ref()
            && at.elapsed() < CACHE_FOR
        {
            return kept.clone().map_err(anyhow::Error::msg);
        }
        let asked = ask().map_err(|e| format!("{e:#}"));
        *held = Some((Instant::now(), asked.clone()));
        asked.map_err(anyhow::Error::msg)
    }

    fn snapshot(&self, socket: &Path) -> Result<serde_json::Value> {
        Self::get(&self.latest, socket, || {
            Ok(Daemon::connect_within(socket, ASK_BUDGET)?.ask(Op::Latest)?)
        })
    }

    fn info(&self, socket: &Path) -> Result<serde_json::Value> {
        Self::get(&self.info, socket, || {
            Ok(Daemon::connect_within(socket, ASK_BUDGET)?.info()?)
        })
    }
}

/// One daemon that answered, and what it is attached to.
struct Live {
    socket: PathBuf,
    /// The instance name, the subdirectory's; empty for a socket
    /// named outright.
    instance: String,
    /// The receiver it is attached to, when its identity parses and
    /// names a serial.
    identity: Option<Identity>,
}

impl Live {
    /// The serial of the receiver it is attached to.
    fn serial(&self) -> Option<&str> {
        self.identity.as_ref().map(|id| id.serial.as_str())
    }
}

/// Every daemon that answers, by instance name.
///
/// A socket nobody answers on -- an instance stopped with its
/// directory still there -- is left out, not an error: the page
/// shows what is live, and history for the rest.
///
/// All asked at once, so one that has wedged costs a request its
/// [`ASK_BUDGET`] and no more, however many others there are.
fn live(daemons: &Daemons, cache: &Cache) -> Vec<Live> {
    let instances = daemons.sockets();
    thread::scope(|scope| {
        let asked: Vec<_> = instances
            .iter()
            .map(|instance| scope.spawn(|| cache.info(&instance.socket)))
            .collect();
        instances
            .iter()
            .zip(asked)
            .filter_map(|(instance, asked)| {
                let info = asked.join().ok()?.ok()?;
                Some(Live {
                    identity: client::identity(&info).filter(|id| !id.serial.is_empty()),
                    socket: instance.socket.clone(),
                    instance: instance.name.clone(),
                })
            })
            .collect()
    })
}

/// The daemon a live request is about.
///
/// `?receiver=<serial>` names the daemon attached to that unit; a
/// unit no daemon is attached to has history and no live strip,
/// and the page is told so.  Without one, the only daemon, or the
/// first by instance name.
fn choose(daemons: &Daemons, cache: &Cache, query: &str) -> Result<PathBuf> {
    let asked = smartclock_http::value(query, "receiver");
    match asked {
        Some(serial) => live(daemons, cache)
            .into_iter()
            .find(|d| d.serial() == Some(serial.as_str()))
            .map(|d| d.socket)
            .ok_or_else(|| anyhow::anyhow!("no daemon is attached to receiver {serial}")),
        None => daemons
            .sockets()
            .into_iter()
            .next()
            .map(|instance| instance.socket)
            .ok_or_else(|| anyhow::anyhow!("no daemon is running")),
    }
}

/// How many of each journal stream the page is given.
///
/// Enough to see a pattern, few enough that the page stays a page.
/// The whole of the receiver's log is 222 entries, so this shows most
/// of one without paging.
const JOURNAL_ROWS: usize = 100;

/// The receiver's own record-keeping -- its diagnostic log, the
/// transitions taken from its event registers, and its error queue --
/// and the notes a person wrote about it.
fn journal(logs: &Logs, query: &str) -> Result<serde_json::Value> {
    let journal = match logs.choose(query)? {
        Some((log, receiver)) => log.journal(receiver, JOURNAL_ROWS)?,
        None => Journal::default(),
    };
    Ok(serde_json::to_value(journal)?)
}

/// The receiver's notes over `from` to `to`, unix seconds, for the
/// charts to mark; the whole log when either is left out.
fn notes(logs: &Logs, query: &str) -> Result<serde_json::Value> {
    let bound = |key| smartclock_http::value(query, key).and_then(|v| v.parse::<i64>().ok());
    let notes = match logs.choose(query)? {
        Some((log, receiver)) => log.notes(
            receiver,
            bound("from").unwrap_or(0),
            bound("to").unwrap_or(NO_LATER_THAN),
        )?,
        None => Vec::new(),
    };
    Ok(serde_json::to_value(notes)?)
}

/// The sensor log, if there is one: a host without the sensor service
/// has none, which is not an error.
fn sensor_log(path: &Path) -> Result<Option<SensorLog>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(SensorLog::open(path)?))
}

/// The sensors the log holds, how often they were read, and the span
/// of their readings.
fn sensors(path: &Path) -> Result<serde_json::Value> {
    let Some(log) = sensor_log(path)? else {
        return Ok(
            serde_json::json!({ "sensors": [], "every_s": null, "first": null, "last": null }),
        );
    };
    let extent = log.extent()?;
    Ok(serde_json::json!({
        "sensors": log.sensors()?,
        "every_s": log.every().map(|every| every.as_secs_f64()),
        "first": extent.map(|(first, _)| first),
        "last": extent.map(|(_, last)| last),
    }))
}

/// `?quantity=temperature&from=...&to=...&points=1500`: every sensor of
/// one quantity, a line each, bucketed on the receivers' grid.
fn sensor_history(path: &Path, query: &str) -> Result<serde_json::Value> {
    let value = |key| smartclock_http::value(query, key);
    let number = |key| value(key).and_then(|v: String| v.parse::<i64>().ok());
    let quantity = value("quantity").ok_or_else(|| anyhow::anyhow!("which quantity?"))?;
    let (Some(from), Some(to)) = (number("from"), number("to")) else {
        anyhow::bail!("a range is from= and to=, in unix seconds");
    };
    let points = value("points").and_then(|v| v.parse().ok()).unwrap_or(1500);
    let lines = match sensor_log(path)? {
        Some(log) => log.series(&quantity, from, to, points)?,
        None => Vec::new(),
    };
    Ok(serde_json::to_value(lines)?)
}

/// Every sensor's latest reading, from the sensor service, each with
/// whether it is current, so the page applies the one rule the exporter
/// and the command line do rather than a copy of it.
fn sensors_latest(socket: &Path) -> Result<serde_json::Value> {
    let latest = Daemon::connect_within(socket, ASK_BUDGET)?.sensor_readings()?;
    let now = Timestamp::now();
    let current: Vec<bool> = latest
        .readings
        .iter()
        .map(|r| r.current(latest.every_s, now).is_some())
        .collect();
    let mut value = serde_json::to_value(latest)?;
    if let Some(readings) = value.get_mut("readings").and_then(|r| r.as_array_mut()) {
        for (reading, current) in readings.iter_mut().zip(current) {
            if let Some(fields) = reading.as_object_mut() {
                fields.insert("current".to_owned(), current.into());
            }
        }
    }
    Ok(value)
}

/// An upper bound past any time a log holds: 2100-01-01, unix seconds.
const NO_LATER_THAN: i64 = 4_102_444_800;

/// The current value of each fact a person recorded about the receiver.
fn facts(logs: &Logs, query: &str) -> Result<serde_json::Value> {
    let facts = match logs.choose(query)? {
        Some((log, receiver)) => log.facts(receiver)?,
        None => Vec::new(),
    };
    Ok(serde_json::to_value(facts)?)
}

/// The Allan deviation of the 1 PPS interval over a range.
///
/// Its own endpoint rather than another column of `/api/history`: a
/// deviation is not a time series and cannot be bucketed like one.  The
/// whole run at full rate is what the estimator needs, since averaging
/// readings together before it sees them is precisely the operation it
/// exists to perform.
fn deviation(logs: &Logs, query: &str) -> Result<serde_json::Value> {
    let (mut from, mut to) = (None, None);
    for (key, value) in smartclock_http::pairs(query) {
        match key {
            "from" => from = value.parse::<i64>().ok(),
            "to" => to = value.parse::<i64>().ok(),
            _ => {}
        }
    }
    let Some((log, receiver)) = logs.choose(query)? else {
        anyhow::bail!("no log names a receiver, so there is nothing to measure");
    };
    let (_, last) = log.extent(receiver)?;
    #[expect(clippy::cast_possible_truncation, reason = "unix seconds fit an i64")]
    let to = to.unwrap_or(last as i64);
    let from = from.unwrap_or_else(|| to.saturating_sub(DEFAULT_WINDOW));
    let (curve, truncated) = log.phase(receiver, from, to, MAX_PHASE_ROWS)?;
    let mut value = serde_json::to_value(curve)?;
    if let Some(fields) = value.as_object_mut() {
        fields.insert("truncated".to_owned(), truncated.into());
    }
    Ok(value)
}

/// Where the history is.
enum Logs {
    /// The daemon's directory: one log per receiver, named
    /// `<model>-<serial>.sqlite`.
    Dir(PathBuf),
    /// One file, holding whichever receivers were logged into it.
    File(PathBuf),
}

/// One receiver, and the log it was found in.
struct Found {
    path: PathBuf,
    receiver: Receiver,
}

impl Logs {
    fn files(&self) -> Result<Vec<PathBuf>> {
        match self {
            Self::File(file) => Ok(vec![file.clone()]),
            Self::Dir(dir) => {
                let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                    .with_context(|| format!("reading {}", dir.display()))?
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "sqlite"))
                    .collect();
                files.sort();
                Ok(files)
            }
        }
    }

    /// Every receiver in every log, newest first by when it was last
    /// seen.
    ///
    /// A file in the directory that will not open is said on stderr
    /// and passed over, so one bad file does not hide the others; the
    /// one file named outright is not, since there is nothing else to
    /// show.
    fn receivers(&self) -> Result<Vec<Found>> {
        let mut found = Vec::new();
        for path in self.files()? {
            let receivers = match Log::open(&path).and_then(|log| log.receivers()) {
                Ok(receivers) => receivers,
                Err(e) if matches!(self, Self::Dir(_)) => {
                    eprintln!("smartclock-web: {e:#}; skipped");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            found.extend(receivers.into_iter().map(|receiver| Found {
                path: path.clone(),
                receiver,
            }));
        }
        found.sort_by(|a, b| b.receiver.last_seen.cmp(&a.receiver.last_seen));
        // A serial is one unit, so the same serial in a second file --
        // a copy left in the directory, or the old single log beside
        // its split -- is the same unit, and the file seen most recently
        // is the one with its history.
        // Not `dedup_by`, which only merges neighbors: two files
        // seen between them leave a unit listed twice.
        let mut listed = HashSet::new();
        found.retain(|f| listed.insert(f.receiver.serial.clone()));
        Ok(found)
    }

    /// The log and receiver a request is about.
    ///
    /// `?receiver=<serial>` when the page has one selected, and
    /// otherwise the most recently seen -- the only one on a bench
    /// with one unit, and the attached one on a bench where they are
    /// swapped.  A serial that names no receiver is refused rather
    /// than quietly serving a different unit's history under its name.
    /// `None` when no log names a receiver.
    fn choose(&self, query: &str) -> Result<Option<(Log, i64)>> {
        let asked = smartclock_http::value(query, "receiver");
        let found = self.receivers()?;
        let chosen = match &asked {
            None => found.first(),
            Some(serial) => found.iter().find(|f| f.receiver.serial == *serial),
        };
        let Some(chosen) = chosen else {
            anyhow::ensure!(
                asked.is_none(),
                "no log holds receiver {}",
                asked.unwrap_or_default()
            );
            return Ok(None);
        };
        Ok(Some((Log::open(&chosen.path)?, chosen.receiver.id)))
    }
}

/// Every receiver the logs hold or a daemon is attached to, for the
/// page's selector: live ones first, then by when last seen.
///
/// A unit a daemon is attached to but no log names yet -- answered
/// `*IDN?` moments ago, first row not yet written -- is listed from
/// its identity alone, so the selector never lacks the unit that is
/// actually on the bench.
fn receivers(logs: &Logs, daemons: &Daemons, cache: &Cache) -> Result<serde_json::Value> {
    let attached = live(daemons, cache);
    let mut found: Vec<Listed> = logs
        .receivers()?
        .into_iter()
        .map(|f| Listed::new(f.receiver, None))
        .collect();
    for daemon in &attached {
        let Some(id) = &daemon.identity else {
            continue;
        };
        match found.iter_mut().find(|l| l.receiver.serial == id.serial) {
            Some(listed) => listed.instance = Some(daemon.instance.clone()),
            None => found.push(Listed::new(
                Receiver {
                    id: 0,
                    serial: id.serial.clone(),
                    model: id.model.clone(),
                    firmware: id.firmware.clone(),
                    first_seen: String::new(),
                    last_seen: String::new(),
                    gps_engine: None,
                },
                Some(daemon.instance.clone()),
            )),
        }
    }
    found.sort_by(|a, b| {
        b.instance
            .is_some()
            .cmp(&a.instance.is_some())
            .then_with(|| b.receiver.last_seen.cmp(&a.receiver.last_seen))
    });
    Ok(serde_json::to_value(found)?)
}

/// One receiver as the page's selector lists it.
#[derive(serde::Serialize)]
struct Listed {
    #[serde(flatten)]
    receiver: Receiver,
    /// The daemon instance attached to it now, if one is.  Not the
    /// log's to know; filled in from the daemons that answer.
    instance: Option<String>,
    /// The plottable columns this unit measures, from its model.
    columns: Vec<&'static str>,
}

impl Listed {
    fn new(receiver: Receiver, instance: Option<String>) -> Self {
        Self {
            columns: measured(&receiver.model),
            receiver,
            instance,
        }
    }
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
fn series(logs: &Logs, query: &str) -> Result<serde_json::Value> {
    let mut columns: Vec<String> = Vec::new();
    let (mut from, mut to, mut points) = (None, None, 1500usize);
    for (key, value) in smartclock_http::pairs(query) {
        match key {
            // Split after decoding, not before: a `+` is a space by
            // the time it gets here, and `%2C` is a comma.
            "columns" | "column" => columns.extend(
                value
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

    let Some((log, receiver)) = logs.choose(query)? else {
        anyhow::bail!("no log names a receiver, so there is nothing to plot");
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
        "plottable": PLOTTABLE.map(|(column, _, _)| column),
        "receiver": log.receivers()?.iter().find(|r| r.id == receiver).map(|r| r.serial.clone()),
        // uPlot wants parallel arrays, not an array of points.  One
        // `at` for all of them: that is the alignment, stated once.
        "at": series.at,
        "plots": series.plots,
    }))
}

#[cfg(test)]
mod tests {
    use super::CHARTS;
    use super::COMMON;
    use super::COMPARE;
    use super::Cache;
    use super::DEVIATION;
    use super::Logs;
    use super::PAGE;
    use super::STATUS_PAGE;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn requests_queued_behind_a_silent_daemon_share_its_failure() {
        // Three requests at once, and a daemon that takes 300 ms to
        // fail.  One ask, and all three told, in about the time of one.
        let cache = Arc::new(Cache::default());
        let asks = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        let requests: Vec<_> = (0..3)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let asks = Arc::clone(&asks);
                std::thread::spawn(move || {
                    Cache::get(&cache.latest, Path::new("/nowhere"), || {
                        asks.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(300));
                        Err(anyhow::anyhow!("it did not answer"))
                    })
                })
            })
            .collect();
        for request in requests {
            let outcome = request.join().expect("a request");
            assert!(outcome.is_err_and(|e| e.to_string().contains("did not answer")));
        }
        assert_eq!(asks.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_millis(800));
    }

    #[test]
    fn a_silent_daemon_holds_up_only_requests_about_itself() {
        let cache = Arc::new(Cache::default());
        let silent = {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                Cache::get(&cache.info, Path::new("/silent"), || {
                    std::thread::sleep(Duration::from_millis(500));
                    Err(anyhow::anyhow!("it did not answer"))
                })
            })
        };
        // Let the silent one take its lock first.
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        let other = Cache::get(&cache.info, Path::new("/other"), || {
            Ok(serde_json::json!({}))
        });
        assert!(other.is_ok());
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "waited {:?} behind another daemon",
            started.elapsed()
        );
        assert!(silent.join().expect("the silent request").is_err());
    }

    #[test]
    fn a_unit_in_two_logs_is_listed_once_even_with_another_between() {
        let dir = std::env::temp_dir().join(format!("smartclock-web-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a log directory");
        // X seen most recently in one file and earliest in another,
        // with Y seen in between: sorted by when seen, the two X are
        // not neighbors.
        for (file, serial, seen) in [
            ("a.sqlite", "X", "2026-09-03"),
            ("b.sqlite", "Y", "2026-09-02"),
            ("c.sqlite", "X", "2026-09-01"),
        ] {
            let conn = rusqlite::Connection::open(dir.join(file)).expect("a log");
            conn.execute_batch(smartclock_log::schema::META)
                .expect("meta");
            conn.execute_batch(smartclock_log::schema::TABLES)
                .expect("the tables");
            conn.execute(
                "INSERT INTO receiver (serial, first_seen, last_seen) VALUES (?1, ?2, ?2)",
                [serial, seen],
            )
            .expect("a receiver");
        }
        let found = Logs::Dir(dir.clone()).receivers().expect("the receivers");
        let _ = std::fs::remove_dir_all(&dir);
        let listed: Vec<(&str, &str)> = found
            .iter()
            .map(|f| {
                (
                    f.receiver.serial.as_str(),
                    f.path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(listed, vec![("X", "a.sqlite"), ("Y", "b.sqlite")]);
    }

    /// No two top-level names in a page and the scripts it loads clash.
    ///
    /// A page's scripts share one global scope, and JavaScript lets a
    /// second `function f()` replace the first without a word; a second
    /// `let` or `const` throws, but only in a browser.  Nothing else
    /// checks: the pages are strings in this binary, with no linter.
    #[test]
    fn no_page_declares_a_name_twice() {
        /// The top-level names `script` declares: function, class, let
        /// and const, at the start of a line.
        fn declared(script: &str) -> Vec<&str> {
            script
                .lines()
                .filter_map(|line| {
                    ["function ", "async function ", "class ", "let ", "const "]
                        .iter()
                        .find_map(|keyword| line.strip_prefix(keyword))
                })
                .filter_map(|rest| {
                    let name = rest
                        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                        .next()?;
                    (!name.is_empty()).then_some(name)
                })
                .collect()
        }
        for (name, page) in [
            ("index.html", PAGE),
            ("status.html", STATUS_PAGE),
            ("adev.html", DEVIATION),
            ("compare.html", COMPARE),
        ] {
            let mut scripts = vec![COMMON, page];
            if page.contains("/charts.js") {
                scripts.push(CHARTS);
            }
            let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for script in scripts {
                for declared in declared(script) {
                    *seen.entry(declared).or_default() += 1;
                }
            }
            let repeated: Vec<&str> = seen
                .iter()
                .filter(|(_, n)| **n > 1)
                .map(|(name, _)| *name)
                .collect();
            assert!(
                repeated.is_empty(),
                "declared more than once with {name}: {repeated:?}"
            );
            // And the guard is worth nothing if the scan found nothing.
            assert!(
                seen.len() > 20,
                "only found {} names with {name}",
                seen.len()
            );
        }
    }
}
