//! A simulated receiver on a TCP port.
//!
//! For pointing the real daemon at something that is not hardware.
//! TCP rather than a pseudo-terminal because a PTY would confine this
//! to Unix, and the only Linux-specific part of the project is meant to
//! be the systemd unit.
//!
//! Most tests do not need this: they use `SimTransport` in process.

use std::net::TcpListener;
use std::thread;

use clap::Parser;
use clap::ValueEnum;
use smartclock_sim::net::serve;
use smartclock_sim::receiver::Receiver;
use smartclock_sim::transport::SimTransport;

/// Serve a simulated SmartClock receiver over TCP.  Each client gets a
/// receiver of its own, so one cannot see another's holdover.
#[derive(Debug, Parser)]
#[command(version = smartclock::VERSION)]
struct Cli {
    /// Where to listen.
    #[arg(default_value = "127.0.0.1:5025")]
    address: String,
    /// Which receiver to be.
    #[arg(long, value_enum, default_value_t = Model::Hp58503a)]
    model: Model,
    /// Report a fault: EFC near full scale, in holdover.
    #[arg(long)]
    faulty: bool,
    /// Send nothing back of what is received, as the bench Z3801A.
    #[arg(long)]
    no_echo: bool,
    /// An error the receiver has raised by itself, as `-313,Calibration
    /// memory lost`.  May be given more than once.
    #[arg(long, value_parser = error_entry, allow_hyphen_values = true)]
    queue_error: Vec<(i32, String)>,
    /// An event already latched, as `questionable,1` -- the bit that says
    /// the receiver stepped its own clock.  May be given more than once.
    #[arg(long, value_parser = event)]
    raise_event: Vec<(String, u16)>,
}

/// The receivers the simulator can be.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Model {
    /// An HP 58503A.
    #[value(name = "58503a")]
    Hp58503a,
    /// An HP Z3801A.
    #[value(name = "z3801a")]
    Z3801a,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let listener = TcpListener::bind(&cli.address)?;
    eprintln!(
        "smartclock-sim: a{} {:?} receiver{} on {}",
        if cli.faulty { " faulty" } else { "" },
        cli.model,
        if cli.no_echo { " without echo" } else { "" },
        cli.address
    );
    eprintln!("smartclock-sim: smartclockd --device tcp://{}", cli.address);

    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("smartclock-sim: rejected a connection: {e}");
                continue;
            }
        };
        let receiver = if cli.faulty {
            Receiver::faulty()
        } else {
            Receiver::default()
        };
        let mut receiver = match cli.model {
            Model::Hp58503a => receiver,
            Model::Z3801a => receiver.as_z3801a(),
        };
        for (code, message) in &cli.queue_error {
            receiver.queue_error(*code, message);
        }
        for (register, bits) in &cli.raise_event {
            receiver.raise_event(register, *bits);
        }
        let transport = SimTransport::new(receiver).with_echo(!cli.no_echo);
        thread::spawn(move || {
            if let Err(e) = serve(stream, transport) {
                eprintln!("smartclock-sim: client ended: {e}");
            }
        });
    }
    Ok(())
}

/// `-313,Calibration memory lost` as a code and a message.
fn error_entry(argument: &str) -> Result<(i32, String), String> {
    let (code, message) = argument.split_once(',').ok_or("expected CODE,MESSAGE")?;
    let code = code
        .trim()
        .parse()
        .map_err(|e| format!("{code:?} is not an error code: {e}"))?;
    Ok((code, message.trim().to_owned()))
}

/// `questionable,1` as a register and its bits.
fn event(argument: &str) -> Result<(String, u16), String> {
    let (register, bits) = argument.split_once(',').ok_or("expected REGISTER,BITS")?;
    let bits = bits
        .trim()
        .parse()
        .map_err(|e| format!("{bits:?} is not a bit mask: {e}"))?;
    Ok((register.trim().to_owned(), bits))
}
