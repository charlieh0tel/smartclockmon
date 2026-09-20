# smartclockmon

A Rust library and terminal monitor for HP / Symmetricom SmartClock GPS
time and frequency reference receivers, spoken to over RS-232.

## Status

Early.  No code yet -- the repository currently holds vendor
documentation and firmware only.  See `PLAN.md`.

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

Its oscillator is an HP 10811-60159, and its GPS engine is a Motorola
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

`the keyword table` and `the keyword table` are 512 KB firmware, kept as
a reference for resolving command trees that the manuals do not cover.

## Licence

GPL-3.0-or-later.  See `LICENSE`.

The vendor manuals and firmware in `third_party/` are not covered
by it; they remain the copyright of Symmetricom and its successors and
are kept here as protocol documentation.
