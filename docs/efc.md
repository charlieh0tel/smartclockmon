# EFC on the 58503A

What the receiver reports about its oscillator's control voltage, and
what that means in volts and in frequency.

## The reported value is twenty bits

`:DIAGnostic:ROSCillator:EFControl:ABSolute?` and `:RELative?` are one
quantity in two units.  Neither is documented; both were found by
sweeping the receiver.

    relative percent = raw / 2^20 * 200 - 100

Raw 713392 gives 36.0687, exactly what `:RELative?` returned at the same
moment.  Tom Van Baak reached the same conclusion on a Z3801A by a
different route and traced the hardware behind it:
<http://www.leapsecond.com/pages/z3801a-efc/>.  The value is 20 bits but
the converter is a 16-bit AD569 updated at 102.4 Hz, its low-order bits
dithered to interpolate the remaining four and smoothed by a
second-order filter.  "20-bit DAC" would be wrong.

## The oscillator

An HP 10811-60159, whose electronic frequency control spans
**+/- 2.0x10^-7 over a -5 V to +5 V input**.  See `OCXO.md` for the full
specification and its sources.

## The voltage and the specification do not agree

About 50 mV was measured from the EFC pin to ground **at the
oscillator** while `RELative` read 36.06 percent and `ABSolute?` read
713352.

If the reported percentage spans the oscillator's -5 V to +5 V input,
that point should read **+1.80 V**.  It reads 50 mV, some 36 times
less.  The first reading was taken at the coax and might have been the
wrong SMB, since the 10811 brings out 10 MHz and EFC on identical
connectors; this one was taken at the pin, so that explanation is gone.

Taking the 50 mV at face value gives a receiver that drives only a
sliver of the input:

| Mapping | Full scale at the pin | Pull available | Per count |
| ------- | --------------------- | -------------- | --------- |
| Bipolar, 0 V at 0 percent | +/- 139 mV | +/- 5.6x10^-9 | 1.1x10^-14 |
| Unipolar, 0 V at -100 percent | 73 mV | +/- 1.5x10^-9 | 2.8x10^-15 |

Which creates a problem.  The oscillator ages at up to 2.5x10^-10 per
day, around 9x10^-8 a year, and typically 1x10^-8 a year once settled.
A pull range of 5.6x10^-9 would be used up within a year even at the
typical rate, and the 10811's only coarse adjustment is an 18-turn
mechanical control that no receiver can reach.  A product meant to run
unattended for years cannot be built that way.

So one of three things is true, and nothing here settles which: the
receiver drives the full span and the 50 mV has some explanation not yet
found; the receiver drives a sliver and the unit needs periodic manual
retrimming; or the percentage means something other than position on the
EFC input.

### Paired readings

Each line is an external measurement at the oscillator's EFC pin,
recorded with the raw value the receiver reported at the same moment.
Two readings far enough apart in count give the slope.

| When (UTC) | EFC pin | Raw | Relative | Temperature |
| ---------- | ------- | --- | -------- | ----------- |
| 2026-09-20 22:41 | 50.77 mV | 713587 | +36.1061 % | 37.40 C |

Earlier, less precisely: about 52 mV at the coax and about 50 mV at the
pin, both near raw 713352 to 713426.  Those are consistent with the row
above and add nothing to the slope, since the count has barely moved.

### The measurement that would settle it

The EFC count drifts on its own, about 70 counts over a few minutes of
probing.  Over a day it should move thousands.  The two candidate
mappings predict very different voltages for that movement:

| If the receiver drives | Volts per count | 1000 counts |
| ---------------------- | --------------- | ----------- |
| The full -5 V to +5 V | 9.5 uV | 9.5 mV, easily seen |
| Only +/- 139 mV | 0.13 uV | 0.13 mV, invisible |

A factor of seventy apart.  So: note the EFC pin voltage and the raw
count together, leave the daemon logging, and read both again a day
later.  The daemon already records the count, so this needs two meter
readings and nothing else.

## Where the unit stands, conditionally

Under the specification mapping, 36.06 percent of +/- 2.0x10^-7 is
7.2x10^-8 used with about 1.3x10^-7 left, on the order of a decade of
headroom at typical aging.

Under the measured mapping there is far less: single-digit nanohertz
per hertz of range, most of a year at best.

These differ by more than an order of magnitude, so the earlier claim of
a decade of headroom should not be relied on until the measurement above
is done.  What does hold regardless is the receiver's own judgement: the
hardware condition register has neither the near-full-scale nor the
full-scale EFC bit set, and its health monitor reports EFC OK.  The
receiver does not think the oscillator is near its limit.

## What is still unmeasured

Frequency pull per EFC unit on this unit.  Van Baak measures 5.2x10^-13
per unit on a Z3801A, but that is a different assembly -- the Z3801A is
reported to use a `-60161`, which does not appear among the 27 variants
in `10811-90027-1`, so its specification is not to hand.

Dividing this oscillator's span, +/- 2.0x10^-7 or 4.0x10^-7 end to end,
across 2^20 counts gives about **3.8x10^-13 per count** if the receiver
drives the whole range.  That is a working number, not a measurement.

It does settle the earlier question of whether the Z3801A's EFC range is
wider.  It is, by about a third -- though only one side of the
comparison has a specification behind it:

| | Per count | Implied span |
| - | --------- | ------------ |
| 58503A, `-60159`, from specification | 3.8x10^-13 | +/- 2.0x10^-7 |
| Z3801A, `-60161`, from Van Baak's measurement | 5.2x10^-13 | +/- 2.7x10^-7 |

Two figures reached independently, from a datasheet and from a counter,
landing a third apart on parts that differ by one dash number.  That is
about the agreement such a comparison deserves, and a long way from the
twenty-fold difference guessed at earlier from an EFC sensitivity that
was never checked.

The `-60161` remains undocumented in anything to hand: it is absent from
the 27 variants in `10811-90027-1` and from the six on the page above.
Its row in the table is inferred from a measurement of one oscillator,
so treat it as indicative rather than as a specification.

Measuring this unit properly still means logging `EFC:ABSolute?` against
a counter while the receiver corrects itself out of a long holdover.
