# Status screen format strings

The Z3801A and the 58503A do not share a firmware build.  The layout
and the field widths are the same, and this enumerates the strings
exactly rather than leaving them to be guessed from the handful of
screens in the manuals.

The string sets are *not* the same, and every entry below is read out
of Z3801A firmware unless marked otherwise.  A 58503A running 3704-C
displays `10MHZ STABLE`, in capitals, once the PLL has settled --
where the Z3801A firmware has no occurrence of `MHZ` in any casing.
Treat the lists as the Z3801A's, corroborated on a 58503A where a
note says so.

## Frame

    ------------------------------- Receiver Status -------------------------------
    SmartClock Mode ___________________________   Reference Outputs _______________
    Holdover Uncertainty ____________
    Position ________________________
    PRNs Ignored ___________________

The underscores are literal, not a rendering artefact of the manuals.
The frame is 79 columns.

## Satellite table

    Tracking: %d        Not Tracking: %-2d%11s
    PRN  El  Az   SS
    PRN  El  Az

Two header variants, and the signal column is what distinguishes the
tracked group from the untracked one.  Cell contents:

    %2d %3d          elevation and azimuth
     %2d             signal
    -- ---           no data
    Acq              acquiring, three animation frames:
    Acq .
    Acq ..

## Reference outputs

    TFOM     %d%13sFFOM    %2s
    1PPS TI %s
    HOLD THR %s
     Off
    [TI %9.9s]
    relative to GPS      width-dependent variants of the same suffix
    rel to GPS
    rel GPS

## Mode suffixes

Appended to the mode marked `>>`.  Long and short forms are chosen by
available width:

    : stabilizing frequency          FFOM 1

Once the PLL has settled the suffix goes, and FFOM reads 0.  A 58503A
shows `10MHZ STABLE` at that point; the Z3801A firmware has no such
string.

Why it is in holdover.  These are the same distinctions
`:SYNChronization:HOLDover:WAITing?` answers with `GPS`, `LIMit` and
`HARDware`, and the first of them is the one the register bits do not
make: a manual holdover is what sets Holding, bit 0:

    : manually initiated
    : GPS 1PPS CLK invalid
    : EXT 1PPS CLK invalid
    : 1PPS TI exceeds hold threshold
    : internal hardware problem

The ladder up to lock, in the order a receiver climbs it:

    : OCXO warm-up
    : GPS acquisition
    : coarse freq adj            : coarse frequency adjustment
    : fine freq adj              : fine frequency adjustment
    : phase alignment
    : leap second determination

A 58503A recovering from a ninety-second antenna outage was watched
through `fine freq adj` and into `stabilizing frequency`, so the
58503A uses this set despite the strings being read out of Z3801A
firmware.

## Synchronization status

    Synchronized to UTC
    Synchronized to GPS Time
    Assessing stability          plus '.', '..', '...' frames
    [?]                          appended to a time that is suspect
    Invalid: not tracking
    Invalid: GPS rcvr err
    Invalid: inacc position
    Invalid: Time RAIM error
    Absent or freq error

## Position

    MODE
    Navigation
    Hold
    Survey:              with '    0', ' <0.1', '>99.9', '%5.1f', '% complete'
    %9sSuspended: %13.13s
    no track data
    track <4 sats
    poor geometry
    INIT                 prefixes on LAT / LON / HGT
    AVG
    %+9.2f m  (MSL)

## Time

    %02d:%02d:%02d
    --:--:--
    %02d %s %04d
    -- --- ----
    LOCL GPS             which scale the displayed time is on
    LOCAL

## Other

    ELEV MASK %2d deg%3s%-25.25s%2s
    ELEV MASK  %-2d%33s%s
    ANT DLY  %s
    1000+ hr

## Time code

Confirms the two documented formats character for character:

    T1#H%08X%01d%01d%1c%1c%1c
    T2%04d%02d%02d%02d%02d%02d%01d%01d%1c%1c%1c

Note that the leap, service-request and validity fields are `%1c`, not
digits, which is why the leap field can carry `+` or `-`.

## Identity

    HEWLETT-PACKARD,%s,%s,%s-%c
