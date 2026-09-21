//! A simulated receiver on a TCP port.
//!
//! For pointing the real daemon at something that is not hardware.
//! TCP rather than a pseudo-terminal because a PTY would confine this
//! to Unix, and the only Linux-specific part of the project is meant to
//! be the systemd unit.
//!
//! Tests do not need this: they use `SimTransport` in process.

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::thread;

use smartclock_sim::receiver::Receiver;
use smartclock_sim::transport::SimTransport;

fn main() -> std::io::Result<()> {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5025".to_owned());
    let faulty = std::env::args().any(|a| a == "--faulty");
    // Errors the receiver is to have raised by itself, as
    // `--queue-error -313,"Calibration memory lost"`.  Seeded per
    // client, since each client gets its own receiver.
    let queued: Vec<(i32, String)> = std::env::args()
        .skip_while(|a| a != "--queue-error")
        .skip(1)
        .take(1)
        .filter_map(|a| parse_error(&a))
        .collect();

    let listener = TcpListener::bind(&address)?;
    eprintln!(
        "smartclock-sim: a{} receiver on {address}",
        if faulty { " faulty" } else { "" }
    );
    eprintln!("smartclock-sim: smartclockd --device tcp://{address}");

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("smartclock-sim: rejected a connection: {e}");
                continue;
            }
        };
        // Each client gets its own receiver, so one test cannot see
        // another's holdover.
        let mut receiver = if faulty {
            Receiver::faulty()
        } else {
            Receiver::default()
        };
        for (code, message) in &queued {
            receiver.queue_error(*code, message);
        }
        thread::spawn(move || {
            if let Err(e) = serve(stream, SimTransport::new(receiver)) {
                eprintln!("smartclock-sim: client ended: {e}");
            }
        });
    }
    Ok(())
}

/// `-313,Calibration memory lost` as a code and a message.
fn parse_error(argument: &str) -> Option<(i32, String)> {
    let (code, message) = argument.split_once(',')?;
    Some((code.trim().parse().ok()?, message.trim().to_owned()))
}

/// Shuttle bytes between the socket and the simulated receiver.
fn serve(mut stream: TcpStream, mut sim: SimTransport) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(20)))?;
    let mut from_client = [0u8; 512];
    let mut from_receiver = [0u8; 512];
    loop {
        match stream.read(&mut from_client) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                sim.write_all(&from_client[..n])?;
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => return Err(e),
        }
        let n = sim.read(&mut from_receiver)?;
        if n > 0 {
            stream.write_all(&from_receiver[..n])?;
            stream.flush()?;
        }
    }
}
