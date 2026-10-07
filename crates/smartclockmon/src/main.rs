//! Terminal monitor for a SmartClock receiver.
//!
//! Normally a client of `smartclockd`, which owns the serial port and
//! records history.  Direct mode is for before the daemon is installed;
//! it records nothing, and the header says so.

mod app;
mod history;
mod source;
mod ui;

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use clap::Parser;
use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;

use crate::app::App;
use smartclock::client::Daemon;
use smartclock::sensors::DEFAULT_SOCKET;
use smartclock::types::Framing;

use crate::app::View;
use crate::source::Update;

#[derive(Parser)]
#[command(about, version = smartclock::VERSION)]
struct Cli {
    /// The daemon's socket: one instance's
    /// `/run/smartclockd/<instance>/socket`.
    #[arg(long, required_unless_present = "device")]
    socket: Option<String>,

    /// Talk to the receiver directly instead of to the daemon.  Needs
    /// the daemon stopped, since it holds the port, and records no
    /// history.
    #[arg(long, conflicts_with = "socket")]
    device: Option<String>,

    /// Bits per second, for direct mode.
    #[arg(long, default_value_t = 19200)]
    baud: u32,

    /// Character framing for direct mode, `8N1` or `7O1`.  The Z3801A's
    /// is fixed at 7O1.
    #[arg(long, default_value = "8N1")]
    framing: Framing,

    /// The sensor service's socket, for the host's sensors in the
    /// header.  A host without the service shows none.
    #[arg(long, default_value = DEFAULT_SOCKET)]
    sensor_socket: PathBuf,
}

/// How often to redraw when nothing has arrived, so the clock in the
/// header does not look frozen.
const TICK: Duration = Duration::from_millis(500);

/// How long one wait on the keyboard lasts before the snapshots are
/// looked at: the most a key or a snapshot waits for the other.
const INPUT_POLL: Duration = Duration::from_millis(50);

/// How often to re-read the log.  Querying a week of rows every frame
/// would be wasteful, and the graphs do not move that fast.
const HISTORY_REFRESH: Duration = Duration::from_secs(5);

/// How often the status view re-reads the status screen.
///
/// Deliberately slower than everything else: one read costs the
/// receiver about 1.5 s of a 19200 link, four fast polls, and the sky
/// does not move appreciably in fifteen seconds.
const STATUS_REFRESH: Duration = Duration::from_secs(15);

/// How often the host's sensors are asked for: they are read every ten
/// seconds by default, so oftener shows nothing new.
const SENSORS_REFRESH: Duration = Duration::from_secs(5);

/// How long the sensor service may take to answer.  Short, since it is
/// asked from the drawing loop; a service that does not answer in time
/// leaves the header without sensors until the next ask.
const SENSORS_BUDGET: Duration = Duration::from_millis(250);

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (updates, attachment, console, policy, cadence) = match (&cli.device, &cli.socket) {
        (Some(device), _) => source::from_device(device, cli.baud, cli.framing)?,
        (None, Some(socket)) => source::from_daemon(socket)?,
        // clap requires one of the two.
        (None, None) => unreachable!("--socket is required without --device"),
    };

    let mut app = App::new(attachment, console, policy, cadence);
    app.open_log();

    let mut terminal = ratatui::init();
    let outcome = run(&mut terminal, app, &updates, &cli.sensor_socket);
    // Restore the terminal whatever happened, or a failure leaves the
    // operator with no echo and no cursor.
    ratatui::restore();
    outcome
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    mut app: App,
    updates: &std::sync::mpsc::Receiver<crate::source::Update>,
    sensor_socket: &Path,
) -> Result<()> {
    let mut due = Instant::now();
    let mut sensors_due = Instant::now();
    let mut last_draw = Instant::now();
    let mut dirty = true;
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
        // No tier polls the status screen, and the daemon reads it for
        // its log only every few minutes, so the status view asks for
        // its own -- only while it is the view being shown, because
        // each read costs the receiver about 1.5 s of its serial link.
        if app.view == View::Stability && Instant::now() >= due {
            app.refresh_deviation();
            due = Instant::now() + HISTORY_REFRESH;
        }
        if app.view == View::Status && Instant::now() >= due {
            let _ = app.console.status();
            due = Instant::now() + STATUS_REFRESH;
        }
        if Instant::now() >= sensors_due {
            app.sensors = Daemon::connect_within(sensor_socket, SENSORS_BUDGET)
                .and_then(|mut service| service.sensor_readings())
                .ok();
            sensors_due = Instant::now() + SENSORS_REFRESH;
            dirty = true;
        }
        if dirty || last_draw.elapsed() >= TICK {
            terminal.draw(|frame| ui::draw(frame, &app))?;
            last_draw = Instant::now();
            dirty = false;
        }

        // Wait on the keyboard in short slices rather than on the
        // snapshots, which would leave a key unanswered until the next
        // snapshot or tick.  One wait per frame, then only what is
        // already queued: a key held down repeats faster than the wait
        // times out, and waiting again would never reach the drawing.
        let mut waiting = INPUT_POLL;
        while event::poll(waiting)? {
            waiting = Duration::ZERO;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                press(&mut app, key, &mut due);
                dirty = true;
            }
        }

        // Then everything that arrived meanwhile, drawn once.
        loop {
            match updates.try_recv() {
                Ok(update) => {
                    apply(&mut app, update);
                    dirty = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.lost("the source thread stopped".to_owned());
                    terminal.draw(|frame| ui::draw(frame, &app))?;
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

/// Take one update from the source.
fn apply(app: &mut App, update: Update) {
    match update {
        Update::Reading(snapshot) => app.accept(*snapshot),
        Update::Reply(answer) => app.console_reply = Some(answer),
        // The daemon restarts under systemd, so losing it is not a
        // reason to quit: say so and keep waiting for it to return.
        Update::Lost(why) => app.lost(why),
        Update::Reattached {
            database,
            policy,
            cadence,
        } => app.reattached(database, policy, cadence),
    }
}

/// Act on one key.  `due` is when the open view next re-reads, and is
/// brought forward by anything that changes what it shows.
fn press(app: &mut App, key: KeyEvent, due: &mut Instant) {
    // Before the console's own handling, which would otherwise type
    // the `c` of a Ctrl-C into the command line.  In raw mode Ctrl-C
    // raises no signal, so this is the only way it quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'd'))
    {
        app.quitting = true;
        return;
    }
    // While the console has focus every printable key is input, or
    // there would be no way to type a command containing q, g or w.
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
        return;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quitting = true,
        KeyCode::Char('g') | KeyCode::Tab => {
            app.view = match app.view {
                View::Dashboard => View::History,
                View::History => View::Journal,
                View::Journal => View::Status,
                View::Status => View::Stability,
                View::Stability => View::Dashboard,
            };
            *due = Instant::now();
        }
        KeyCode::Char('l') => {
            app.view = match app.view {
                View::Journal => View::Dashboard,
                _ => View::Journal,
            };
            *due = Instant::now();
        }
        KeyCode::Char('w') => {
            app.window = app.window.next();
            *due = Instant::now();
        }
        KeyCode::Char('r') => {
            app.next_receiver();
            *due = Instant::now();
        }
        KeyCode::Char('c') | KeyCode::Char(':') => app.console_open = true,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::press;
    use crate::app::App;
    use crate::source::Attachment;
    use crate::source::Console;
    use crate::source::Policy;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use smartclock::task::Cadence;
    use std::time::Instant;

    #[test]
    fn control_c_quits_even_from_the_console() {
        let mut app = App::new(
            Attachment::Direct {
                device: "/dev/null".to_owned(),
            },
            Console::default(),
            Policy::default(),
            Cadence::default(),
        );
        app.console_open = true;
        let mut due = Instant::now();
        press(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut due,
        );
        assert!(app.quitting);
        assert!(app.console_input.is_empty(), "the c was typed");
    }
}
