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

use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use smartclock::client::Daemon;
use smartclock_http::Response;

use crate::metrics::Scrape;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// Where the daemons' sockets are: one instance per subdirectory,
    /// `<run-dir>/<instance>/socket`.  Every one is scraped.
    #[arg(
        long,
        env = "SMARTCLOCK_EXPORTER_RUN_DIR",
        default_value = "/run/smartclockd"
    )]
    run_dir: PathBuf,

    /// One daemon's socket in place of the directory.
    #[arg(long, env = "SMARTCLOCK_EXPORTER_SOCKET")]
    socket: Option<PathBuf>,

    /// Address to serve /metrics on.
    ///
    /// Localhost by default.  The readings say where a receiver is to
    /// within a few metres, so exposing them further is a decision to
    /// take deliberately rather than by accepting a default.
    #[arg(
        long,
        env = "SMARTCLOCK_EXPORTER_LISTEN",
        default_value = "127.0.0.1:9979"
    )]
    listen: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let daemons = match cli.socket.clone() {
        Some(file) => Daemons::File(file),
        None => Daemons::Dir(cli.run_dir.clone()),
    };
    eprintln!(
        "smartclock-exporter: serving http://{}/metrics from {}",
        cli.listen,
        match &daemons {
            Daemons::File(file) => file.display().to_string(),
            Daemons::Dir(dir) => format!("every daemon under {}", dir.display()),
        }
    );
    smartclock_http::serve(&cli.listen, move |path| match path {
        "/metrics" => Response::ok(
            "text/plain; version=0.0.4; charset=utf-8",
            metrics::render(&daemons.scrape()),
        ),
        "/" => Response::ok(
            "text/plain; charset=utf-8",
            "smartclock-exporter\n\nMetrics are at /metrics.\n".to_owned(),
        ),
        _ => Response::not_found(),
    })
}

/// Where the daemons are.
enum Daemons {
    /// The run directory: one instance per subdirectory, its socket
    /// inside.
    Dir(PathBuf),
    /// One socket.
    File(PathBuf),
}

impl Daemons {
    /// Every socket that could be a daemon's, by instance name.
    ///
    /// An instance's directory outlives nothing: systemd removes it
    /// when the instance stops, so a socket that is there is one
    /// something should be answering on, and one nobody answers on is
    /// reported as `up 0` under its name rather than left out.
    fn sockets(&self) -> Vec<(String, PathBuf)> {
        match self {
            Self::File(file) => vec![(String::new(), file.clone())],
            Self::Dir(dir) => {
                let Ok(entries) = std::fs::read_dir(dir) else {
                    return Vec::new();
                };
                let mut sockets: Vec<(String, PathBuf)> = entries
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.path().join("socket").exists())
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.path().join("socket"),
                        )
                    })
                    .collect();
                sockets.sort();
                sockets
            }
        }
    }

    /// Ask every daemon, once each.
    fn scrape(&self) -> Vec<Scrape> {
        self.sockets()
            .into_iter()
            .map(|(instance, socket)| scrape(&instance, &socket))
            .collect()
    }
}

/// One daemon's last reading, labelled by who it is.
///
/// A daemon that cannot be asked is reported as `up 0` with no
/// readings rather than as an HTTP error: Prometheus records the
/// former as a fact about the receiver and the latter as a fault in
/// the scrape, and the receiver being unreachable is the fact.  The
/// identity comes from the same connection, so a swap on the port
/// relabels the next scrape.
fn scrape(instance: &str, socket: &Path) -> Scrape {
    let mut labels = String::new();
    if !instance.is_empty() {
        labels = format!("daemon=\"{}\"", escape(instance));
    }
    let asked = Daemon::connect(socket).and_then(|mut d| {
        let info = d.info()?;
        let reading = d.latest()?;
        Ok((info, reading))
    });
    match asked {
        Ok((info, reading)) => {
            let identity = info
                .get("identity")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if let Ok(id) = smartclock::parse::identity(identity) {
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
            }
        }
        Err(e) => {
            eprintln!("smartclock-exporter: {}: {e}", socket.display());
            Scrape {
                labels,
                reading: None,
            }
        }
    }
}

/// A label value as the exposition format wants it quoted.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
