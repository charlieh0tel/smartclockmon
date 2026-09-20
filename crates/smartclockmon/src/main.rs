//! Terminal monitor for a SmartClock receiver.
//!
//! Normally a client of `smartclockd`, which owns the serial port and
//! records history.  Direct mode is for before the daemon is installed;
//! it records nothing, and the header says so.

mod app;
mod source;
mod ui;

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;

use crate::app::App;
use crate::source::Update;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// The daemon's socket.
    #[arg(long, default_value = "/run/smartclockd/socket")]
    socket: String,

    /// Talk to the receiver directly instead of to the daemon.  Needs
    /// the daemon stopped, since it holds the port, and records no
    /// history.
    #[arg(long, conflicts_with = "socket")]
    device: Option<String>,

    /// Bits per second, for direct mode.
    #[arg(long, default_value_t = 19200)]
    baud: u32,

    /// Draw with ASCII only, for a terminal that cannot render box
    /// drawing or block elements.
    #[arg(long)]
    ascii: bool,
}

/// How often to redraw when nothing has arrived, so the clock in the
/// header does not look frozen.
const TICK: Duration = Duration::from_millis(500);

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (updates, attachment) = match &cli.device {
        Some(device) => source::from_device(device, cli.baud)?,
        None => source::from_daemon(&cli.socket)?,
    };

    let mut terminal = ratatui::init();
    let outcome = run(&mut terminal, App::new(attachment, !cli.ascii), &updates);
    // Restore the terminal whatever happened, or a failure leaves the
    // operator with no echo and no cursor.
    ratatui::restore();
    outcome
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    updates: &std::sync::mpsc::Receiver<crate::source::Update>,
) -> Result<()> {
    while !app.quitting {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        // Wait for a snapshot, but wake often enough to notice a
        // keypress and to redraw.
        match updates.recv_timeout(TICK) {
            Ok(Update::Reading(snapshot)) => app.accept(*snapshot),
            // The daemon restarts under systemd, so losing it is not a
            // reason to quit: say so and keep waiting for it to return.
            Ok(Update::Lost(why)) => app.lost(why),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                app.lost("the source thread stopped".to_owned());
                terminal.draw(|frame| ui::draw(frame, &app))?;
                return Ok(());
            }
        }

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.quitting = true,
                    KeyCode::Char('u') => app.unicode = !app.unicode,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
