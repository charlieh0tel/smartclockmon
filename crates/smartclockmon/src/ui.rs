//! Drawing the monitor.
//!
//! Laid out for an 80 by 24 terminal and growing from there, since that
//! is what a serial console on the bench is likely to be.

use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Cell;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Row;
use ratatui::widgets::Table;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Snapshot;
use smartclock::types::SmartClockMode;

use crate::app::App;

/// Block elements for the trend, lightest first.
const TREND_BLOCKS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// The same ramp for a terminal that cannot draw blocks.
const TREND_ASCII: [char; 8] = [' ', '.', ':', '-', '=', '+', '*', '#'];

/// Width of the label column, so values line up across panes.
const LABEL_WIDTH: usize = 12;

/// Draw the whole monitor.
pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(11),
            Constraint::Min(6),
            Constraint::Length(1),
        ])
        .split(frame.area());

    header(frame, rows[0], app);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    lock(frame, middle[0], app);
    oscillator(frame, middle[1], app);

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[2]);
    satellites(frame, lower[0], app);
    time_and_place(frame, lower[1], app);

    footer(frame, rows[3], app);
}

fn block(app: &App, title: &str) -> Block<'static> {
    let set = if app.unicode {
        border::ROUNDED
    } else {
        // A serial console on the bench may not render box drawing.
        border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        }
    };
    Block::bordered()
        .border_set(set)
        .title(format!(" {title} "))
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    let (state, style) = match app.snapshot.as_ref().map(|s| s.freshness) {
        Some(Freshness::Live) => ("LIVE", Style::new().fg(Color::Green)),
        Some(Freshness::Stale) => ("STALE", Style::new().fg(Color::Yellow)),
        Some(Freshness::Disconnected) => (
            "DISCONNECTED",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        None => ("WAITING", Style::new().fg(Color::DarkGray)),
    };
    let line = Line::from(vec![
        Span::styled(format!("[{state}]"), style),
        Span::raw("  "),
        Span::raw(app.attachment.describe()),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(block(app, "smartclockmon")),
        area,
    );
}

/// Pair a label with a value, padded so the columns line up.
fn field<'a>(label: &'a str, value: String, style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(value, style),
    ])
}

fn plain(label: &str, value: String) -> Line<'_> {
    field(label, value, Style::new())
}

fn absent(label: &str) -> Line<'_> {
    Line::from(vec![
        Span::styled(
            format!("{label:<LABEL_WIDTH$}"),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled("--", Style::new().fg(Color::DarkGray)),
    ])
}

fn lock(frame: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.snapshot.as_ref() else {
        frame.render_widget(
            Paragraph::new("waiting for a reading").block(block(app, "Lock")),
            area,
        );
        return;
    };
    let (mode_text, mode_style) = mode_line(s);
    let mode = field("mode", mode_text, mode_style);

    let lines = vec![
        mode,
        s.tfom
            .map_or_else(|| absent("TFOM"), |v| plain("TFOM", v.to_string())),
        s.ffom
            .map_or_else(|| absent("FFOM"), |v| plain("FFOM", v.to_string())),
        s.time_interval
            .map_or_else(|| absent("1 PPS TI"), |v| plain("1 PPS TI", v.to_string())),
        s.holdover_waiting
            .map_or_else(|| absent("waiting"), |w| plain("waiting", format!("{w:?}"))),
        s.holdover_predicted.map_or_else(
            || absent("24 h error"),
            |v| plain("24 h error", v.to_string()),
        ),
        match s.holdover_present {
            Some(v) => field("now off by", v.to_string(), Style::new().fg(Color::Yellow)),
            // Only meaningful in holdover, so its absence is normal.
            None => absent("now off by"),
        },
        s.screen
            .as_ref()
            .and_then(|sc| sc.hold_threshold.clone())
            .map_or_else(|| absent("hold thr"), |v| plain("hold thr", v)),
        s.holdover_duration.map_or_else(
            || absent("holdover"),
            |h| {
                let text = if h.active {
                    format!("active, {}", h.elapsed)
                } else {
                    format!("last {}", h.elapsed)
                };
                field(
                    "holdover",
                    text,
                    if h.active {
                        Style::new().fg(Color::Yellow)
                    } else {
                        Style::new()
                    },
                )
            },
        ),
    ];
    frame.render_widget(Paragraph::new(lines).block(block(app, "Lock")), area);
}

/// The mode line, and how to colour it.
///
/// The state comes from `:SYNChronization:STATe?`, which is on the
/// one-second tier, but that returns a bare `LOCK` with no detail.  The
/// detail -- "stabilizing frequency", "GPS acquisition", "GPS 1PPS
/// invalid" -- exists only on the status screen, which is polled every
/// ten seconds.
///
/// So the two are combined, and the suffix is used only while the
/// screen still agrees about the base state.  Otherwise a transition
/// would show the fresh state carrying ten seconds of stale
/// explanation, which is worse than no explanation.
fn mode_line(snapshot: &Snapshot) -> (String, Style) {
    let Some(mode) = snapshot.mode else {
        return ("--".to_owned(), Style::new().fg(Color::DarkGray));
    };
    let (base, screen_word, style) = match mode {
        SmartClockMode::Locked => ("Locked to GPS", "Locked", Style::new().fg(Color::Green)),
        SmartClockMode::Recovery => ("Recovery", "Recovery", Style::new().fg(Color::Yellow)),
        SmartClockMode::Holdover => (
            "Holdover",
            "Holdover",
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        SmartClockMode::Waiting => ("Waiting to recover", "Wait", Style::new().fg(Color::Yellow)),
        SmartClockMode::PowerUp => ("Power-up", "Power-up", Style::new().fg(Color::Cyan)),
        SmartClockMode::Other => ("Other", "Other", Style::new().fg(Color::Magenta)),
    };

    let detail = snapshot
        .screen
        .as_ref()
        .and_then(|s| s.mode.as_deref())
        .filter(|text| text.starts_with(screen_word))
        .and_then(|text| text.split_once(':'))
        .map(|(_, suffix)| suffix.trim().to_owned())
        .filter(|suffix| !suffix.is_empty());

    match detail {
        Some(detail) => (format!("{base}: {detail}"), style),
        None => (base.to_owned(), style),
    }
}

fn oscillator(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    match app.snapshot.as_ref().and_then(|s| s.efc) {
        Some(efc) => {
            let used = efc.range_used();
            // An ageing OCXO fails by walking to a rail, so how much of
            // the range is gone matters more than the signed value.
            let style = if used > 0.9 {
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if used > 0.75 {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::Green)
            };
            lines.push(field("EFC", efc.to_string(), style));
            lines.push(field(
                "range used",
                format!("{:.0}%  {}", used * 100.0, gauge(used, 18, app.unicode)),
                style,
            ));
        }
        None => lines.push(absent("EFC")),
    }

    if let Some((lo, hi)) = app.efc_range() {
        lines.push(plain("trend", format!("{lo:+.3} to {hi:+.3}%")));
        // Indented to sit under the value column, so it reads as part
        // of the trend field rather than as a stray row.
        let width = (area.width as usize).saturating_sub(LABEL_WIDTH + 4);
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(LABEL_WIDTH)),
            Span::styled(trend(app, width), Style::new().fg(Color::Cyan)),
        ]));
    } else {
        lines.push(absent("trend"));
    }

    if let Some(snap) = app.snapshot.as_ref() {
        // Temperature belongs next to EFC: an OCXO's control voltage
        // moves with it, so a drift reading means little on its own.
        match (snap.temperature, snap.oven_current) {
            (Some(t), Some(i)) => {
                lines.push(plain("temperature", format!("{t:.2} C    oven {i:.1}")));
            }
            (Some(t), None) => lines.push(plain("temperature", format!("{t:.2} C"))),
            _ => {}
        }
        if let Some(code) = snap.efc_dac {
            lines.push(plain("EFC code", format!("{code} of {}", 1u32 << 20)));
        }
    }

    match app.snapshot.as_ref().and_then(|s| s.hardware) {
        Some(h) if h.is_healthy() => {
            lines.push(field(
                "hardware",
                "no faults".to_owned(),
                Style::new().fg(Color::Green),
            ));
        }
        Some(h) => {
            for fault in h.faults() {
                lines.push(field(
                    "FAULT",
                    fault.describe().to_owned(),
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
        }
        None => lines.push(absent("hardware")),
    }

    // The receiver's own health monitor line, which covers the ovens
    // and supplies that the condition register does not spell out.
    if let Some(s) = app.snapshot.as_ref() {
        match health_faults(s) {
            Some(faults) if faults.is_empty() => {
                lines.push(field(
                    "self report",
                    "all OK".to_owned(),
                    Style::new().fg(Color::Green),
                ));
            }
            Some(faults) => {
                for fault in faults {
                    lines.push(field(
                        "REPORTED",
                        fault,
                        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ));
                }
            }
            None => {}
        }
    }

    frame.render_widget(Paragraph::new(lines).block(block(app, "Oscillator")), area);
}

/// A horizontal bar showing how much of the tuning range is used.
fn gauge(fraction: f64, width: usize, unicode: bool) -> String {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let (full, empty) = if unicode {
        ('\u{2588}', '\u{2591}')
    } else {
        ('#', '.')
    };
    let mut bar = String::with_capacity(width + 2);
    bar.push('[');
    for n in 0..width {
        bar.push(if n < filled { full } else { empty });
    }
    bar.push(']');
    bar
}

/// A sparkline of recent EFC, scaled to its own span.
///
/// Scaled to the window rather than to the full -100..100 range: the
/// movement worth seeing is thousandths of a percent, which would be a
/// flat line against the whole range.
fn trend(app: &App, width: usize) -> String {
    let Some((lo, hi)) = app.efc_range() else {
        return String::new();
    };
    let ramp = if app.unicode {
        TREND_BLOCKS
    } else {
        TREND_ASCII
    };
    let span = (hi - lo).max(f64::EPSILON);
    app.efc_trend
        .iter()
        .rev()
        .take(width)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|efc| {
            let level = ((efc.percent() - lo) / span * (ramp.len() - 1) as f64).round();
            ramp[(level as usize).min(ramp.len() - 1)]
        })
        .collect()
}

fn satellites(frame: &mut Frame, area: Rect, app: &App) {
    let screen = app.snapshot.as_ref().and_then(|s| s.screen.as_ref());
    let Some(screen) = screen else {
        frame.render_widget(
            Paragraph::new("waiting for a status screen").block(block(app, "Satellites")),
            area,
        );
        return;
    };

    let rows: Vec<Row> = screen
        .satellites
        .iter()
        .map(|sat| {
            let style = if sat.tracked {
                Style::new().fg(Color::Green)
            } else if sat.acquiring {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            let cell = |v: Option<String>| Cell::from(v.unwrap_or_else(|| "--".to_owned()));
            Row::new(vec![
                Cell::from(sat.prn.to_string()),
                cell(sat.elevation.map(|d| d.get().to_string())),
                cell(sat.azimuth.map(|d| d.get().to_string())),
                cell(sat.signal.map(|s| s.to_string())),
                Cell::from(if sat.tracked {
                    "tracked"
                } else if sat.acquiring {
                    "acquiring"
                } else {
                    "seen"
                }),
            ])
            .style(style)
        })
        .collect();

    let title = match (screen.tracking, screen.not_tracking) {
        (Some(t), Some(n)) => format!("Satellites  {t} tracked, {n} not"),
        _ => "Satellites".to_owned(),
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(9),
        ],
    )
    .header(
        Row::new(vec!["PRN", "El", "Az", "SS", "state"]).style(
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(block(app, &title));
    frame.render_widget(table, area);
}

fn time_and_place(frame: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.snapshot.as_ref() else {
        frame.render_widget(
            Paragraph::new("waiting for a reading").block(block(app, "Time and position")),
            area,
        );
        return;
    };
    let mut lines = Vec::new();

    match s.date {
        Some(date) => match date.rollover() {
            Some(slip) => {
                // The receiver's calendar is wrong by whole GPS epochs.
                // Its time of day and outputs are fine, so this is a
                // warning rather than an alarm, but it must not be
                // quietly corrected out of sight.
                lines.push(field(
                    "date",
                    format!("{}  WRONG", date.raw()),
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
                lines.push(field(
                    "actually",
                    format!("{}  ({} rollover)", date.corrected(), slip.epochs),
                    Style::new().fg(Color::Yellow),
                ));
            }
            None => lines.push(plain("date", date.raw().to_string())),
        },
        None => lines.push(absent("date")),
    }

    if let Some(screen) = s.screen.as_ref() {
        match (&screen.time, &screen.time_scale) {
            (Some(t), Some(scale)) => {
                let suspect = if screen.time_suspect { "  [?]" } else { "" };
                lines.push(plain("time", format!("{t} {scale}{suspect}")));
            }
            _ => lines.push(absent("time")),
        }
        if let Some(status) = &screen.sync_status {
            lines.push(plain("1 PPS", status.clone()));
        }
    }

    match s.position {
        Some(p) => {
            lines.push(plain("latitude", format!("{:+.6}", p.latitude)));
            lines.push(plain("longitude", format!("{:+.6}", p.longitude)));
            lines.push(plain("height", format!("{:+.2} m {}", p.height, p.datum)));
        }
        None => lines.push(absent("position")),
    }

    if let Some(screen) = s.screen.as_ref() {
        if let Some(mode) = &screen.position_mode {
            lines.push(plain("survey", mode.clone()));
        }
        if let Some(why) = &screen.survey_suspended {
            lines.push(field(
                "suspended",
                why.clone(),
                Style::new().fg(Color::Yellow),
            ));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(block(app, "Time and position")),
        area,
    );
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = vec![Span::styled(
        "q quit  u toggle ASCII mode",
        Style::new().fg(Color::DarkGray),
    )];
    if let Some(error) = app.snapshot.as_ref().and_then(|s| s.last_error.as_ref()) {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(error.clone(), Style::new().fg(Color::Red)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The health monitor line, reduced to what is wrong.
///
/// The full line runs to about seventy characters, which will not fit a
/// half-width pane, and reading six "OK"s to find the one that is not
/// is worse than being told directly.  `None` means nothing to report.
fn health_faults(snapshot: &Snapshot) -> Option<Vec<String>> {
    let items = &snapshot.screen.as_ref()?.health_items;
    if items.is_empty() {
        return None;
    }
    Some(
        items
            .iter()
            .filter(|(_, value)| !value.eq_ignore_ascii_case("OK"))
            .map(|(label, value)| format!("{label}: {value}"))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::mode_line;
    use jiff::Timestamp;
    use smartclock::screen;
    use smartclock::snapshot::Snapshot;
    use smartclock::types::SmartClockMode;

    fn snapshot(mode: Option<SmartClockMode>, screen_text: Option<&str>) -> Snapshot {
        let mut s = Snapshot::new(Timestamp::now());
        s.mode = mode;
        s.screen = screen_text.map(screen::parse);
        s
    }

    /// A screen whose mode line says `text`.
    ///
    /// The right-hand panel starts at column 46, as it does on the
    /// receiver.  The scraper cuts every line there, so a panel placed
    /// any closer would clip the mode text being tested.
    fn screen_with(text: &str) -> String {
        let pad = |left: &str, right: &str| format!("{left:<46}{right}\n");
        let mut out = String::from("---- Receiver Status ----\n");
        out.push_str("SYNCHRONIZATION ..................... [ Outputs Valid ]\n");
        out.push_str(&pad("SmartClock Mode", "Reference Outputs"));
        out.push_str(&pad(
            &format!(">> {text}"),
            "TFOM     3            FFOM     1",
        ));
        out.push_str("ACQUISITION ......................... [ GPS 1PPS Valid ]\n");
        out.push_str(&pad("Tracking: 1       Not Tracking: 0", "Time"));
        out.push_str(&pad(
            "PRN  El  Az   SS",
            "UTC      20:04:20     04 Feb 2007",
        ));
        out.push_str(&pad("  3  88 281  111", "ANT DLY  0 ns"));
        out.push_str("HEALTH MONITOR ...................... [ OK ]\n");
        out.push_str("Self Test: OK   GPS Rcv: OK\n");
        out
    }

    #[test]
    fn the_screens_detail_is_appended_to_the_fast_state() {
        // The receiver's own display says "Locked to GPS: stabilizing
        // frequency"; :SYNC:STATe? says only LOCK.
        let s = snapshot(
            Some(SmartClockMode::Locked),
            Some(&screen_with("Locked to GPS: stabilizing frequency")),
        );
        assert_eq!(mode_line(&s).0, "Locked to GPS: stabilizing frequency");
    }

    #[test]
    fn a_state_with_no_detail_reads_plainly() {
        let s = snapshot(
            Some(SmartClockMode::Locked),
            Some(&screen_with("Locked to GPS")),
        );
        assert_eq!(mode_line(&s).0, "Locked to GPS");
    }

    #[test]
    fn a_stale_detail_is_dropped_when_the_state_has_moved_on() {
        // The screen is on the ten second tier and the state on the one
        // second tier, so during a transition the screen still says
        // Locked while the receiver has already dropped to holdover.
        // Carrying the old explanation across would be worse than none.
        let s = snapshot(
            Some(SmartClockMode::Holdover),
            Some(&screen_with("Locked to GPS: stabilizing frequency")),
        );
        assert_eq!(mode_line(&s).0, "Holdover");
    }

    #[test]
    fn holdover_keeps_its_own_detail() {
        let s = snapshot(
            Some(SmartClockMode::Holdover),
            Some(&screen_with("Holdover: GPS 1PPS invalid")),
        );
        assert_eq!(mode_line(&s).0, "Holdover: GPS 1PPS invalid");
    }

    #[test]
    fn power_up_detail_survives_the_missing_space_after_the_colon() {
        // The firmware writes "Power-up:GPS acquisition", with no space.
        let s = snapshot(
            Some(SmartClockMode::PowerUp),
            Some(&screen_with("Power-up:GPS acquisition")),
        );
        assert_eq!(mode_line(&s).0, "Power-up: GPS acquisition");
    }

    #[test]
    fn without_a_screen_the_state_still_shows() {
        let s = snapshot(Some(SmartClockMode::Locked), None);
        assert_eq!(mode_line(&s).0, "Locked to GPS");
    }

    #[test]
    fn nothing_polled_yet_shows_as_absent() {
        assert_eq!(mode_line(&snapshot(None, None)).0, "--");
    }
}
