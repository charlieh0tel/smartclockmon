//! Prometheus exporter for a SmartClock receiver.
//!
//! Asks the daemon on each scrape rather than holding a subscription,
//! and asks for its last reading rather than a fresh one, so scraping
//! costs the receiver nothing and cannot compete with the poll
//! schedule.  Prometheus decides how often that happens; the daemon
//! decides how often the receiver is read.
//!
//! No async runtime and no HTTP crate.  A scrape is one short request
//! answered from a value already in hand, and the rest of this
//! workspace is blocking and synchronous.

mod metrics;

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use smartclock::client::Daemon;
use smartclock::wire::Reading;
use smartclock_http::Response;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// The daemon's socket.
    #[arg(
        long,
        env = "SMARTCLOCK_EXPORTER_SOCKET",
        default_value = "/run/smartclockd/socket"
    )]
    socket: PathBuf,

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
    eprintln!(
        "smartclock-exporter: serving http://{}/metrics from {}",
        cli.listen,
        cli.socket.display()
    );
    let socket = cli.socket.clone();
    smartclock_http::serve(&cli.listen, move |path| match path {
        "/metrics" => Response::ok(
            "text/plain; version=0.0.4; charset=utf-8",
            metrics::render(latest_reading(&socket).as_ref()),
        ),
        "/" => Response::ok(
            "text/plain; charset=utf-8",
            "smartclock-exporter\n\nMetrics are at /metrics.\n".to_owned(),
        ),
        _ => Response::not_found(),
    })
}

/// The daemon's last reading, or nothing if it could not be asked.
///
/// A scrape that cannot reach the daemon is reported as `up 0` with no
/// readings rather than as an HTTP error: Prometheus records the former
/// as a fact about the receiver and the latter as a fault in the
/// scrape, and the receiver being unreachable is the fact.
fn latest_reading(socket: &Path) -> Option<Reading> {
    match Daemon::connect(socket).and_then(|mut d| d.latest()) {
        Ok(reading) => Some(reading),
        Err(e) => {
            eprintln!("smartclock-exporter: {e}");
            None
        }
    }
}
