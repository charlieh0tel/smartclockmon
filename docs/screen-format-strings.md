# Status screen format strings

The Z3801A and the 58503A do not share a firmware build, but the
layout, the field widths and the set of variant strings are the same,
and this enumerates them exactly rather than leaving them to be guessed
from the handful of screens in the manuals.

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

    : stabilizing frequency
    : manually initiated
    : GPS 1PPS CLK invalid
    : OCXO warm-up
    : GPS acquisition
    : coarse freq adj            : coarse frequency adjustment
    : fine freq adj              : fine frequency adjustment
    : phase alignment
    : leap second determination

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
