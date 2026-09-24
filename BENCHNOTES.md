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

Option 001, so it has the front-panel display and keypad.  19200 8N1,
which is as fast as it goes: asked for 38400 it replies `+0,"No error"`
and stays at 19200.
Oscillator is an HP 10811-60159; see `docs/OCXO.md` for the
specification and `docs/efc.md` for the EFC measurements taken on it.
Time zone offset `+0,+0` (`:PTIMe:TZONe?`, read 2026-09-23), so the
dates and times it reports are UTC.

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

GPS receiver, as `:DIAGnostic:IDENtification:GPSystem?` reports it:

    COPYRIGHT 1991-1995 MOTOROLA INC.
    SFTW P/N # 98-P39972M
    SOFTWARE VER # 8   REV # 4   DATE 13 JUL 1995
    MODEL #    B1121P1114
    SERIAL #   SSG0163878
    MANUFACTUR DATE 6G09
    OPTIONS LIST    IB

One digit and one year from the 58503A's, which is why both report the
same 1024-week offset and, on the same day, the same date.

Its own diagnostic log held 225 entries, not the 222 the 58503A stops
at.  Copied out and cleared.

Position was asserted by hand rather than surveyed, and
survey-on-powerup turned off so it survives a power cycle.  **Both
settings are non-volatile.**  If that antenna moves, this receiver will
not notice and will serve time from a position that is no longer true.

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

So the almanac is sound and it is pointing correctly.  On the same
cable and adapter the 58503A reads 62 to 250 where the Z3805A has
never exceeded 29 in a day of watching, on either of two antennas.
Even allowing that two engines may scale signal differently, it hears
roughly an order of magnitude less.

Everything else is eliminated, each by measurement against the 58503A
as a control on the same cables: both antennas, the distribution amp
drop, the cable, the adapter, the antenna bias (4.8 V at the N
connector against a 4.5 V specification -- `097-58503-13` Antenna Power
Verification), the supply (28 V at 3 A, no rail fault bit in ten
hours), the asserted position (reads back correct, agrees with the
58503A's survey to within a metre), the elevation mask (10 degrees,
nothing ignored, all 32 included), and the health monitor (six of six
OK throughout).

What it can still do is the reason not to call it dead: over about
five hours it acquired enough to download an almanac, validate its
time and reach `Locked` with TFOM 3.  Downloading an almanac needs
sustained data lock, which a deaf receiver cannot manage.  It is
degraded, not broken -- which matches a time-nuts report of another
Z3805A whose Oncore lost sensitivity over time.

Left in position hold at the surveyed antenna position with
survey-on-powerup off, so it serves time from its oscillator whether
or not it ever tracks.
