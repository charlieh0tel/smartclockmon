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
            Constraint::Length(8),
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
    let mode = match s.mode {
        Some(SmartClockMode::Locked) => field(
            "mode",
            "Locked to GPS".to_owned(),
            Style::new().fg(Color::Green),
        ),
        Some(SmartClockMode::Holdover) => field(
            "mode",
            "Holdover".to_owned(),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Some(other) => field("mode", format!("{other:?}"), Style::new().fg(Color::Yellow)),
        None => absent("mode"),
    };
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

    if let Some(screen) = s.screen.as_ref()
        && let Some(mode) = &screen.position_mode
    {
        lines.push(plain("survey", mode.clone()));
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
