# The views

What the monitor, the browser pages and the exporter show, and why.
`running.md` covers installing and configuring them.

## The monitor

In the monitor, `g` or Tab cycles the views, `l` jumps to the journal,
`w` cycles the graph span, `r` the receiver when the log holds more
than one, `c` or `:` opens a command line, `q` quits, and Ctrl-C quits
from anywhere, the command line included.  The journal is what
the receiver has recorded about itself -- its diagnostic log, its
error queue and its alarm changes -- and the notes written about it
(`docs/running.md`, "Notes and facts"); the browser view shows it too.

`smartclockmon --device ...` talks to the receiver directly, which
needs the daemon stopped and records no history; the header says so.

The header ends with the host's sensors and their current readings,
from `smartclock-sensord` (`--sensord`), or `--` for one that
has stopped reading.  The history view adds a pane per sensor quantity
below the receiver's three, a line per sensor, from the log the service
says it keeps.

## Dates

Firmware predating the 2019 GPS week rollover reports a date 1024
weeks behind, as nearly every receiver of this vintage does.  The
clients show the corrected date with a note, not a warning, which
would stay lit on a healthy instrument.  The receiver's own date stays
visible beside it, because the correction is arithmetic done here
against the host clock.

## The status screen

The status view, in the monitor and at `/status` in the browser,
shows the receiver's `:SYSTem:STATus?` screen as sent, beside the
satellites scraped from it, which the browser draws as a sky plot.
The screen is read, through the socket op `status`, while the view is
open, and by the daemon every `--sky` seconds (300 by default) to keep
the satellite table in the log: a read costs about 1.5 s of a 19200
link, four times a whole one-second poll, and no tier polls it.  The
satellite counts are queried directly, on the medium tier.

The monitor's dashboard shows what only the screen says -- the hold
threshold, the time and its scale, the 1 PPS status, the survey, the
receiver's own health report -- from the last screen read, each with
its age, and forgets it when the link drops.  With the status view
closed, that read is the daemon's own for its log, every `--sky`
seconds, five minutes by default.  The mode's detail ("stabilizing
frequency") comes only from a screen read within the last three
medium-tier intervals, so a stale explanation never labels a fresh
state.

## Stability

Stability is its own view in both, `/adev` in the browser: the
modified Allan deviation, the time deviation, the maximum time
interval error and the overlapping Allan deviation of the interval
between the 1 PPS from the GPS receiver and a 1 PPS divided down from
the OCXO, on log axes.  The modified form leads because the receiver's
reading is already a ten-second mean, the innermost block of that
form's own averaging, so it is exact here, where the plain form sits a
factor √10 low wherever the receiver's white phase noise dominates --
on the bench, the whole measured range.  The plain form stays because
data sheets quote it.  How the curves are computed and checked is in
`docs/stability.md`.
Each curve is shaded to its one-sigma interval, from chi-squared
statistics with Greenhall's degrees of freedom for the noise type the
lag 1 autocorrelation method finds at each tau (NIST SP 1065 sections
5.3 to 5.5), checked against allantools point by point.  The GPS
receiver's 1 PPS is quantized to its own crystal, and while locked
the OCXO is steered to follow it, so the curve is of the pair and of
the loop between them rather than of the OCXO alone; the page says
so.  Gaps stay unfilled: the run is cut where a relock, a holdover
or an absence makes the phase either side incomparable, only the
second differences that exist are counted, and the curve carries the
number of readings, holes and unbroken runs behind it.  `PLAN.md` has
the decisions behind it.

## The browser

The browser view is read-only and is not the monitor in a window: it
draws what a terminal cannot, mainly history that can be dragged to
zoom, and the polar sky plot.

Any number of series can be stacked, and they share a time axis by
construction: one request buckets them all in the same pass, so their
x values are identical; separate requests would each compute bucket
boundaries from their own end time.  The cursor moves across the stack
together and a drag on any plot zooms all of them.  EFC against
internal temperature is the most useful pairing; `efc.md` has what
that comparison showed.

The time range works as in Grafana's dashboards.  It is "the last N
units" up to now -- presets from an hour to thirty days and `all` fill
the box in, and any other length can be typed -- or a fixed pair, which
a drag on a chart zooms to.  ‹ and › move it by half its length, −
and a double click on a chart double it about its center, and `now`
returns a fixed range to a moving one; with `all` there is nothing to
move or zoom.  Grafana's keys work: `t ←` and `t →` move, `t -` (or
Ctrl+Z) zooms out, `t +` zooms in, `t a` fixes a moving range where it
is.  Each change of range is a step the browser's Back button undoes.
A refresh picker sets how often the range is read again: Auto by
default, about one pixel's worth of time across the window rounded up
to the next of the fixed intervals, or one of those intervals, never
faster than the page can afford (5 s for history, a minute where each
read measures stability).  The correlation page has no picker: it is
read when its range or measures change.  ⟳ reads at once.  Reading pauses while the
tab is hidden, while a read is still running, while the pointer is on
a chart, and while text is selected.

Two things differ from Grafana.  A range moved or zoomed out to end
within half its length of now becomes the moving range of that length,
where Grafana would slide on into the future.  And a fixed range is
read again only until a read has started after its end; past that its
readings cannot change.

The choice rides in the address (`?last=172800`, `?last=all`, or
`?from=…&to=…` in unix seconds, plus `refresh=`) beside the receiver
and the columns, so a reload or a shared link shows the same window,
and links between pages carry the receiver and the range.  Grafana's
forms are read too: `from=now-6h&to=now` (units s, m, h, d and w; not
months, years or rounding such as `now/d`), epoch milliseconds, ISO
times, with `to=now` keeping the range moving; so is the older
`range=SECONDS`.  The stability page uses the
same control; there the range is the record the estimator runs on, so
changing it recomputes.

The host's sensors, from `smartclock-sensord`, are charted a quantity
apiece and a line per sensor, right below the receiver's internal
temperature, on the same time axis; a sensors box in the column menu
leaves them out (`sensors=off`).  Their current readings end the live
strip on every page.  A host without the service shows neither, and
with no receiver logged the sensors are charted alone.

The chart library comes from a CDN, pinned with an integrity hash, so
the page needs internet though the daemon does not.  If the library
cannot be fetched, the page says so instead of showing an empty
frame.

`/compare` overlays every receiver the logs hold over one range: 1 PPS
TI, EFC, temperature, TFOM and FFOM a chart each with a line per
receiver, the host's sensors below them a chart per quantity, and the
receivers' MDEV (solid) and ADEV (dashed) curves on one plot.
A checkbox per receiver chooses which are drawn, ticked at first for
every one with readings in the range, and each receiver keeps one
color.  Each 1 PPS TI is against that receiver's own GPS solution, so
the lines compare errors, not one unit against another.  Notes show as
on the live page, in their receiver's color.  Every receiver is asked
for the same window, so their points share one time grid; a receiver
that cannot be read is named as such, and a stability curve from a
range longer than one measurement reads says it covers the newest
part.

`/correlation` sets two measures against each other over one range:
the chosen receiver's EFC, internal temperature or 1 PPS TI, any of
the host's sensors, or another receiver's EFC.  Each is charted
against time, then Y against X with its least-squares line, then r as
Y is moved against X, a bucket at a time, two hours either way, so a
peak, marked, says which leads and by how much.  The figures are r,
r², both also in the scatter's title, r of the bucket-to-bucket
changes, which is near 0 when the two share only a slow drift, the
slope of an ordinary least-squares fit of Y on X, the best lag, and how many buckets of what
width went in.  The range is a day unless the address says otherwise,
which holds one turn of the room's temperature; zoom in for a finer
lag, or to leave out a stretch such as a crystal trim.  "Locked only", on by default
(`locked=0` turns it off), leaves out readings taken in holdover, in
recovery and at power-up, when the EFC is frozen or being slewed;
`/api/history` takes it as `locked=1`.

The live, status, stability and correlation pages show one
receiver at a time, chosen by a selector that appears once there is
more than one and follows receivers and daemons as they come and go.
The address carries the choice as `?receiver=<id>`, so it survives a
reload and moving between pages.  The pages share one lifecycle, in
`crates/smartclock-web/src/common.js`: a change of receiver clears
what was shown before the new one is read, an answer that arrives for
the last receiver is dropped, and when a daemon goes, what was read
from it is cleared and read again once it is back.  Playwright tests
check that in a browser against a faked API: `make test-web`, after
`make web-deps` once, and in CI (`.github/workflows/web.yml`).

Pages show only the columns the chosen unit measures: a reading it
cannot take -- a Z3801A or Z3805A has no internal temperature -- is
left out of the strip and the chart menu, not shown as a permanent
`--` or an empty chart.  What a unit measures comes from its model's
command table, in `/api/receivers`, so this holds for units only in
the logs too.

## The exporter

The exporter answers a scrape from whatever each daemon last polled,
so scraping costs the receivers nothing and cannot compete with the
poll schedule.  Every sample is labeled with the daemon instance and
the receiver's serial and model.  It exports `smartclock_up`, and the
age of each tier as `smartclock_tier_age_seconds`, since a daemon that
has stopped polling otherwise leaves every value where it was.  A
reading the receiver declined is left out rather than exported as
zero, and so is every value whose tier is failing, has not read for
three of its intervals, or whose link is down, so a panel does not go
on drawing the last number read.

The daemons are asked all at once, each given five seconds, so a
wedged one costs a scrape five seconds rather than pushing it past
Prometheus' timeout and losing every receiver's series.  A daemon that
stops, and whose directory systemd removes with it, goes on reporting
`smartclock_up 0` under its instance name until the exporter restarts,
so an alert on `up == 0` fires instead of the series ending.
Restarting the exporter forgets a unit retired on purpose.  The
receiver's identity is asked before and after its reading, and a
scrape in which it changed is skipped, so a swap never labels one
unit's reading with the other's serial.

The host's sensors, from `smartclock-sensord`'s socket, are
`smartclock_sensor_temperature_celsius`,
`smartclock_sensor_humidity_percent` and
`smartclock_sensor_pressure_pascals`, labeled `sensor`, beside
`smartclock_sensord_up`.  A reading is left out once its latest read
failed or is older than three read periods.  A host without the service exports
none of these; once it has answered, a service that stops is
`smartclock_sensord_up 0`.
