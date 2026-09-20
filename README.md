# smartclockmon

A Rust library and terminal monitor for HP / Symmetricom SmartClock GPS
time and frequency reference receivers, spoken to over RS-232.

## Status

Working against the development receiver.  The library, the daemon, the
monitor and a simulator all run.  See `PLAN.md` for what is done, what
is not, and the defects a review has found but nobody has fixed yet.

## Parts

| | |
| - | - |
| `smartclock` | the library: transports, SCPI framing, the command table, parsers, the status screen scraper, and the polling task |
| `smartclockd` | holds the serial port, logs to SQLite, serves clients over a local socket |
| `smartclockmon` | terminal monitor: a dashboard and history graphs |
| `smartclock-cli` | one-shot queries, `diagnose`, transcript capture, and sweeping for undocumented commands |
| `smartclock-sim` | a simulated receiver, in process for tests and over TCP for driving the real daemon |

## Running it

Build with `make`, or `make deb` for a package.

The daemon holds the port, so everything else is a client of it:

    smartclockd --device /dev/serial/by-id/usb-... \
                --database ~/smartclock.sqlite \
                --socket /tmp/smartclockd.sock

    smartclockmon --socket /tmp/smartclockd.sock

In the monitor, `g` switches to the graphs, `w` cycles their span, `c`
opens a command line, `u` falls back to ASCII, `q` quits.

`smartclockmon --device ...` talks to the receiver directly, which needs
the daemon stopped and records no history; the header says so.

With no receiver to hand, the simulator answers in its place:

    smartclock-sim 127.0.0.1:5025
    smartclockd --device tcp://127.0.0.1:5025 ...

`--faulty` gives one with its oscillator control near the rail, for the
paths that only run when something is wrong.

The daemon refuses anything that changes the receiver unless started
with `--allow-control`, refuses what can strand the link without
`--allow-dangerous`, and refuses commands the table does not know
without `--allow-raw`.  All three are off by default and every command
that is not a scheduled poll is recorded in the log.

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
| Z3816A | `:PTIME:GPSYSTEM:`, `:ROSC:` | Assumed as Z3801A; unverified          |

The development unit is a 58503A with Option 001 (front-panel
display/keypad), at 19200 8N1 on

    /dev/serial/by-id/usb-Prolific_Technology_Inc._USB-Serial_Controller_D-if00-port0

which is stable across re-enumeration, unlike `/dev/ttyUSB0`.  Its rear
serial port is DB-25; the 58503B uses DB-9.  It answers `*IDN?` with

    HEWLETT-PACKARD,58503A,3710A01056,3704-C

Its oscillator is an HP 10811-60159 (see `docs/OCXO.md`), and its GPS
engine is a Motorola
reporting `SOFTWARE DATE 06 Aug 1996`.  That engine is why the receiver
reports a date 1024 weeks in the past: its firmware predates the GPS
week rollovers of 1999 and 2019.  Time of day, 1 PPS and 10 MHz are
unaffected.

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

`the keyword table` and `the keyword table` are 512 KB firmware, kept as
a reference for resolving command trees that the manuals do not cover.

`docs/commands.md` is the command table rendered readably: which
commands each tree has, how far each is confirmed, and the eleven that
appear in no manual.  It is generated by `make docs` and a test fails if
it and the table disagree.

Other notes are in `docs/`: `OCXO.md` for the
oscillator, `efc.md` for how the receiver reports its control voltage,
`z3801-keywords.md` for the SCPI vocabulary and the undocumented
commands found by sweeping, and `screen-format-strings.md` for the
status screen's layout.

## Licence

GPL-3.0-or-later.  See `LICENSE`.

The vendor manuals and firmware in `third_party/` are not covered
by it; they remain the copyright of Symmetricom and its successors and
are kept here as protocol documentation.
