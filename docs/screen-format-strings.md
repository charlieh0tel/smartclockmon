# Status screen format strings

The Z3801A and the 58503A do not share a firmware build, but the
layout, the field widths and the set of variant strings are the same,
and this enumerates them exactly rather than leaving them to be guessed
from the handful of screens in the manuals.

This is the screen `:SYSTem:STATus?` returns over serial, and nothing
else.  The 58503A could be ordered with a front panel display, which
the Z3801A and Z3805A never had; it is separate hardware driven by its
own strings, much shorter and in capitals, and none of them are below.
A 58503A reading `10MHZ STABLE` on its panel says nothing about this
screen -- captured from the same unit at the same moment, the mode line
read `>> Locked to GPS` with no suffix at all.  The panel's strings
would come from a 58503A firmware image, which is not in
`third_party/`.

Where the two do differ is narrower than a different string set.  The
1 PPS markers are composed, `%s %s` over a label: the firmware holds
`GPS 1PPS` and `1PPS CLK` separately, so a Z3801A reads
`[ GPS 1PPS CLK Valid ]` where a 58503A, which has no external 1 PPS
input to disambiguate from, reads `[ GPS 1PPS Valid ]`.  The captured
58503A screen is otherwise string for string what the Z3801A firmware
holds.

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

Once the PLL has settled the suffix goes and FFOM reads 0.

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
through `fine freq adj` and into `stabilizing frequency` -- on its
front panel, not on this screen, so it corroborates the ladder and its
order but not the exact spellings.

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

## Section markers

Each banner is dotted out to the right margin and closed with a
bracket:

    SYNCHRONIZATION .....  [ Outputs Valid ]
                           [ Outputs Invalid ]
                           [ Outputs Valid/Reduced Accuracy ]
    ACQUISITION .........  [ GPS 1PPS CLK Valid ]     'CLK' per the note above
                           [ GPS 1PPS CLK Invalid ]
                           [ EXT 1PPS CLK Valid ]
                           [ EXT 1PPS CLK Invalid ]
    HEALTH MONITOR ......  [ OK ]
                           [ ERROR ]

## Health monitor

One line of named tests, each `%s: %-3s%s`, reading `OK` or `Err`:

    Self Test    Int Pwr    Oven Pwr    OCXO    EFC    GPS Rcv

## Other

    ELEV MASK %2d deg%3s%-25.25s%2s
    ELEV MASK  %-2d%33s%s
    *attempting to track     footnote; the PRN it refers to is starred
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
