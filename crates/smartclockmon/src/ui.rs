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
use ratatui::symbols::Marker;
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Axis;
use ratatui::widgets::Block;
use ratatui::widgets::Cell;
use ratatui::widgets::Chart;
use ratatui::widgets::Dataset;
use ratatui::widgets::GraphType;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Row;
use ratatui::widgets::Table;
use smartclock::screen::Screen;
use smartclock::snapshot::Freshness;
use smartclock::snapshot::Tier;
use smartclock::types::EfcPercent;
use smartclock::types::Seconds;
use smartclock::types::SmartClockMode;
use smartclock::wire::Reading;

use crate::app::App;
use crate::app::View;
use crate::history::Source;
use crate::history::Trace;
use crate::source::Attachment;

/// Block elements for the trend, lightest first.
const TREND_BLOCKS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// Width of the label column, so values line up across panes: the
/// longest label, `internal temp`, and a space.
const LABEL_WIDTH: usize = 14;

/// Rows the console takes while it is open.
const CONSOLE_ROWS: u16 = 4;

/// Width assumed for the status screen before one has been read: the
/// receivers' screens are 80 columns.
const SCREEN_COLUMNS: u16 = 80;

/// Width the satellite table needs beside the screen: its five columns,
/// their gaps and the border.
const SATELLITE_COLUMNS: u16 = 4 + 4 + 5 + 5 + 9 + 4 + 2;

/// Draw the whole monitor.
///
/// The console and the key line are drawn here rather than by each
/// view: the keys that open the console work in every view, and a view
/// that did not draw it left the operator typing into nothing.
pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let console_rows = if app.console_open { CONSOLE_ROWS } else { 0 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(console_rows),
            Constraint::Length(1),
        ])
        .split(frame.area());
    match app.view {
        View::Dashboard => dashboard(frame, rows[0], app),
        View::History => history(frame, rows[0], app),
        View::Journal => journal(frame, rows[0], app),
        View::Status => status(frame, rows[0], app),
        View::Stability => stability(frame, rows[0], app),
    }
    if app.console_open {
        console(frame, rows[1], app);
    }
    footer(frame, rows[2], app);
}

/// What the receiver has recorded about itself.
///
/// Three records shown as one list -- its diagnostic log, the
/// transitions taken from its event registers, and its error queue --
/// because the operator wants to know what the receiver has been
/// saying, not which mechanism said it.  None of this is in the
/// snapshot table and none of it can be plotted, so without a pane it
/// is visible only to somebody holding a SQL prompt.
fn journal(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Row> = app
        .journal
        .iter()
        .map(|line| {
            let colour = match line.source {
                Source::Log => Color::Gray,
                Source::Event => Color::Cyan,
                Source::Error => Color::Yellow,
                Source::Note => Color::Green,
            };
            Row::new(vec![
                Cell::from(stamp(&line.stamp)).style(Style::new().fg(Color::DarkGray)),
                Cell::from(line.source.tag()).style(Style::new().fg(colour)),
                Cell::from(line.text.clone()),
            ])
        })
        .collect();

    let title = if let Some(error) = &app.history_error {
        format!(" Journal -- {error} ")
    } else if app.journal.is_empty() {
        " Journal -- nothing recorded yet ".to_owned()
    } else {
        // Not "newest first": events and errors are, the receiver's
        // log follows in its own entry order, and claiming one
        // chronology across two clocks would be a lie.
        format!(
            " Journal -- {} lines: events and notes, then the receiver's log ",
            app.journal.len()
        )
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(STAMP_WIDTH as u16),
            Constraint::Length(5),
            Constraint::Min(20),
        ],
    )
    .block(Block::bordered().title(title));
    frame.render_widget(table, area);
}

/// As much of a timestamp as is worth a column.
///
/// Two formats arrive here and they need opposite treatment.  A host
/// timestamp is `2026-09-21T13:46:42.366174805Z`, where the fraction is
/// noise and the `T` is a separator.  The receiver's own is
/// `20050727.06:17:34`, where the dot separates the date from the time
/// -- so cutting at the first dot, which is right for the first, threw
/// away the whole time of day for the second.
///
/// The receiver's stamps are shown as written.  They come from a
/// calendar 1024 weeks behind, and correcting them here would put a
/// date on screen that appears nowhere in the instrument and cannot be
/// searched for.
fn stamp(at: &str) -> String {
    let shown = match at.split_once('T') {
        Some((date, time)) => format!("{date} {}", time.split('.').next().unwrap_or(time)),
        None => at.to_owned(),
    };
    shown.chars().take(STAMP_WIDTH).collect()
}

/// Width of the timestamp column.
const STAMP_WIDTH: usize = 19;

/// Graphs over a longer span, read from the daemon's log.
///
/// The three share one time axis and are stacked rather than overlaid,
/// because they have different units and the question they answer is
/// whether they move together: EFC following temperature is the room,
/// EFC moving without it is the oscillator.
fn history(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .split(area);
    header(frame, rows[0], app);

    if let Some(why) = &app.history_error {
        frame.render_widget(
            Paragraph::new(why.clone()).block(block("History")),
            rows[1].union(rows[3]),
        );
        return;
    }

    let span = app.window.label();
    // The whole window on every pane, not each trace's own extent, so
    // the three line up: a tier that stopped reading would otherwise
    // stretch its trace to the edge and put its last value under the
    // others' "now".
    let window = -(app.window.seconds() as f64);
    for (area, title, trace, colour) in [
        (rows[1], "EFC percent", &app.history.efc, Color::Cyan),
        (
            rows[2],
            "Internal temperature C",
            &app.history.temperature,
            Color::Yellow,
        ),
        (
            rows[3],
            "1 PPS TI ns",
            &app.history.time_interval,
            Color::Green,
        ),
    ] {
        graph(
            frame,
            area,
            &format!("{title}, {span}"),
            trace,
            colour,
            window,
        );
    }
}

/// One metric against time, drawn as a band between its extremes with
/// the mean through it.
///
/// Where every reading in a column agreed the three coincide and it
/// reads as a single line.  Where they did not, the band shows how far
/// apart they were, which is the only way a step survives being thinned
/// into a column.
fn graph(frame: &mut Frame, area: Rect, title: &str, trace: &Trace, colour: Color, since: f64) {
    let Some(y) = trace.bounds() else {
        frame.render_widget(
            Paragraph::new("no readings in this window").block(block(title)),
            area,
        );
        return;
    };
    // Say what the three lines are.  A column holds every reading that
    // fell in it, so the outer pair is the spread within that column
    // and collapses onto the mean wherever the readings agreed.
    let titled = format!("{title}  [mean, min..max]");
    // Braille packs four times the horizontal resolution of a cell, so
    // an hour of readings fits a terminal width.
    let marker = Marker::Braille;
    let edge = Style::new().fg(colour).add_modifier(Modifier::DIM);
    // A dataset per run, so the line breaks where the record does.
    let datasets: Vec<Dataset> = trace
        .each_run()
        .flat_map(|[mean, low, high]| {
            // A line between fewer than two points draws nothing, and
            // a run can be a single column.  Scatter still shows it.
            let kind = if mean.len() < 2 {
                GraphType::Scatter
            } else {
                GraphType::Line
            };
            [(low, edge), (high, edge), (mean, Style::new().fg(colour))].map(|(data, style)| {
                Dataset::default()
                    .marker(marker)
                    .graph_type(kind)
                    .style(style)
                    .data(data)
            })
        })
        .collect();
    let x = [since, 0.0];
    let axis = Style::new().fg(Color::DarkGray);
    let chart = Chart::new(datasets)
        .block(block(&titled))
        // No floating legend: three entries do not fit a pane this
        // short, and where they do they sit on top of the trace.  The
        // title carries the key instead, where it always shows and
        // costs no chart area.
        .legend_position(None)
        .x_axis(
            Axis::default()
                .style(axis)
                .bounds(x)
                .labels([oldest(x[0]), "now".to_owned()]),
        )
        .y_axis(
            Axis::default()
                .style(axis)
                .bounds(y)
                .labels([format(y[0], y), format(y[1], y)]),
        );
    frame.render_widget(chart, area);
}

/// Show enough decimals to tell the two axis labels apart.
///
/// EFC moves by thousandths of a percent, so a fixed three decimals
/// prints the same number at both ends of the axis and the reader
/// cannot see the scale at all.
fn format(value: f64, bounds: [f64; 2]) -> String {
    let span = (bounds[1] - bounds[0]).abs();
    let decimals = if span <= 0.0 {
        3
    } else {
        (-span.log10().floor() as i32 + 1).clamp(0, 6) as usize
    };
    format!("{value:.decimals$}")
}

/// Label the left edge of the time axis.
fn oldest(seconds_ago: f64) -> String {
    let ago = -seconds_ago;
    if ago >= 86_400.0 {
        format!("-{:.1}d", ago / 86_400.0)
    } else if ago >= 3600.0 {
        format!("-{:.1}h", ago / 3600.0)
    } else {
        format!("-{:.0}m", ago / 60.0)
    }
}

/// Current state at a glance.
fn dashboard(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(11),
            Constraint::Min(6),
        ])
        .split(area);

    header(frame, rows[0], app);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    lock(frame, middle[0], app);
    oscillator(frame, middle[1], app);

    // The satellite table is not here: it comes from the status screen,
    // which no tier polls.  It has a view of its own, which asks.
    time_and_place(frame, rows[2], app);
}

fn block(title: &str) -> Block<'static> {
    Block::bordered()
        .border_set(border::ROUNDED)
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
    let mut spans = vec![
        Span::styled(format!("[{state}]"), style),
        Span::raw("  "),
        Span::raw(app.attachment.describe()),
    ];
    // The receiver's own alarm, which latches and stays latched until
    // someone clears it at the front panel.  Put in the header rather
    // than a pane because it is the one thing that should be seen
    // whatever else is on screen, and because the daemon deliberately
    // does not clear it: what is shown here is what the lamp is showing.
    if let Some(snapshot) = app.snapshot.as_ref() {
        if snapshot.time_reset == Some(true) {
            spans.push(Span::styled(
                "  [CLOCK STEPPED]",
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        } else if snapshot.alarming == Some(true) {
            spans.push(Span::styled(
                format!("  [ALARM: {}]", snapshot.alarm_summary.join(", ")),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(block("smartclockmon")),
        area,
    );
}

/// A note for a pane heading when the tier behind it has gone quiet.
///
/// The whole snapshot used to carry one timestamp, so the one-second
/// tier succeeding kept relabelling a minutes-old status screen as
/// current.  Each pane now says the age of the fields it is actually
/// showing.
fn staleness(app: &App, tier: Tier) -> Option<String> {
    let snapshot = app.snapshot.as_ref()?;
    let now = jiff::Timestamp::now();
    // The daemon's own cadence, not this tier's default: run with
    // --medium 30 and a ten-second rule calls perfectly fresh data
    // stale, which is the opposite of what the label is for.
    let age = snapshot.polled.age(tier, now)?;
    snapshot
        .polled
        .is_stale(tier, &app.cadence, now)
        .then(|| format!("  [{age:.0}s old]"))
}

/// The last screen, if it was read recently enough to explain the
/// mode: within the medium tier's current window.
fn recent_screen(app: &App) -> Option<&Screen> {
    let read = app.last_screen.as_ref()?;
    let age = (jiff::Timestamp::now() - read.at)
        .total(jiff::Unit::Second)
        .ok()?;
    (age <= app.cadence.current_window(Tier::Medium)).then_some(&read.screen)
}

/// A field read off the last status screen, with how long ago that
/// was: no tier polls the screen, so it is as old as the last read --
/// the status view's, or the daemon's own for its log, every `--sky`
/// seconds.
fn from_screen<'a>(app: &App, label: &'a str, value: String, style: Style) -> Line<'a> {
    let Some(read) = app.last_screen.as_ref() else {
        return field(label, value, style);
    };
    let age = (jiff::Timestamp::now() - read.at)
        .total(jiff::Unit::Second)
        .unwrap_or(0.0);
    let mut line = field(label, value, style);
    line.spans.push(Span::styled(
        format!("  ({} ago)", elapsed(age)),
        Style::new().fg(Color::DarkGray),
    ));
    line
}

/// A duration in the coarsest unit that still says it.
fn elapsed(seconds: f64) -> String {
    if seconds < 90.0 {
        format!("{seconds:.0} s")
    } else if seconds < 90.0 * 60.0 {
        format!("{:.0} min", seconds / 60.0)
    } else {
        format!("{:.1} h", seconds / 3600.0)
    }
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
            Paragraph::new("waiting for a reading").block(block("Lock")),
            area,
        );
        return;
    };
    let (mode_text, mode_style) = mode_line(s, recent_screen(app));
    let mode = field("mode", mode_text, mode_style);

    let lines = vec![
        mode,
        s.tfom
            .map_or_else(|| absent("TFOM"), |v| plain("TFOM", v.to_string())),
        s.ffom
            .map_or_else(|| absent("FFOM"), |v| plain("FFOM", v.to_string())),
        s.time_interval_ns.map_or_else(
            || absent("1 PPS TI"),
            |v| plain("1 PPS TI", format!("{v:+.1} ns")),
        ),
        s.holdover_waiting
            .map_or_else(|| absent("waiting"), |w| plain("waiting", w.to_string())),
        Line::from(vec![
            Span::styled(
                format!("{:<LABEL_WIDTH$}", "TI trend"),
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled(ti_trend(app, 30), Style::new().fg(Color::Green)),
        ]),
        s.holdover_predicted_s.map_or_else(
            || absent("24 h error"),
            |v| plain("24 h error", Seconds::new(v).to_string()),
        ),
        match s.holdover_present_s {
            Some(v) => field(
                "now off by",
                Seconds::new(v).to_string(),
                Style::new().fg(Color::Yellow),
            ),
            // Only meaningful in holdover, so its absence is normal.
            None => absent("now off by"),
        },
        app.screen()
            .and_then(|sc| sc.hold_threshold.clone())
            .map_or_else(
                || absent("hold thr"),
                |v| from_screen(app, "hold thr", v, Style::new()),
            ),
        match (s.holdover_active, s.holdover_seconds) {
            (Some(active), Some(seconds)) => {
                let elapsed = Seconds::new(seconds);
                let text = if active {
                    format!("active, {elapsed}")
                } else {
                    format!("last {elapsed}")
                };
                let style = if active {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new()
                };
                field("holdover", text, style)
            }
            _ => absent("holdover"),
        },
    ];
    frame.render_widget(Paragraph::new(lines).block(block("Lock")), area);
}

/// The mode line, and how to colour it.
///
/// The state comes from `:SYNChronization:STATe?`, which is on the
/// one-second tier, but that returns a bare `LOCK` with no detail.  The
/// detail -- "stabilizing frequency", "GPS acquisition", "GPS 1PPS
/// invalid" -- exists only on the status screen, which no tier polls.
///
/// So the two are combined, from a recent screen only
/// ([`recent_screen`]), and the suffix is used only while the screen
/// still agrees about the base state.  Otherwise a transition would
/// show the fresh state carrying a stale explanation, which is worse
/// than no explanation.
fn mode_line(snapshot: &Reading, screen: Option<&Screen>) -> (String, Style) {
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

    let detail = screen
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
                format!("{:.0}%  {}", used * 100.0, gauge(used, 18)),
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
        match (snap.temperature_c, snap.oven_current) {
            (Some(t), Some(i)) => {
                lines.push(plain("internal temp", format!("{t:.2} C    oven {i:.1}")));
            }
            (Some(t), None) => lines.push(plain("internal temp", format!("{t:.2} C"))),
            _ => {}
        }
        if let Some(code) = snap.efc_raw {
            lines.push(plain(
                "EFC raw",
                format!("{code} of {}", EfcPercent::FULL_SCALE),
            ));
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
    match app.screen().and_then(health_faults) {
        Some(faults) if faults.is_empty() => {
            lines.push(from_screen(
                app,
                "self report",
                "all OK".to_owned(),
                Style::new().fg(Color::Green),
            ));
        }
        Some(faults) => {
            for fault in faults {
                lines.push(from_screen(
                    app,
                    "REPORTED",
                    fault,
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
        }
        None => {}
    }

    let mut title = "Oscillator".to_owned();
    // EFC is fast-tier but temperature and the raw value are not.
    if let Some(age) = staleness(app, Tier::Medium) {
        title.push_str(&age);
    }
    frame.render_widget(Paragraph::new(lines).block(block(&title)), area);
}

/// A horizontal bar showing how much of the tuning range is used.
fn gauge(fraction: f64, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let (full, empty) = ('\u{2588}', '\u{2591}');
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
    let ramp = TREND_BLOCKS;
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

/// A sparkline of recent 1 PPS intervals, scaled to its own span.
fn ti_trend(app: &App, width: usize) -> String {
    let mut values = app.ti_trend.iter().copied();
    let Some(first) = values.next() else {
        return "--".to_owned();
    };
    let (lo, hi) = values.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let ramp = TREND_BLOCKS;
    let span = (hi - lo).max(f64::EPSILON);
    app.ti_trend
        .iter()
        .rev()
        .take(width)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|v| {
            let level = ((v - lo) / span * (ramp.len() - 1) as f64).round();
            ramp[(level as usize).min(ramp.len() - 1)]
        })
        .collect()
}

/// An exponent as superscript digits, for a decade axis label.
///
/// `10^-9` written as `10` and a raised `-9` rather than as `1e-9`,
/// which is a programming language's spelling of a number and not a
/// physicist's.
fn superscript(exponent: i32) -> String {
    const DIGITS: [char; 10] = [
        '\u{2070}', '\u{00b9}', '\u{00b2}', '\u{00b3}', '\u{2074}', '\u{2075}', '\u{2076}',
        '\u{2077}', '\u{2078}', '\u{2079}',
    ];
    let sign = if exponent < 0 { "\u{207b}" } else { "" };
    let digits: String = exponent
        .unsigned_abs()
        .to_string()
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| DIGITS[d as usize])
        .collect();
    format!("{sign}{digits}")
}

/// The Allan deviation, on a view of its own.
///
/// Both axes are decades, which is the only way this curve is read: the
/// slope between decades is what names the noise.  ratatui has no log
/// axis, so the points are plotted as their logarithms and the labels
/// say what the numbers are.
fn stability(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Fill(1)])
        .split(area);
    header(frame, rows[0], app);

    let curve = &app.deviation;
    if curve.points.is_empty() {
        let why = if curve.present == 0 {
            "no 1 PPS readings in this window".to_owned()
        } else {
            format!(
                "{} readings over {} unbroken runs: too few for a deviation",
                curve.present, curve.segments
            )
        };
        frame.render_widget(
            Paragraph::new(why).block(block(&format!("Stability  {}", app.window.label()))),
            rows[1],
        );
        return;
    }

    // The modified deviation is the one this record supports exactly
    // (see `smartclock::adev`); the plain one is drawn beside it, dim,
    // for comparison with the figures data sheets quote.
    let plain: Vec<(f64, f64)> = curve
        .points
        .iter()
        .map(|p| (p.tau.log10(), p.deviation.log10()))
        .collect();
    let modified: Vec<(f64, f64)> = curve
        .points
        .iter()
        .filter_map(|p| Some((p.tau.log10(), p.modified?.deviation.log10())))
        .collect();
    let points: Vec<(f64, f64)> = plain.iter().chain(&modified).copied().collect();
    let bounds = |values: &[f64]| {
        let low = values.iter().copied().fold(f64::INFINITY, f64::min).floor();
        let high = values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil();
        // A curve inside one decade would otherwise be drawn on a zero
        // height axis.
        if high > low {
            [low, high]
        } else {
            [low, low + 1.0]
        }
    };
    let x = bounds(&points.iter().map(|p| p.0).collect::<Vec<_>>());
    let y = bounds(&points.iter().map(|p| p.1).collect::<Vec<_>>());
    let decades = |range: [f64; 2]| {
        let (low, high) = (range[0] as i32, range[1] as i32);
        (low..=high)
            .map(|d| format!("10{}", superscript(d)))
            .collect::<Vec<_>>()
    };

    let datasets = vec![
        Dataset::default()
            .name("ADEV")
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(ratatui::widgets::GraphType::Line)
            .style(Style::new().fg(Color::DarkGray))
            .data(&plain),
        Dataset::default()
            .name("MDEV")
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(ratatui::widgets::GraphType::Line)
            .style(Style::new().fg(Color::Cyan))
            .data(&modified),
    ];
    let axis = Style::new().fg(Color::DarkGray);
    // The coverage is in the title because the curve cannot show it: a
    // run that is mostly holes draws the same line as a clean one.
    let total = curve.present + curve.holes;
    let complete = if total == 0 {
        0.0
    } else {
        100.0 * curve.present as f64 / total as f64
    };
    let title = format!(
        "Stability  {}   tau0 {:.3} s   {} readings, {} runs, {complete:.0}% complete",
        app.window.label(),
        curve.tau0,
        curve.present,
        curve.segments,
    );
    frame.render_widget(
        Chart::new(datasets)
            .block(block(&title))
            .legend_position(Some(ratatui::widgets::LegendPosition::TopRight))
            .x_axis(
                Axis::default()
                    .style(axis)
                    .bounds(x)
                    .labels(decades(x))
                    .title("τ, seconds"),
            )
            .y_axis(
                Axis::default()
                    .style(axis)
                    .bounds(y)
                    .labels(decades(y))
                    .title("σy"),
            ),
        rows[1],
    );
}

/// The receiver's status screen as it sent it, beside the satellites
/// scraped from it.
///
/// The screen costs the receiver about 1.5 s of its link, so it is read
/// while this view is open and at no other time.  The screen is shown
/// whole because the read has been paid for either way, and the
/// receiver's own layout says things no parsed field does.
fn status(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    header(frame, rows[0], app);
    // Side by side when the screen fits whole beside the table, stacked
    // otherwise, so that neither is clipped on a narrow terminal.
    let text = app.screen().map(|s| s.text.as_str());
    let border = 2;
    let screen_width = text
        .and_then(|t| t.lines().map(|l| l.chars().count()).max())
        .map_or(SCREEN_COLUMNS, |w| u16::try_from(w).unwrap_or(u16::MAX))
        .saturating_add(border);
    let screen_height = text
        .map_or(1, |t| u16::try_from(t.lines().count()).unwrap_or(u16::MAX))
        .saturating_add(border);
    let panes = if rows[1].width >= screen_width.saturating_add(SATELLITE_COLUMNS) {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(screen_width)])
            .split(rows[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(screen_height)])
            .split(rows[1])
    };
    satellites(frame, panes[0], app);
    screen_text(frame, panes[1], app);
}

/// The screen as received, one line per line.
fn screen_text(frame: &mut Frame, area: Rect, app: &App) {
    let text = app
        .screen()
        .map_or("reading the status screen...", |s| s.text.as_str());
    frame.render_widget(Paragraph::new(text).block(block("Status screen")), area);
}

fn satellites(frame: &mut Frame, area: Rect, app: &App) {
    let Some(screen) = app.screen() else {
        frame.render_widget(
            Paragraph::new("reading the status screen...").block(block("Satellites")),
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

    let mut title = match (screen.tracking, screen.not_tracking) {
        (Some(t), Some(n)) => format!("Satellites  {t} tracked, {n} not"),
        _ => "Satellites".to_owned(),
    };
    // The screen states its own counts, so a table that disagrees with
    // them has been misread.  Say so rather than letting a wrong sky
    // look like a right one.
    if screen.satellites_suspect {
        title.push_str("  [table disagrees with these counts]");
    }
    let header_style = if screen.satellites_suspect {
        Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
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
    .header(Row::new(vec!["PRN", "El", "Az", "SS", "state"]).style(header_style))
    .block(block(&title));
    frame.render_widget(table, area);
}

fn time_and_place(frame: &mut Frame, area: Rect, app: &App) {
    let Some(s) = app.snapshot.as_ref() else {
        frame.render_widget(
            Paragraph::new("waiting for a reading").block(block("Time and position")),
            area,
        );
        return;
    };
    let mut lines = Vec::new();

    match s.time {
        Some(time) => lines.push(plain("UTC", format!("{time}"))),
        None => lines.push(absent("UTC")),
    }
    match s.date {
        Some(date) => match date.rollover() {
            Some(slip) => {
                // Nearly every receiver of this vintage is behind by
                // whole GPS epochs, so this is the normal condition and
                // not a fault: its time of day and its outputs are
                // unaffected.  Shown corrected, therefore, with the
                // correction noted rather than shouted -- a warning
                // that is always lit is not a warning, and this strip
                // is where real faults have to be noticed.
                //
                // The raw date stays visible because the correction is
                // ours, not the receiver's: it is computed against the
                // host clock, and a reader should be able to see what
                // the instrument actually said.
                lines.push(field("date", date.corrected().to_string(), Style::new()));
                // On its own line rather than appended: the pane is
                // narrow when the terminal is, and one long line loses
                // its tail exactly where the provenance is.
                lines.push(field(
                    "reported",
                    format!("{}  (+{slip})", date.raw()),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            None => lines.push(plain("date", date.raw().to_string())),
        },
        None => lines.push(absent("date")),
    }

    if let Some(screen) = app.screen() {
        match (&screen.time, &screen.time_scale) {
            (Some(t), Some(scale)) => {
                let suspect = if screen.time_suspect { "  [?]" } else { "" };
                lines.push(from_screen(
                    app,
                    "time",
                    format!("{t} {scale}{suspect}"),
                    Style::new(),
                ));
            }
            _ => lines.push(absent("time")),
        }
        if let Some(status) = &screen.sync_status {
            lines.push(from_screen(app, "1 PPS", status.clone(), Style::new()));
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

    if let Some(screen) = app.screen() {
        if let Some(mode) = &screen.position_mode {
            lines.push(from_screen(app, "survey", mode.clone(), Style::new()));
        }
        if let Some(why) = &screen.survey_suspended {
            lines.push(from_screen(
                app,
                "suspended",
                why.clone(),
                Style::new().fg(Color::Yellow),
            ));
        }
    }

    // Position and date are on the sixty-second tier.
    let mut title = "Time and position".to_owned();
    if let Some(age) = staleness(app, Tier::Slow) {
        title.push_str(&age);
    }
    frame.render_widget(Paragraph::new(lines).block(block(&title)), area);
}

/// The raw command line, and the last answer.
///
/// Shown even when the daemon has not been given --allow-raw.  Hiding
/// it would leave no way to discover that the console exists or what
/// would enable it; shown with the reason, the refusal teaches the
/// flag.
fn console(frame: &mut Frame, area: Rect, app: &App) {
    // Say what the daemon permits, so a refusal is not a surprise and
    // the flag that would lift it is discoverable.
    let title = if let Attachment::Direct { .. } = app.attachment {
        "Command  [direct mode has no daemon to ask]".to_owned()
    } else if !app.console.is_connected() {
        "Command  [not connected to the daemon]".to_owned()
    } else {
        let mut allowed = vec!["queries"];
        if app.policy.control {
            allowed.push("control");
        }
        if app.policy.dangerous {
            allowed.push("dangerous");
        }
        if app.policy.raw {
            allowed.push("raw");
        }
        format!("Command  [this daemon allows: {}]", allowed.join(", "))
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("scpi> ", Style::new().fg(Color::Cyan)),
        Span::raw(app.console_input.clone()),
        Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
    ])];
    match &app.console_reply {
        Some(Ok(answer)) => lines.push(Line::from(Span::styled(
            answer.clone(),
            Style::new().fg(Color::DarkGray),
        ))),
        Some(Err(why)) => lines.push(Line::from(Span::styled(
            why.clone(),
            Style::new().fg(Color::Red),
        ))),
        None => {}
    }
    frame.render_widget(Paragraph::new(lines).block(block(&title)), area);
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let keys = if app.console_open {
        "Enter send  Esc close".to_owned()
    } else if app.receivers.len() > 1 {
        "q quit  g next view  l journal  w window  r receiver  c command".to_owned()
    } else {
        "q quit  g next view  l journal  w window  c command".to_owned()
    };
    let mut spans = vec![Span::styled(keys, Style::new().fg(Color::DarkGray))];
    // Only when the log holds more than one, since naming the sole
    // receiver on every frame says nothing.
    if let Some(label) = app.receiver_label() {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            format!("showing {label}"),
            Style::new().fg(Color::Cyan),
        ));
    }
    if let Some(error) = app.snapshot.as_ref().and_then(|s| s.polled.any_error()) {
        spans.push(Span::raw("   "));
        spans.push(Span::styled(error.to_owned(), Style::new().fg(Color::Red)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The health monitor line, reduced to what is wrong.
///
/// The full line runs to about seventy characters, which will not fit a
/// half-width pane, and reading six "OK"s to find the one that is not
/// is worse than being told directly.  `None` means nothing to report.
fn health_faults(screen: &Screen) -> Option<Vec<String>> {
    let items = &screen.health_items;
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
    use super::draw;
    use super::mode_line;
    use crate::app::App;
    use crate::app::View;
    use crate::source::Attachment;
    use crate::source::Console;
    use crate::source::Policy;
    use jiff::Timestamp;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use smartclock::screen;
    use smartclock::snapshot::Freshness;
    use smartclock::snapshot::Snapshot;
    use smartclock::task::Cadence;
    use smartclock::types::SmartClockMode;
    use smartclock::wire::Reading;

    fn snapshot(mode: Option<SmartClockMode>, screen_text: Option<&str>) -> Reading {
        let mut s = Snapshot::new(Timestamp::now());
        s.mode = mode;
        s.screen = screen_text.map(screen::parse);
        Reading::from(&s)
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
        assert_eq!(
            mode_line(&s, s.screen.as_ref()).0,
            "Locked to GPS: stabilizing frequency"
        );
    }

    #[test]
    fn a_state_with_no_detail_reads_plainly() {
        let s = snapshot(
            Some(SmartClockMode::Locked),
            Some(&screen_with("Locked to GPS")),
        );
        assert_eq!(mode_line(&s, s.screen.as_ref()).0, "Locked to GPS");
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
        assert_eq!(mode_line(&s, s.screen.as_ref()).0, "Holdover");
    }

    #[test]
    fn holdover_keeps_its_own_detail() {
        let s = snapshot(
            Some(SmartClockMode::Holdover),
            Some(&screen_with("Holdover: GPS 1PPS invalid")),
        );
        assert_eq!(
            mode_line(&s, s.screen.as_ref()).0,
            "Holdover: GPS 1PPS invalid"
        );
    }

    #[test]
    fn power_up_detail_survives_the_missing_space_after_the_colon() {
        // The firmware writes "Power-up:GPS acquisition", with no space.
        let s = snapshot(
            Some(SmartClockMode::PowerUp),
            Some(&screen_with("Power-up:GPS acquisition")),
        );
        assert_eq!(
            mode_line(&s, s.screen.as_ref()).0,
            "Power-up: GPS acquisition"
        );
    }

    #[test]
    fn without_a_screen_the_state_still_shows() {
        let s = snapshot(Some(SmartClockMode::Locked), None);
        assert_eq!(mode_line(&s, s.screen.as_ref()).0, "Locked to GPS");
    }

    #[test]
    fn nothing_polled_yet_shows_as_absent() {
        let s = snapshot(None, None);
        assert_eq!(mode_line(&s, s.screen.as_ref()).0, "--");
    }

    /// What `app` draws in `view`, as text.
    fn drawn(app: &mut App, view: View) -> String {
        app.view = view;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("a terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn app() -> App {
        App::new(
            Attachment::Direct {
                device: "/dev/null".to_owned(),
            },
            Console::default(),
            Policy::default(),
            Cadence::default(),
        )
    }

    #[test]
    fn the_dashboard_shows_the_screen_after_the_snapshot_that_carried_it() {
        // The screen arrives on one snapshot and the next poll's has
        // none, which left every screen-only field on the dashboard
        // blank.
        let live = |screen: Option<&str>| {
            let mut s = snapshot(Some(SmartClockMode::Locked), screen);
            s.freshness = Freshness::Live;
            s
        };
        let mut app = app();
        app.accept(live(Some(&screen_with(
            "Locked to GPS: stabilizing frequency",
        ))));
        app.accept(live(None));
        let screen = drawn(&mut app, View::Dashboard);
        assert!(
            screen.contains("Locked to GPS: stabilizing frequency"),
            "{screen}"
        );
        assert!(screen.contains("s ago)"), "the age is not shown: {screen}");
    }

    #[test]
    fn a_disconnected_reading_forgets_the_screen() {
        let mut app = app();
        let mut read = snapshot(
            Some(SmartClockMode::Locked),
            Some(&screen_with("Locked to GPS")),
        );
        read.freshness = Freshness::Live;
        app.accept(read);
        assert!(app.screen().is_some());
        let mut gone = snapshot(Some(SmartClockMode::Locked), None);
        gone.freshness = Freshness::Disconnected;
        app.accept(gone);
        assert!(app.screen().is_none());
    }

    #[test]
    fn the_console_is_drawn_in_every_view() {
        // The keys that open it work everywhere, so a view that did not
        // draw it had the operator typing into nothing.
        let mut app = app();
        app.console_open = true;
        app.console_input = "*IDN?".to_owned();
        for view in [
            View::Dashboard,
            View::History,
            View::Journal,
            View::Status,
            View::Stability,
        ] {
            assert!(drawn(&mut app, view).contains("scpi> *IDN?"), "{view:?}");
        }
    }
}
