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

use std::io::BufRead as _;
use std::io::BufReader;
use std::io::Write as _;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::thread;

use anyhow::Context as _;
use anyhow::Result;
use clap::Parser;
use smartclock::client::Daemon;

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
    let listener =
        TcpListener::bind(&cli.listen).with_context(|| format!("binding {}", cli.listen))?;
    eprintln!(
        "smartclock-exporter: serving http://{}/metrics from {}",
        cli.listen,
        cli.socket.display()
    );

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            // One client failing to connect is not a reason to stop
            // serving the others.
            Err(e) => {
                eprintln!("smartclock-exporter: rejected a connection: {e}");
                continue;
            }
        };
        let socket = cli.socket.clone();
        // A scrape that hangs must not block the next one: Prometheus
        // gives up after its own timeout and tries again, and a serial
        // link that has gone quiet can take seconds to say so.
        let spawned = thread::Builder::new()
            .name("smartclock-scrape".to_owned())
            .spawn(move || serve(&stream, &socket));
        if let Err(e) = spawned {
            eprintln!("smartclock-exporter: could not serve a scrape: {e}");
        }
    }
    Ok(())
}

/// Answer one HTTP request.
fn serve(stream: &TcpStream, socket: &std::path::Path) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // "GET /metrics HTTP/1.1".  Only the path is of interest; a scrape
    // sends no body and no header this needs.
    let path = line.split_whitespace().nth(1).unwrap_or("/");

    let (status, body) = match path {
        "/metrics" => ("200 OK", metrics::render(read(socket).as_ref())),
        "/" => (
            "200 OK",
            "smartclock-exporter\n\nMetrics are at /metrics.\n".to_owned(),
        ),
        _ => ("404 Not Found", "not found\n".to_owned()),
    };
    let mut writer = stream;
    let _ = write!(
        writer,
        "HTTP/1.1 {status}\r\n\
         Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The daemon's last reading, or nothing if it could not be asked.
///
/// A scrape that cannot reach the daemon is reported as `up 0` with no
/// readings rather than as an HTTP error: Prometheus records the former
/// as a fact about the receiver and the latter as a fault in the
/// scrape, and the receiver being unreachable is the fact.
fn read(socket: &std::path::Path) -> Option<smartclock::wire::Reading> {
    match Daemon::connect(socket).and_then(|mut d| d.latest()) {
        Ok(reading) => Some(reading),
        Err(e) => {
            eprintln!("smartclock-exporter: {e}");
            None
        }
    }
}
