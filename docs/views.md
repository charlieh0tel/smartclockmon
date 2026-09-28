# The views

What the monitor, the browser pages and the exporter show, and why
they show it that way.  Installing and configuring them is in
`running.md`.

## The monitor

In the monitor, `g` cycles the views, `l` jumps to the journal, `w`
cycles the graph span, `c` opens a command line, `q` quits.  The
journal is what the receiver has recorded about itself -- its
diagnostic log, its error queue, and the changes in its alarm -- none of
which is in the snapshot table or can be plotted, and all of which the
browser view shows too.

`smartclockmon --device ...` talks to the receiver directly, which needs
the daemon stopped and records no history; the header says so.

## Dates

The GPS week rollover is treated as the ordinary condition it is.
Firmware predating the 2019 wrap reports a date 1024 weeks behind, and
nearly every receiver of this vintage does, so the clients show the
corrected date with the correction noted quietly rather than raising a
warning that would be lit permanently on a healthy instrument.  What
the receiver actually said stays visible beside it, because the
correction is arithmetic done here against the host clock, not
something the receiver reported.

## The status screen

The status screen is a view of its own in both the monitor and the
browser, shown as the receiver sent it beside the satellites scraped
from it, and is read only while the view is open: it costs the
receiver about 1.5 s of a 19200 link, four times what a whole
one-second poll costs.  Nothing else needs it, so nothing else pays
for it.  The satellite counts are queried directly and are on the
one-second tier with everything else.

## Stability

Stability is its own view in both, `/adev` in the browser: the
modified Allan deviation, the time deviation, the maximum time
interval error and the overlapping Allan deviation of the interval
between the 1 PPS from the GPS receiver and a 1 PPS divided down from
the OCXO, on log axes.  The modified form
leads because the receiver's reading is already a ten-second mean,
which is the innermost block of that form's own averaging, so it is
exact here where the plain form sits a factor √10 low wherever the
receiver's white phase noise dominates -- on the bench, the whole
measured range; the plain form is kept because data sheets quote it.
Each curve is shaded to its one-sigma interval, from chi-squared
statistics with Greenhall's degrees of freedom for the noise type the
lag 1 autocorrelation method finds at each tau (NIST SP 1065 sections
5.3 to 5.5), checked against allantools point by point.  The GPS
receiver's 1 PPS is quantized to its own crystal, and while locked
the OCXO is steered to follow it, so the curve is of the pair and of
the loop between them rather than of the OCXO alone; the page says
so.  Gaps are not filled in: the run is cut where a relock, a
holdover or an absence makes the phase either side incomparable, only
the second differences that exist are counted, and the curve carries
the number of readings, holes and unbroken runs behind it so it can be
judged.  `PLAN.md` has what that took.

## The browser

The browser view is read-only and is not the monitor in a window: it
draws what a terminal cannot, which is mainly history you can drag to
zoom, and the polar sky plot, beside the status screen, at `/status`.

Any number of series can be stacked, and they share a time axis by
construction rather than by appearance: one request buckets them all in
the same pass, so their x values are the same values, and separate
requests -- which would each compute their own bucket boundaries from
their own end time -- could not promise that.  The cursor moves across
the stack together and a drag on any plot zooms all of them.  EFC
against internal temperature is the pairing that earns its keep; see
`docs/efc.md` for what that comparison settled.

The time range is a pair of instants, chosen the way Grafana chooses
one: "the last N units" up to now -- presets from an hour to thirty
days and `all` fill the box in, and any other length can be typed --
or a fixed pair once a drag has zoomed, which then steps earlier and
later by its own length and returns to a moving window with `now`.
The choice rides in the address (`?last=172800`, `?last=all`, or
`?from=…&to=…`) beside the receiver and the columns, so a reload or a
shared link shows the same window, and the receiver and the range go
along to the other pages.  The stability page uses the same
control; there a range is the record the estimator runs on, so
changing it recomputes.

The chart library comes from a CDN, pinned with an integrity hash, so
the page needs internet even though the daemon does not; the page says
so rather than showing an empty frame if it cannot be fetched.

Every browser page -- live, status and stability -- shows one receiver
at a time, chosen by a selector that appears once there is more than
one and follows receivers and daemons as they come and go; the choice
is carried in the address as `?receiver=<id>`, so it survives a reload
and moving between pages.  The pages behave alike at the edges: a
change of receiver clears what was shown before the new one is read,
an answer that arrives for the last receiver is dropped, and when a
daemon goes, what was read from it is cleared and read again once it
is back.  `make test-web` checks all of that in a browser.

A reading the chosen unit cannot take -- a Z3801A or Z3805A has no
internal temperature -- is left out of the strip and the chart menu
rather than shown as a permanent `--` or an empty chart.  What a unit
measures comes from its model's command table, in `/api/receivers`,
so it holds for units only in the logs too.

## The exporter

The exporter answers a scrape from whatever each daemon last polled,
so scraping costs the receivers nothing and cannot compete with the
poll schedule.  Every sample is labeled with the daemon instance and
the receiver's serial and model.  It exports `smartclock_up`, and the age of each tier as
`smartclock_tier_age_seconds`, because a daemon that has stopped polling
otherwise looks like a remarkably steady oscillator: every other value
stays exactly where it was.  A reading the receiver declined is left out
rather than exported as zero, and so is every value whose tier is
failing or whose link is down: the last number read, exported as
though current, is what a panel would go on drawing.
