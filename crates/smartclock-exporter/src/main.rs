//! Prometheus exporter for the SmartClock receivers on a host.
//!
//! Asks every daemon on each scrape rather than holding subscriptions,
//! and asks for its last reading rather than a fresh one, so scraping
//! costs the receivers nothing and cannot compete with the poll
//! schedule.  Prometheus decides how often that happens; each daemon
//! decides how often its receiver is read.  One endpoint for all of
//! them, with the daemon and the receiver's serial as labels, which is
//! how Prometheus tells things apart.
//!
//! No async runtime and no HTTP crate.  A scrape is one short request
//! answered from a value already in hand, and the rest of this
//! workspace is blocking and synchronous.

mod metrics;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use smartclock::client;
use smartclock::client::Daemon;
use smartclock::client::Daemons;
use smartclock::defaults::RUN_DIR;
use smartclock::defaults::SENSOR_SOCKET;
use smartclock::error::Error;
use smartclock::task::Cadence;
use smartclock_http::Response;

use crate::metrics::Scrape;
use crate::metrics::SensorScrape;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// Where the daemons' sockets are: one instance per subdirectory,
    /// `<run-dir>/<instance>/socket`.  Every one is scraped.
    #[arg(
        long,
        env = "SMARTCLOCK_EXPORTER_RUN_DIR",
        default_value = RUN_DIR
    )]
    run_dir: PathBuf,

    /// The daemons to ask, in place of the directory: each
    /// `[NAME=]ENDPOINT`, the endpoint its socket or `tcp://HOST:PORT`.
    /// Repeated, or comma-separated.  An unnamed one goes by its
    /// endpoint.
    #[arg(long, env = "SMARTCLOCK_EXPORTER_DAEMON", value_delimiter = ',')]
    daemon: Vec<String>,

    /// Address to serve /metrics on.
    ///
    /// Localhost by default.  The readings say where a receiver is to
    /// within a few meters, so exposing them further is a decision to
    /// take deliberately rather than by accepting a default.
    #[arg(
        long,
        env = "SMARTCLOCK_EXPORTER_LISTEN",
        default_value = "127.0.0.1:9979"
    )]
    listen: String,

    /// The sensor service, its socket or `tcp://HOST:PORT`.  Its sensors
    /// are exported once it has answered; a host without one exports
    /// none.
    #[arg(long, env = "SMARTCLOCK_EXPORTER_SENSORD", default_value = SENSOR_SOCKET)]
    sensord: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let daemons = Daemons::from_names(&cli.daemon, &cli.run_dir);
    eprintln!(
        "smartclock-exporter: serving http://{}/metrics from {}",
        cli.listen,
        match &daemons {
            Daemons::Listed(listed) => listed
                .iter()
                .map(|instance| instance.endpoint.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            Daemons::Dir(dir) => format!("every daemon under {}", dir.display()),
        }
    );
    let seen = Mutex::new(BTreeSet::new());
    let sensord_seen = AtomicBool::new(false);
    let sensor_socket = cli.sensord.clone();
    smartclock_http::serve(&cli.listen, move |path| match path {
        "/metrics" => {
            let (scrapes, sensors) = thread::scope(|scope| {
                let sensors = scope.spawn(|| scrape_sensors(&sensor_socket, &sensord_seen));
                let scrapes = scrape_all(&daemons, &seen);
                (
                    scrapes,
                    sensors.join().unwrap_or(SensorScrape {
                        seen: true,
                        latest: None,
                    }),
                )
            });
            Response::ok(
                "text/plain; version=0.0.4; charset=utf-8",
                metrics::render(&scrapes) + &metrics::render_sensors(&sensors),
            )
        }
        "/" => Response::ok(
            "text/plain; charset=utf-8",
            "smartclock-exporter\n\nMetrics are at /metrics.\n".to_owned(),
        ),
        _ => Response::not_found(),
    })?;
    Ok(())
}

/// How long one daemon may take to answer a scrape.
///
/// Every daemon is asked at once, so this is about how long a whole
/// scrape takes when one is wedged.  Well under Prometheus' default
/// ten-second scrape timeout, past which the scrape is abandoned and
/// every receiver loses every series, `up` included -- not only the
/// daemon that was slow.
const SCRAPE_BUDGET: Duration = Duration::from_secs(5);

/// Ask every daemon, once each and all at once.
///
/// A socket nobody answers on is reported as `up 0` under its name
/// rather than left out, and so is an instance `seen` before whose
/// socket has since gone.  systemd removes an instance's directory when
/// it stops, so without remembering it a stopped daemon's series would
/// simply end, and an alert on `up == 0` would never fire.  Remembered
/// until the exporter restarts, which is how a unit retired on purpose
/// is forgotten.
fn scrape_all(daemons: &Daemons, seen: &Mutex<BTreeSet<String>>) -> Vec<Scrape> {
    let current = daemons.instances();
    let mut scrapes: Vec<(String, Scrape)> = thread::scope(|scope| {
        let asked: Vec<_> = current
            .iter()
            .map(|instance| scope.spawn(|| scrape(&instance.name, &instance.endpoint)))
            .collect();
        current
            .iter()
            .zip(asked)
            .map(|(instance, asked)| {
                let scrape = asked.join().unwrap_or_else(|_| unanswered(&instance.name));
                (instance.name.clone(), scrape)
            })
            .collect()
    });
    let mut seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    seen.extend(
        current
            .iter()
            .map(|instance| instance.name.clone())
            .filter(|name| !name.is_empty()),
    );
    for name in seen.iter() {
        if !current.iter().any(|instance| instance.name == *name) {
            scrapes.push((name.clone(), unanswered(name)));
        }
    }
    scrapes.sort_by(|a, b| a.0.cmp(&b.0));
    scrapes.into_iter().map(|(_, scrape)| scrape).collect()
}

/// The sensor service's latest readings, if it answers within the
/// scrape's budget, and whether it ever has.
fn scrape_sensors(socket: &Path, seen: &AtomicBool) -> SensorScrape {
    let latest =
        Daemon::connect_within(socket, SCRAPE_BUDGET).and_then(|mut d| d.sensor_readings());
    match latest {
        Ok(latest) => {
            seen.store(true, Ordering::Relaxed);
            SensorScrape {
                seen: true,
                latest: Some(latest),
            }
        }
        Err(e) => {
            let seen = seen.load(Ordering::Relaxed);
            if seen {
                eprintln!("smartclock-exporter: {}: {e}", socket.display());
            }
            SensorScrape { seen, latest: None }
        }
    }
}

/// The labels naming one daemon instance, before anything is known of
/// the receiver it is attached to.
fn instance_labels(instance: &str) -> String {
    if instance.is_empty() {
        String::new()
    } else {
        format!("daemon=\"{}\"", escape(instance))
    }
}

/// A daemon that did not answer: `up 0` and nothing else.
fn unanswered(instance: &str) -> Scrape {
    Scrape {
        labels: instance_labels(instance),
        reading: None,
        cadence: Cadence::default(),
    }
}

/// One daemon's last reading, labeled by who it is.
///
/// A daemon that cannot be asked is reported as `up 0` with no
/// readings rather than as an HTTP error: Prometheus records the
/// former as a fact about the receiver and the latter as a fault in
/// the scrape, and the receiver being unreachable is the fact.  The
/// identity comes from the same connection, so a swap on the port
/// relabels the next scrape.
fn scrape(instance: &str, socket: &Path) -> Scrape {
    let mut labels = instance_labels(instance);
    let asked = Daemon::connect_within(socket, SCRAPE_BUDGET).and_then(|mut d| {
        let info = d.info()?;
        let reading = d.latest()?;
        // Asked again, because the reading does not say which receiver
        // it is of: one swapped in between the two answers would have
        // its reading filed under the unit before it.  Rare enough that
        // skipping this one scrape costs nothing.
        if client::identity(&d.info()?) != client::identity(&info) {
            return Err(Error::Daemon(
                "the receiver changed during the scrape".to_owned(),
            ));
        }
        Ok((info, reading))
    });
    match asked {
        Ok((info, reading)) => {
            if let Some(id) = client::identity(&info) {
                for (key, value) in [("serial", &id.serial), ("model", &id.model)] {
                    if !labels.is_empty() {
                        labels.push(',');
                    }
                    let _ = write!(labels, "{key}=\"{}\"", escape(value));
                }
            }
            Scrape {
                labels,
                reading: Some(reading),
                cadence: client::cadence(&info),
            }
        }
        Err(e) => {
            eprintln!("smartclock-exporter: {}: {e}", socket.display());
            unanswered(instance)
        }
    }
}

/// A label value as the exposition format wants it quoted.
pub(crate) fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::SCRAPE_BUDGET;
    use super::scrape_all;
    use crate::metrics::render;
    use smartclock::client::Daemons;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Instant;

    /// A run directory of our own, removed on drop.
    struct RunDir(PathBuf);

    impl RunDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("smartclock-exporter-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a run directory");
            Self(dir)
        }

        /// An instance whose daemon accepts and then never answers.
        fn wedged(&self, name: &str) {
            let socket = self.0.join(name).join("socket");
            std::fs::create_dir_all(socket.parent().expect("a parent")).expect("instance");
            let listener = smartclock::server::listen(&socket).expect("listen");
            std::thread::spawn(move || {
                let held = listener.accept();
                std::thread::sleep(SCRAPE_BUDGET * 3);
                drop(held);
            });
        }
    }

    impl Drop for RunDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_stopped_daemon_stays_down_rather_than_disappearing() {
        let run = RunDir::new("stopped");
        // A socket file nobody listens on: up 0 while it is there.
        std::fs::create_dir_all(run.0.join("bench")).expect("instance");
        std::fs::write(run.0.join("bench").join("socket"), b"").expect("socket");
        let daemons = Daemons::Dir(run.0.clone());
        let seen = Mutex::new(BTreeSet::new());
        assert!(
            render(&scrape_all(&daemons, &seen)).contains(r#"smartclock_up{daemon="bench"} 0"#)
        );
        // systemd removes the directory when the instance stops.
        std::fs::remove_dir_all(run.0.join("bench")).expect("stop it");
        let out = render(&scrape_all(&daemons, &seen));
        assert!(out.contains(r#"smartclock_up{daemon="bench"} 0"#), "{out}");
    }

    #[test]
    fn a_listed_daemon_is_labeled_by_its_name() {
        let daemons = Daemons::from_names(
            &["bench=tcp://127.0.0.1:1".to_owned()],
            std::path::Path::new("/nowhere"),
        );
        let out = render(&scrape_all(&daemons, &Mutex::new(BTreeSet::new())));
        assert!(out.contains(r#"smartclock_up{daemon="bench"} 0"#), "{out}");
    }

    #[test]
    fn wedged_daemons_are_asked_together_and_given_up_on() {
        let run = RunDir::new("wedged");
        run.wedged("one");
        run.wedged("two");
        let started = Instant::now();
        let scrapes = scrape_all(&Daemons::Dir(run.0.clone()), &Mutex::new(BTreeSet::new()));
        let took = started.elapsed();
        assert_eq!(scrapes.len(), 2);
        assert!(scrapes.iter().all(|s| s.reading.is_none()));
        assert!(
            took < SCRAPE_BUDGET * 3 / 2,
            "two wedged daemons took {took:?}"
        );
    }
}
