# Status screen format strings

The templates of the screen `:SYSTem:STATus?` returns, as the
Z3801A image (`third_party/z3801a-3543.bin`) holds them.  The 58503A
images (`58503a-3633.bin`, `58503a-3704.bin`) have the same layout and
field widths; where their strings differ is noted below.

The 58503A could be ordered with a front panel display, which the
Z3801A and Z3805A never had.  Its strings are shorter, in capitals, and
not part of this screen; in 3704 they sit at `0x53b82` on: `HLD USR`,
`HLD LIM`, `HLD GPS`, `HOLDOVR`, `FINE F ADJ`, `PHASE ALIGN`,
`HLD RECVR`, `STABILIZING`, `10MHZ STABLE`, `NAV MODE`, `SURVEY HALT`.
A 58503A showing `10MHZ STABLE` on its panel had, at the same moment,
`>> Locked to GPS` on this screen with no suffix.

The 1 PPS strings differ between the families.  The Z3801A image
holds `GPS 1PPS CLK` and `EXT 1PPS CLK` forms; the 58503A images hold
`GPS 1PPS` and `Ext 1PPS`:

| Z3801A 3543 | 58503A 3633, 3704 |
| ----------- | ----------------- |
| `[ GPS 1PPS CLK Valid ]` | `[ GPS 1PPS Valid ]` |
| `[ EXT 1PPS CLK Valid ]` | `[ Ext 1PPS Valid ]` |
| `: GPS 1PPS CLK invalid` | `: GPS 1PPS invalid` |
| `: EXT 1PPS CLK invalid` | `: Ext 1PPS invalid` |
| -- | `: switched to GPS 1PPS reference` |
| -- | `: switched to Ext 1PPS reference` |

and likewise for `Invalid`.  The captured 58503A screen
(`crates/smartclock/tests/fixtures/status_screen/58503a-live-01.txt`)
otherwise matches the Z3801A's strings.

## Frame

    ------------------------------- Receiver Status -------------------------------
    SmartClock Mode ___________________________   Reference Outputs _______________
    Holdover Uncertainty ____________
    Position ________________________
    PRNs Ignored ___________________

The underscores are literal, not a rendering artifact of the manuals.
The frame is 79 columns.

## Satellite table

    Tracking: %d        Not Tracking: %-2d%11s
    PRN  El  Az   SS
    PRN  El  Az

Two header variants; the signal column distinguishes the tracked group
from the untracked one.  Cell contents:

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

Appended to the mode marked `>>`.  Available width chooses the long or
short form:

    : stabilizing frequency          FFOM 1

Once the PLL has settled the suffix goes and FFOM reads 0.

Why it is in holdover.  `:SYNChronization:HOLDover:WAITing?` makes the
same distinctions with `GPS`, `LIMit` and `HARDware`.  The register
bits do not make the first: a manual holdover sets Holding, bit 0:

    : manually initiated
    : GPS 1PPS CLK invalid       58503A: : GPS 1PPS invalid
    : EXT 1PPS CLK invalid       58503A: : Ext 1PPS invalid
    : 1PPS TI exceeds hold threshold
    : internal hardware problem

The ladder up to lock, in the order a receiver climbs it:

    : OCXO warm-up
    : GPS acquisition
    : coarse freq adj            : coarse frequency adjustment
    : fine freq adj              : fine frequency adjustment
    : phase alignment
    : leap second determination

A 58503A recovering from a ninety-second antenna outage showed fine
frequency adjustment and then stabilizing on its front panel, in that
order; the panel does not use these spellings.

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
    ACQUISITION .........  [ GPS 1PPS CLK Valid ]     58503A forms above
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

The leap, service-request and validity fields are `%1c`, not digits,
so the leap field can carry `+` or `-`.

## Identity

    HEWLETT-PACKARD,%s,%s,%s-%c
