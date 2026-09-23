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
| `smartclock` | the library: transports, SCPI framing, the command table, parsers, the status screen scraper, and the polling task |
| `smartclockd` | holds the serial port, logs to SQLite, serves clients over a local socket |
| `smartclockmon` | terminal monitor: a dashboard and history graphs |
| `smartclock-cli` | one-shot queries, `diagnose`, transcript capture, and sweeping for undocumented commands |
| `smartclock-exporter` | Prometheus metrics for the receiver, from the daemon's own readings |
| `smartclock-web` | a browser view: live state, a sky plot, and history you can zoom |
| `smartclock-sim` | a simulated receiver, in process for tests and over TCP for driving the real daemon |

## Running it

Build with `make`, or `make deb` for a package.  Installed from the
package, the daemon is configured entirely through
`/etc/default/smartclockd` and started with `systemctl enable --now
smartclockd`; see `docs/running.md`.

Run by hand, it holds the port and everything else is a client of it:

    smartclockd --device /dev/serial/by-id/usb-... \
                --database snapshots.sqlite \
                --socket /tmp/smartclockd.sock

    smartclockmon --socket /tmp/smartclockd.sock

In the monitor, `g` cycles the views, `l` jumps to the journal, `w`
cycles the graph span, `c` opens a command line, `q` quits.  The
journal is what the receiver has recorded about itself -- its
diagnostic log, its error queue, and the changes in its alarm -- none of
which is in the snapshot table or can be plotted, and all of which the
browser view shows too.

    smartclock-exporter            # http://127.0.0.1:9979/metrics

    smartclock-web                 # http://127.0.0.1:9980/

The GPS week rollover is treated as the ordinary condition it is.
Firmware predating the 2019 wrap reports a date 1024 weeks behind, and
nearly every receiver of this vintage does, so the clients show the
corrected date with the correction noted quietly rather than raising a
warning that would be lit permanently on a healthy instrument.  What
the receiver actually said stays visible beside it, because the
correction is arithmetic done here against the host clock, not
something the receiver reported.

The browser view is read-only and is not the monitor in a window: it
draws what a terminal cannot, which is mainly a polar sky plot and
history you can drag to zoom.

Any number of series can be stacked, and they share a time axis by
construction rather than by appearance: one request buckets them all in
the same pass, so their x values are the same values, and separate
requests -- which would each compute their own bucket boundaries from
their own end time -- could not promise that.  The cursor moves across
the stack together and a drag on any plot zooms all of them.  EFC
against internal temperature is the pairing that earns its keep; see
`docs/efc.md` for what that comparison settled.

The chart library comes from a CDN, pinned with an integrity hash, so
the page needs internet even though the daemon does not; the page says
so rather than showing an empty frame if it cannot be fetched.

The exporter answers a scrape from whatever the daemon last polled, so
scraping costs the receiver nothing and cannot compete with the poll
schedule.  It exports `smartclock_up`, and the age of each tier as
`smartclock_tier_age_seconds`, because a daemon that has stopped polling
otherwise looks like a remarkably steady oscillator: every other value
stays exactly where it was.  A reading the receiver declined is left out
rather than exported as zero.

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
with `--allow-control`, refuses what can strand the link without
`--allow-dangerous`, and refuses commands the table does not know
without `--allow-raw`.  All three are off by default and every command
that is not a scheduled poll is recorded in the log.  Every option also
reads from a `SMARTCLOCKD_`-prefixed environment variable, which is how
the service is configured without touching its unit.

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

The 58503A's rear serial port is DB-25; the 58503B uses DB-9.

These receivers' GPS engines are mid-1990s Motorola boards whose
firmware predates the GPS week rollovers of 1999 and 2019, so a unit
reports a date 1024 weeks in the past.  Time of day, 1 PPS and 10 MHz
are unaffected, and `smartclock` corrects the date rather than
presenting it as a fault.  `:DIAGnostic:IDENtification:GPSystem?`
names the engine on both command trees.

Which receivers are actually on hand, what they answer `*IDN?` with
and how they are wired are facts about one bench rather than about
this project, so they live in `BENCHNOTES.md` and not here.

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
  screen.  No SCPI query returns them.

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
| `efc.md` | how the receiver reports its control voltage, and what three meter readings settled about it |
| `OCXO.md` | the oscillator itself |
| `z3801-keywords.md` | the 313 SCPI keywords the Z3801A firmware defines, and the undocumented commands found by sweeping a 58503A with them |
| `screen-format-strings.md` | the status screen's printf templates |

## Licence

GPL-3.0-or-later.  See `LICENSE`.

The vendor manuals in `third_party/` are not covered by it; they remain
the copyright of Symmetricom and its successors and are kept here as
protocol documentation.  See `third_party/NOTICE`.
