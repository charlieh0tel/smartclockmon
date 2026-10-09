# Bench notes

What is on this bench, and what it has been seen doing.

This goes stale when hardware is swapped, so it is not in `README.md`,
`PLAN.md` or `AGENTS.md`, and no code should depend on it.  A running
daemon is the better source: `smartclock-cli --daemon ... diagnose`
heads its report with the model, serial and firmware of whatever is
attached, and every log row names its receiver.

## HP Z3801A, serial 3542A01548

On the bench but not connected.  Its history is in
`docs/hardware-investigations.md`, items 11 and 12.

## HP 58503A, serial 3710A01056

    HEWLETT-PACKARD,58503A,3710A01056,3704-C

Option 001: front-panel display and keypad.  19200 8N1, its fastest:
asked for 38400 it replies `+0,"No error"` and stays at 19200.
Oscillator: HP 10811-60159, serial 3505A15217; see `docs/ocxo.md` for the specification
and `docs/efc.md` for the EFC measurements taken on it.  Time zone
offset `+0,+0` (`:PTIMe:TZONe?`, read 2026-09-23), so its dates and
times are UTC.

GPS receiver, as `:DIAGnostic:IDENtification:GPSystem?` reports it:

    COPYRIGHT 1991-1996 MOTOROLA INC.
    SFTW P/N # 98-P36830P
    SOFTWARE VER # 8
    SOFTWARE REV # 8
    SOFTWARE DATE  06 Aug 1996
    MODEL #    B4121P1115
    HDWR P/N # _
    SERIAL #   SSG0220999
    MANUFACTUR DATE 7D01
    OPTIONS LIST    IB

Its diagnostic log had been full at 222 entries, and so not
recording, since March 2025; the entries were copied out and the log
cleared, so it records again.  It showed nineteen holdover and relock
cycles over two days in March 2025 and nothing since -- GPS reception,
not a failing crystal.

## HP Z3805A, serial 3625A01487

    HEWLETT-PACKARD,Z3805A,3625A01487,3543B-A

Found at 9600 8N1, moved to 19200 to match the other unit.  No
front-panel display -- the Z3801A and Z3805A never had one -- so the
serial status screen is the only status output.

Takes the Z3801A command tree, selected by the `Z38` prefix.  Every
read-only command in that tree answered; the sixteen entries added to
it in September 2026 were confirmed here.

GPS receiver, as `:DIAGnostic:IDENtification:GPSystem?` reports it:

    COPYRIGHT 1991-1995 MOTOROLA INC.
    SFTW P/N # 98-P39972M
    SOFTWARE VER # 8   REV # 4   DATE 13 JUL 1995
    MODEL #    B1121P1114
    SERIAL #   SSG0163878
    MANUFACTUR DATE 6G09
    OPTIONS LIST    IB

A different engine from the 58503A's B4121P1115: a six-channel B1
(`bench-sky-2026-10-03.html`, from the VP Oncore Command Reference)
with software a year older (`firmware/gps.md`, "The engines on the
bench").  Both report the same 1024-week offset and, on the same day,
the same date.

Its diagnostic log held 225 entries, not the 222 the 58503A stops at.
Copied out and cleared.

Position asserted by hand rather than surveyed, and survey-on-powerup
turned off so it survives a power cycle.  **Both settings are
non-volatile.**  If that antenna moves, this receiver will not notice
and will serve time from a wrong position.

## The Z3805A's receive path is degraded

It knows where every satellite is and cannot hear them.  Its predicted
elevations and azimuths match what the 58503A tracks, PRN for PRN, on
the same antenna minutes apart:

    PRN     Z3805A predicted      58503A tracking, with signal
              1     22  314         23  314    74
              2     45  310         46  310   152
              8     29  256         29  254    79
             10     47   60         46   61   137
             23     18   81         17   83    62
             27     23  214         22  214   105
             28     32  148         33  148   103
             32     78   35         76   35   250

So the almanac is sound and it points correctly.  On the same cable
and adapter the 58503A reads 62 to 250; the Z3805A never exceeded 29
in a day of watching, on either of two antennas.  Even if the two
engines scale signal differently, it hears roughly an order of
magnitude less.

Everything else is eliminated, each by measurement against the 58503A
on the same cables: both antennas, the distribution amp
drop, the cable, the adapter, the antenna bias (4.8 V at the N
connector, "a little less than +5 Volts" as `097-z3801-01` 2-12 says
it should read), the supply (28 V at 3 A, no rail fault bit in ten
hours), the asserted position (reads back correct, agrees with the
58503A's survey to within a meter), the elevation mask (10 degrees,
nothing ignored, all 32 included), and the health monitor (six of six
OK throughout).

It is not dead: over about five hours it acquired enough to download
an almanac, validate its time and reach `Locked` with TFOM 3.  An
almanac download needs sustained data lock, which a deaf receiver
cannot manage.  It is degraded, not broken, matching a time-nuts report
of another Z3805A whose Oncore lost sensitivity over time.

Later results (`hardware-investigations.md`, item 11):

- 2026-09-28, overnight after `master_reset`, a power cycle and 20 dB
  of low-noise gain ahead of it: lock about 98% of nine hours on 2 to
  4 satellites.
- 2026-10-03, 24 hours on one splitter beside the 58503A and a u-blox
  NEO-M8T, with that gain ahead of it alone: 2.9 satellites on average, none held below 39 dB-Hz where
  the 58503A held 88% at 36 to 39
  ([bench-sky-2026-10-03.html](https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/bench-sky-2026-10-03.html)).
- The 24 hours to 21:00 UTC on 2026-10-05, with the 20 dB LNA still in
  place: lock 88.3% of the time, on about 2.7 satellites on average
  while locked.

In position hold at the 58503A's surveyed antenna position, which it
read back on 2026-09-28, with survey-on-powerup off (read 0 on
2026-10-05), so a power cycle keeps the position.
