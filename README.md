# smartclockmon

A Rust library and terminal monitor for HP / Symmetricom SmartClock GPS
time and frequency reference receivers, spoken to over RS-232.

## Status

Running as a service against the development receiver, which has been
logging continuously for long enough to be useful.  `PLAN.md` carries
the design decisions and their reversals, what is still undecided, and
the defects that are known and unfixed.

The receiver it was written to diagnose looks healthier than expected:
locked rather than stuck in holdover, with its EFC mapping measured and
about a decade of tuning headroom.  Its own diagnostic log, once
recovered, points at GPS reception in March 2025 rather than at the
oscillator.  See `docs/efc.md`.

## Parts

| | |
| - | - |
| `smartclock` | the library: transports, SCPI framing, the command table, parsers, the status screen scraper, the polling task, and the Allan deviation |
| `smartclockd` | holds the serial port, logs to SQLite, serves clients over a local socket |
| `smartclockmon` | terminal monitor: a dashboard, history graphs, the journal, the sky and stability |
| `smartclock-cli` | one-shot queries, `diagnose`, transcript capture, and sweeping for undocumented commands |
| `smartclock-exporter` | Prometheus metrics for every receiver on the host, from the daemons' own readings |
| `smartclock-web` | a browser view: live state, history you can zoom, and pages for the sky and for stability |
| `smartclock-sim` | a simulated receiver, in process for tests and over TCP for driving the real daemon |

## Running it

Build with `make`, or `make deb` for a package.  Installed from the
package, the daemon runs as one instance per serial port, configured
entirely through `/etc/default/smartclockd.<instance>` and started
with `systemctl enable --now smartclockd@<instance>`; see
`docs/running.md`.

Run by hand, it holds the port and everything else is a client of it:

    smartclockd --device /dev/serial/by-id/usb-... \
                --log-dir . \
                --socket /tmp/smartclockd.sock

    smartclockmon --socket /tmp/smartclockd.sock

In the monitor, `g` cycles the views, `l` jumps to the journal, `w`
cycles the graph span, `c` opens a command line, `q` quits.  The
journal is what the receiver has recorded about itself -- its
diagnostic log, its error queue, and the changes in its alarm -- none of
which is in the snapshot table or can be plotted, and all of which the
browser view shows too.

    smartclock-exporter --socket /tmp/smartclockd.sock   # http://127.0.0.1:9979/metrics

    smartclock-web --socket /tmp/smartclockd.sock --log-dir .   # http://127.0.0.1:9980/

Installed, neither needs telling: both read every daemon's socket
under `/run/smartclockd` and every log under `/var/lib/smartclockd`.

The GPS week rollover is treated as the ordinary condition it is.
Firmware predating the 2019 wrap reports a date 1024 weeks behind, and
nearly every receiver of this vintage does, so the clients show the
corrected date with the correction noted quietly rather than raising a
warning that would be lit permanently on a healthy instrument.  What
the receiver actually said stays visible beside it, because the
correction is arithmetic done here against the host clock, not
something the receiver reported.

The sky is a view of its own in both the monitor and the browser, and
is read only while it is open: it is scraped from the status screen,
which costs the receiver about 1.5 s of a 19200 link, four times what
a whole one-second poll costs.  Nothing else needs it, so nothing else
pays for it.  The satellite counts are queried directly and are on the
one-second tier with everything else.

Stability is its own view in both, `/adev` in the browser: the
modified Allan deviation, the time deviation, the maximum time
interval error and the overlapping Allan deviation of the interval
between the 1 PPS from the GPS receiver and a 1 PPS divided down from
the OCXO, on log axes.  The modified form
leads because the receiver's reading is already a ten-second mean,
which is the innermost block of that form's own averaging, so it is
exact here where the plain form sits a factor √10 low wherever the
receiver's white phase noise dominates -- on the bench, the whole
measured range; the plain form is kept because data sheets quote it.  The GPS
receiver's 1 PPS is quantized to its own crystal, and while locked
the OCXO is steered to follow it, so the curve is of the pair and of
the loop between them rather than of the OCXO alone; the page says
so.  Gaps are not filled in: the run is cut where a relock, a
holdover or an absence makes the phase either side incomparable, only
the second differences that exist are counted, and the curve carries
the number of readings, holes and unbroken runs behind it so it can be
judged.  `PLAN.md` has what that took.

The browser view is read-only and is not the monitor in a window: it
draws what a terminal cannot, which is mainly history you can drag to
zoom, and the polar sky plot at `/sky`.

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
shared link shows the same window.  The stability page uses the same
control; there a range is the record the estimator runs on, so
changing it recomputes.

The chart library comes from a CDN, pinned with an integrity hash, so
the page needs internet even though the daemon does not; the page says
so rather than showing an empty frame if it cannot be fetched.

The exporter answers a scrape from whatever each daemon last polled,
so scraping costs the receivers nothing and cannot compete with the
poll schedule.  Every sample is labelled with the daemon instance and
the receiver's serial and model.  It exports `smartclock_up`, and the age of each tier as
`smartclock_tier_age_seconds`, because a daemon that has stopped polling
otherwise looks like a remarkably steady oscillator: every other value
stays exactly where it was.  A reading the receiver declined is left out
rather than exported as zero, and so is every value whose tier is
failing or whose link is down: the last number read, exported as
though current, is what a panel would go on drawing.

The daemon also copies out what the receiver writes down for itself, on
every connection rather than once per unit, because a connection is the
boundary across which nothing is known.  Its error queue is drained and
every entry recorded -- reading an entry is what removes it, the queue
holds thirty and discards the newest when it overflows, so errors
raised while the daemon was down are both the ones nobody else will
read and the first to be lost.  Its diagnostic log, 222 entries deep
and no deeper, is copied entry by entry, newest first and then
backwards through whatever was already there, resuming from the
database so a restart does not start over.

It watches the receiver's alarm without taking it.  The event
registers hold the transitions worth knowing about -- Time Reset among
them, the receiver quietly stepping its own clock because it disagreed
with the satellites, which invalidates every interval measurement
across the step and appears in no condition register.  But reading an
event register clears it, and clearing the events extinguishes the
front-panel Alarm LED and the BITE output, because the alarm
summarises them.  **That lamp belongs to whoever is standing at the
instrument**, so the daemon never reads an event register.

It polls `*STB?` instead, the alarm condition register, which reports
the same latched state in real time and changes nothing.  The cost is
that it names the group rather than the bit -- except for the
questionable group, which holds only Time Reset and a bit nothing here
sets, so that one names itself.  Changes are recorded as they happen
and shown in the monitor's header, the browser's status strip and
`smartclock_alarm`; the lamp stays lit until you clear it at the
receiver.  The transition filters are recorded too, because this unit
latches faults appearing and never clearing, so a missing clear means
only that nobody enabled that transition.

`--adopt-log` lets the daemon erase the receiver's log once every entry
is copied here and the receiver says it is nearly full.  Off by
default: erasing is irreversible and non-volatile, and on a unit
somebody is investigating that log is evidence.  The entry count is
passed with the command, so the receiver refuses if one arrived in
between.

Every row says which receiver it came from.  A `receiver` table holds
one row per unit that has written to the file, keyed on the serial from
`*IDN?` -- the serial alone, because firmware changes under it and an
upgrade is not a different instrument -- and the snapshots, satellites,
errors, diagnostic log entries and audit trail all carry its id.  A
bench where units are swapped otherwise accumulates two oscillators'
history in one file with no way to tell the rows apart, which makes
every long-run comparison in it a comparison between two crystals.
The browser's history and stability pages show one receiver at a time,
chosen by a selector that appears once a file holds more than one; the
choice is carried in the address as `?receiver=<id>`, so it survives a
reload and moving between pages.

`smartclockmon --device ...` talks to the receiver directly, which needs
the daemon stopped and records no history; the header says so.

With no receiver to hand, the simulator answers in its place:

    smartclock-sim 127.0.0.1:5025
    smartclockd     --device tcp://127.0.0.1:5025 ...
    smartclock-cli  --device tcp://127.0.0.1:5025 diagnose

Every tool takes `tcp://host:port` wherever it takes a device path, so
the same command that talks to the receiver talks to the simulator, or
to a serial adapter behind ser2net.

`--faulty` gives one with its oscillator control near the rail, for the
paths that only run when something is wrong.

The daemon refuses anything that changes the receiver unless started
with `--allow-control` -- which includes reading an event register,
since that clears it, and reading the error queue, since that takes
the entry the daemon's journal would have kept -- refuses what can
strand the link without `--allow-dangerous`, and refuses commands the
table does not know without `--allow-raw`.  All three are off by
default and every command that is not a scheduled poll is recorded in
the log.  Every option also reads from a `SMARTCLOCKD_`-prefixed
environment variable, which is how the service is configured without
touching its unit.

`smartclock-cli` talking to the receiver directly has no such flags,
and refuses outright to send `:SYSTem:PRESet`, the undocumented
`:SYSTem:PON`, anything under `:SYSTem:COMMunicate`,
`:DIAGnostic:ERASe`, or a `:SYSTem:LANGuage` setting, before it opens
the port.

## Hardware

Target family is the HP / Agilent / Symmetricom SmartClock receivers.
These are GPS-disciplined double-oven OCXO references that emit 10 MHz
and 1 PPS, and report state over a serial port using SCPI.

| Model  | Command tree             | Notes                                   |
| ------ | ------------------------ | --------------------------------------- |
| 58503A | `:GPS:`, `:SYNC:`        | Primary development target              |
| 58503B | `:GPS:`, `:SYNC:`        | Same tree as the 58503A                 |
| 59551A | `:GPS:`, `:SYNC:`        | Adds pulse output and event timestamping|
| Z3801A | `:PTIME:GPSYSTEM:`, `:ROSC:` | Divergent tree; different response formats |
| Z3805A | `:PTIME:GPSYSTEM:`, `:ROSC:` | Answers the Z3801A tree; verified on hardware |
| Z3816A | `:PTIME:GPSYSTEM:`, `:ROSC:` | Assumed as Z3801A; unverified          |

These receivers' GPS engines are mid-1990s Motorola boards whose
firmware predates the GPS week rollovers of 1999 and 2019, so a unit
reports a date 1024 weeks in the past.  Time of day, 1 PPS and 10 MHz
are unaffected, and `smartclock` corrects the date rather than
presenting it as a fault.  `:DIAGnostic:IDENtification:GPSystem?`
names the engine on both command trees.

Which receivers are on hand, what they answer `*IDN?` with and how
they are cabled are facts about one bench rather than about this
project, so they live in `BENCHNOTES.md` and not here.

Both report `:DIAGnostic:IDENtification:GPSystem?`, so the pair can be
compared directly rather than inferred.  Note what it does not settle:
on one antenna the 58503A holds seven to ten satellites while the
Z3805A holds two to four, and nothing measured explains it.  Antenna,
antenna bias at 4.8 V against a 4.5 V specification, supply rails,
asserted position and the whole health monitor all check out on both.

Factory default for all models is 9600 8N1, no pacing, full duplex.
Serial settings are stored in the receiver and survive a power cycle, so
a unit may not be at the default.  `:SYSTem:COMMunicate:SERial1:PRESet`
restores them.

## Protocol

SCPI over RS-232, but not a VISA-style instrument.  The receiver behaves
as an interactive terminal:

- It echoes received characters, one at a time as they arrive.
- It prompts with `scpi > `, or `E-nnn > ` when the previous command
  raised an error.  Note the space before the angle bracket: the manuals
  render the prompt `scpi>`, but the wire carries `scpi > `.
- The prompt's trailing space often arrives after the rest of the
  prompt, so it turns up at the head of the next reply.
- Abandoning a reply part-read leaves the receiver still sending.  The
  next prompt seen then belongs to the abandoned reply, and every
  exchange after it reads one reply behind, so a session must drain
  before resynchronising.
- `:SYSTem:STATus?` returns a multi-line formatted ASCII status screen
  rather than a SCPI response.  `:SYSTem:STATus:LENGth?` gives that
  screen's line count.
- Per-satellite elevation, azimuth and C/N appear only in that status
  screen.  No SCPI query returns them, and the command tree extracted
  from the firmware has no per-satellite node at all.  The counts are
  the exception: `:GPS:SATellite:TRACking:COUNt?` is the screen's
  `Tracking`, and `:GPS:SATellite:VISible:PREDicted:COUNt?` less that
  is its `Not Tracking`.
- The health monitor line is the hardware condition register rendered
  coarsely, so it says nothing the register does not say in more
  detail.

Error reporting and the status registers do follow the standards:
`:SYSTem:ERRor?` returns the conventional `<code>,"<description>"`, and
the status register structure is IEEE 488.2.

## Documentation

Vendor manuals in `third_party/`:

| File               | Contents                                            |
| ------------------ | --------------------------------------------------- |
| `097-59551-02`     | 59551A/58503A Operating and Programming Guide.  The primary reference: command reference, status screen fields, status registers, error codes. |
| `097-58503-13`     | 58503B/59551A Operating and Programming Guide       |
| `097-58503-12`     | 58503B/59551A Getting Started Guide                 |
| `097-58503-08`     | 58503A Option 001 front-panel supplement            |
| `097-z3801-01`     | Z3801A User's Guide; documents the Z3801A tree      |
| `58503a-01`        | Oscillator bracket service note                     |

`10811-variants-90027-1.pdf` and `HP-10811AB-Manual.pdf` specify the
oscillator.

`docs/commands.md` is the command table rendered readably: which
commands each tree has, how far each is confirmed, and the eleven that
appear in no manual.  It is generated by `make docs` and a test fails if
it and the table disagree.

The rest of `docs/`:

| File | Contents |
| ---- | -------- |
| `running.md` | installing, configuring and what the daemon does to the receiver |
| `efc.md` | how the receiver reports its control voltage, measured at the oscillator's EFC pin |
| `OCXO.md` | the oscillator itself |
| `z3801-keywords.md` | the 313 SCPI keywords the Z3801A firmware defines, and the undocumented commands found by sweeping a 58503A with them |
| `screen-format-strings.md` | the status screen's printf templates |
| `z3801-tree.md` | every SCPI command path in the Z3801A firmware, read from the parser's tables |
| `58503a-tree.md` | the same for the 58503A firmware, revision 3633, checked against the command table and the Z3801A tree |
| `firmware.md` | what the Z3816A firmware shows: how it measures the 1 PPS time interval, the loop that disciplines the oscillator from it, where the Oncore's sawtooth goes, and its pForth console |
| `hardware-investigations.md` | what `firmware.md` leaves open that only a bench can settle: what to measure and what each answer changes |
| `loop.html` | the disciplining loop as a block diagram, with its update law, constants and closed-loop poles; a standalone page, open it in a browser |

## Licence

GPL-3.0-or-later.  See `LICENSE`.

The vendor manuals in `third_party/` are not covered by it; they remain
the copyright of Symmetricom and its successors and are kept here as
protocol documentation.  See `third_party/NOTICE`.
