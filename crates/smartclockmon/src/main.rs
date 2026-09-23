//! Terminal monitor for a SmartClock receiver.
//!
//! Normally a client of `smartclockd`, which owns the serial port and
//! records history.  Direct mode is for before the daemon is installed;
//! it records nothing, and the header says so.

mod app;
mod history;
mod source;
mod ui;

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;

use crate::app::App;
use crate::app::View;
use crate::source::Update;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
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
}

/// How often to redraw when nothing has arrived, so the clock in the
/// header does not look frozen.
const TICK: Duration = Duration::from_millis(500);

/// How often to re-read the log.  Querying a week of rows every frame
/// would be wasteful, and the graphs do not move that fast.
const HISTORY_REFRESH: Duration = Duration::from_secs(5);

/// How often the sky view re-reads the status screen.
///
/// Deliberately slower than everything else: one read costs the
/// receiver about 1.5 s of a 19200 link, four fast polls, and the sky
/// does not move appreciably in fifteen seconds.
const SKY_REFRESH: Duration = Duration::from_secs(15);

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (updates, attachment, console, policy, cadence) = match &cli.device {
        Some(device) => source::from_device(device, cli.baud)?,
        None => source::from_daemon(&cli.socket)?,
    };

    let mut app = App::new(attachment, console, policy, cadence);
    app.open_log();

    let mut terminal = ratatui::init();
    let outcome = run(&mut terminal, app, &updates);
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
    let mut due = Instant::now();
    while !app.quitting {
        if app.view == View::Journal && Instant::now() >= due {
            app.refresh_journal();
            due = Instant::now() + HISTORY_REFRESH;
        }
        if app.view == View::History && Instant::now() >= due {
            let columns = terminal.size().map_or(80, |s| usize::from(s.width));
            app.refresh_history(columns);
            due = Instant::now() + HISTORY_REFRESH;
        }
        // Nothing polls the status screen, so the sky is only as fresh
        // as this view asks for it -- and it asks only while it is the
        // view being shown, because each read costs the receiver about
        // 1.5 s of its serial link.
        if app.view == View::Stability && Instant::now() >= due {
            app.refresh_deviation();
            due = Instant::now() + HISTORY_REFRESH;
        }
        if app.view == View::Sky && Instant::now() >= due {
            let _ = app.console.sky();
            due = Instant::now() + SKY_REFRESH;
        }
        terminal.draw(|frame| ui::draw(frame, &app))?;

        // Wait for a snapshot, but wake often enough to notice a
        // keypress and to redraw.
        match updates.recv_timeout(TICK) {
            Ok(Update::Reading(snapshot)) => app.accept(*snapshot),
            Ok(Update::Reply(answer)) => app.console_reply = Some(answer),
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
                // While the console has focus every printable key is
                // input, or there would be no way to type a command
                // containing q, g or w.
                if app.console_open {
                    match key.code {
                        KeyCode::Esc => {
                            app.console_open = false;
                            app.console_input.clear();
                        }
                        KeyCode::Enter => app.submit(),
                        KeyCode::Backspace => {
                            app.console_input.pop();
                        }
                        KeyCode::Char(c) => app.console_input.push(c),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.quitting = true,
                    KeyCode::Char('g') | KeyCode::Tab => {
                        app.view = match app.view {
                            View::Dashboard => View::History,
                            View::History => View::Journal,
                            View::Journal => View::Sky,
                            View::Sky => View::Stability,
                            View::Stability => View::Dashboard,
                        };
                        due = Instant::now();
                    }
                    KeyCode::Char('l') => {
                        app.view = match app.view {
                            View::Journal => View::Dashboard,
                            _ => View::Journal,
                        };
                        due = Instant::now();
                    }
                    KeyCode::Char('w') => {
                        app.window = app.window.next();
                        due = Instant::now();
                    }
                    KeyCode::Char('r') => {
                        app.next_receiver();
                        due = Instant::now();
                    }
                    KeyCode::Char('c') | KeyCode::Char(':') => app.console_open = true,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
