# Bench notes

What is actually on this bench, and what it has been observed doing.

Everything here goes stale the moment hardware is swapped, which is why
it is not in `README.md`, `PLAN.md` or `CLAUDE.md`.  Nothing in the
code should depend on any of it.  A running daemon is always the better
source: `smartclock-cli --socket ... diagnose` heads its report with
the model, serial and firmware of whatever is attached, and every row
in the log says which receiver it came from.

## HP 58503A, serial 3710A01056

    HEWLETT-PACKARD,58503A,3710A01056,3704-C

Option 001, so it has the front-panel display and keypad.  19200 8N1.
Oscillator is an HP 10811-60159; see `docs/OCXO.md` for the
specification and `docs/efc.md` for the EFC measurements taken on it.

GPS engine: Motorola, `MODEL # B4121P1115`, `SOFTWARE DATE 06 Aug 1996`.

Its diagnostic log had been full at 222 entries since March 2025 and
had therefore stopped recording; the entries were copied out and the
log cleared, so it records again.  That history showed nineteen
holdover and relock cycles over two days in March 2025 and nothing
since -- GPS reception rather than a failing crystal.

## HP Z3805A, serial 3625A01487

    HEWLETT-PACKARD,Z3805A,3625A01487,3543B-A

Found at 9600 8N1, moved to 19200 to match the other unit.  No front
panel display -- the Z3801A and Z3805A never had one, so the serial
status screen is the only status output.

Takes the Z3801A command tree, selected by the `Z38` prefix.  Every
read-only command in that tree answered; the sixteen entries added to
it in September 2026 were confirmed here.

GPS engine: Motorola, `MODEL # B1121P1114`, `SOFTWARE DATE 13 Jul
1995` -- one digit and one year from the 58503A's, which is why both
report the same 1024-week offset and, on the same day, the same date.

Its own diagnostic log held 225 entries, not the 222 the 58503A stops
at.  Copied out and cleared.

Position was asserted by hand rather than surveyed, and
survey-on-powerup turned off so it survives a power cycle.  **Both
settings are non-volatile.**  If that antenna moves, this receiver will
not notice and will serve time from a position that is no longer true.

## Wiring

Both receivers present a DB-25 and take the same lead and the same
USB-serial adapter, so swapping which one is monitored is a matter of
moving one cable:

    /dev/serial/by-id/usb-Prolific_Technology_Inc._USB-Serial_Controller_D-if00-port0

Both are at 19200 8N1, so nothing needs reconfiguring when they swap.
Only one can be monitored at a time: the daemon serves one device, and
there is one adapter.

## Unexplained

On one antenna the 58503A holds seven to ten satellites while the
Z3805A holds two to four, and drops lock every few minutes where the
other stays locked for days.

Ruled out, each by measurement rather than argument: the antenna (the
58503A works on it), the Z3805A's antenna bias (4.8 V at the N
connector against a 4.5 V specification -- `097-58503-13` Antenna Power
Verification), its supply (28 V at 3 A, and no rail fault bit in nine
hours), its asserted position (reads back correct and agrees with the
58503A's survey to within a metre), the elevation mask (10 degrees on
both, nothing ignored, all 32 included) and its health monitor (all
six items OK throughout).

Signal strengths on the Z3805A have never exceeded 29 and mostly sit
between 19 and 28.  Whether that is comparable to the 58503A's 35 to
128 is unknown: the engines differ and the scale is undocumented.
