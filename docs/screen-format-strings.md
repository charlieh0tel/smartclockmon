# Status screen format strings

The templates of the screen `:SYSTem:STATus?` returns, as the
Z3801A image (`third_party/firmware/z3801a-3543.bin`) holds them.  The 58503A
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

The mode lines are `Locked `, `Recovery`, `Holdover` and `Power-up`;
the one marked `>>` carries a suffix.  Both come from the loop's stage
byte, the first of its state block (`firmware/loop.md`, "The
stages"), through four accessors the status screen's snapshot calls.
The routines were read in the Z3816A (mode `FUN_0004eef0`, ladder
`FUN_0004ea3a`, accessors `0x495a8` to `0x496c4`, snapshot
`FUN_0004ebbc`); the Z3801A's accessors are byte for byte the same, at
`0x45ba8` to `0x45cc4` with the stage at `0x10215a`, and "In every
image" below gives the others.  The stage values are the Z3816A's
names (`firmware/loop.md`):

| Stage | Mode marked | Suffix |
| ----- | ----------- | ------ |
| 1 `powerup` | `Power-up` | the ladder, by sub-state (below) |
| 2 `powerup recovery` | none | -- |
| 3 `holdover` | `Holdover` | why (below) |
| 4 `holdover recovery` | `Recovery` | `fine freq adj` or `phase alignment` while its sub-state is `fine`, by the fine sub-state; otherwise none |
| 5 `startup pll` | `Locked ` | `to GPS: stabilizing frequency` |
| 6 `normal pll` | `Locked ` | `to GPS` |
| 0, 7 to 9 | none | -- |

`to Ext` replaces `to GPS` when the snapshot's external-reference byte
is set.  The suffix after it is chosen by the outputs line: it shows
while the outputs are other than `[ Outputs Valid ]`, which in every
image but the Z3815A only stage 6 gives (stages 2 to 5 and 8 give
`Valid/Reduced Accuracy`, 1, 7 and 9 `Invalid`).  A 58503A with `10MHZ STABLE` on its
panel had `>> Locked to GPS` with no suffix at the same moment, and
the suffix was seen with FFOM 1 and gone with FFOM 0.

The ladder, from the power-up sub-state (`firmware/loop.md`, value 1)
and, in `fine`, the fine sub-state:

| Power-up sub-state | Suffix |
| ------------------ | ------ |
| 0 `start` | none |
| 1 `internal oven warmup`, 2 `external oven warmup` | `: OCXO warm-up` |
| 3 `warmed up, waiting for GPS` | `: GPS acquisition`, or `Ext 1PPS invalid` with the external reference |
| 4 `coarse` | `: coarse freq adj` / `: coarse frequency adjustment` |
| 5 `fine`, fine sub-state 0 to 6 (`fine start` to `fine meas 2`) | `: fine freq adj` / `: fine frequency adjustment` |
| 5 `fine`, fine sub-state 7 to 9 (`fine slew` to `fine fine slew`) | `: phase alignment` |
| 6 `waiting`, 7 `calculating leapseconds` | `: leap second determination` |

The short form, followed by `[TI %9.9s]` with the latest one-second
interval reading, is used when that reading's valid flag is set
(`firmware/interval.md`, "Mean, flag": the snapshot copies the reading
and flag to `+8` to `+0x13`); otherwise the long form, with no
reading.  Width does not choose.  `phase alignment` has only the one
form and takes the reading the same way.

Why it is in holdover.  A manual holdover sets a flag beside the stage
(Z3816A stage + `0x12`), and the suffix is then `: manually
initiated`.  Otherwise a cause byte (stage + `0x11`) is tested in this
order:

| Cause bits | Suffix |
| ---------- | ------ |
| 4 or 2 | `: internal hardware problem` |
| 1 | `: GPS 1PPS CLK invalid` (58503A and later: `: GPS 1PPS invalid`), `Ext` with the external reference |
| 3 | `: 1PPS TI exceeds hold threshold` |
| none of these | none |

Which code sets each bit was not traced.  The mode routine also
handles causes the accessor never returns -- 1 and 4, and `0x40` and
`0x80`, which would show `: switched to GPS 1PPS reference` (or `Ext`)
and `: GPS receiver failure` -- so in the images that hold those two
strings they are never shown.  `:SYNChronization:HOLDover:WAITing?` makes the same
distinctions with `GPS`, `LIMit` and `HARDware`; the register bits do
not make the manual one: a manual holdover sets Holding, bit 0.

A 58503A recovering from a ninety-second antenna outage showed fine
frequency adjustment and then stabilizing on its front panel, in that
order -- stage 4 then stage 5; the panel does not use these spellings.

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

## In every image

Each string this file quoted on 2026-10-09 was looked for in every
image: 55 of the 88 are in all seven.  The mode lines and suffixes
"Mode suffixes" quotes are covered under "The mode suffixes" below.  The rest divide by
family:

| String | Z3801A 3543, Z3805A 3543B | 58503A 3633, 3704 | Z3816A 4001 | Z3815A 4010 | 58503B 1.01.04 |
| ------ | ------------------------- | ----------------- | ----------- | ----------- | -------------- |
| the frame's `Receiver Status` header, `Synchronized to GPS Time` | ✓ | ✓ | ✓ | | ✓ |
| `GPS 1PPS CLK`, `EXT 1PPS CLK` and their `Valid`/`invalid` forms, `rel to GPS` and its variants, `Invalid: GPS rcvr err`, `ELEV MASK %2d deg%3s%-25.25s%2s` | ✓ | | | | |
| `GPS 1PPS`, `Ext 1PPS` and their forms, `Questionable accuracy` | | ✓ | ✓ | ✓ | ✓ |
| the front-panel strings (`HLD USR` to `SURVEY HALT`) | | ✓ | | | ✓ |
| `PRN  El  Az   SS`, the tracked group's header | ✓ | ✓ | | | |
| `PRN  El  Az  C/N`, `------------------- GPSR `, `GT-74 Command Log` | | | | ✓ | |

So the 58503B has a front panel's strings as the 58503A does.  The
Z3815A's screen is headed `------------------- GPSR ` and
` Status ----` rather than `Receiver Status`, has no `Synchronized to
GPS Time` (every image has `Synchronized to UTC`), heads its tracked
satellites `PRN  El  Az  C/N` for its Furuno engine's carrier-to-noise
figure, and adds a `GT-74 Command Log` screen.  The Z3816A and 58503B
hold neither signal-column header, only `PRN  El  Az`; how they head
that column, if they do, was not traced.

**The mode suffixes.**  Every image has the four accessors and the
strings of "Mode suffixes"; the 58503A and later spell the 1 PPS
suffixes without `CLK`, and the Z3801A and Z3805A lack the two
`switched to` suffixes and `GPS receiver failure`.  The accessors
are the Z3816A's byte for byte in the Z3801A, Z3805A and 3633; in
3704, the 58503B and the Z3815A they are the same tests at other
offsets in the state block, the cause read as the word at `+0x10`
(Z3815A `+0x1e`):

| Image | Stage | Accessors | Manual flag | Recovery, fine sub-states |
| ----- | ----- | --------- | ----------- | ------------------------- |
| Z3801A 3543 | `0x10215a` | `0x45ba8` | `+0x12` | `+0x15`, `+0x17` |
| Z3805A 3543B | `0x102162` | `0x45b70` | `+0x12` | `+0x15`, `+0x17` |
| 58503A 3633 | `0x10246e` | `0x45c52` | `+0x12` | `+0x15`, `+0x17` |
| 58503A 3704 | `0x102476` | `0x45d44` | `+0x13` | `+0x17`, `+0x19` |
| Z3816A 4001 | `0x10282c` | `0x495a8` | `+0x12` | `+0x15`, `+0x17` |
| Z3815A 4010 | `0x10286e` | `0x4f3d6` | `+0x21` | `+0x25`, `+0x27` |
| 58503B 1.01.04 | `0x102bb2` | `0x4acee` | `+0x13` | `+0x1f`, `+0x21` |

The Z3815A's stages run one higher from `n3 lock pll`, 6
(`firmware/loop.md`, "The Z3815A's extra stage"): 5 to 7 mark
`Locked `, and its outputs line (`0x561a0`) gives `Valid` at 7 and at
6 when `FUN_00033f5e` returns nonzero, `Invalid` at 1, 8 and 10.  The
mode routines were compared only through their accessors and strings.
