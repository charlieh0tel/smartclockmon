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

## The pin follows the count

Each row is an external measurement at the oscillator's EFC pin, taken
with an HP 34401A -- black lead on ground, red on the pin, the same two
points each time -- recorded with the raw value the receiver reported
at that moment.

| When (UTC) | EFC pin | Raw | Relative | Temperature |
| ---------- | ------- | --- | -------- | ----------- |
| 2026-09-20 22:41 | 50.77 mV | 713587 | +36.1061 % | 37.40 C |
| 2026-09-21 01:52 | 55.5 mV  | 712820 | +35.9597 % | 36.04 C |
| 2026-09-22 01:35 | 52.58 mV | 713269 | +36.0453 % | 39.86 C |
| 2026-09-22 19:56 | 54.77 mV | 712948 | +35.9841 % | 36.86 C |

| Against | Slope | R^2 |
| ------- | ----- | --- |
| Raw count | -6.25 uV/count | 0.998 |
| Temperature | -0.69 mV/C | 0.281 |

No reading is more than 0.12 mV off the count fit.  The third row was
taken four degrees hotter than the second and came out in the middle,
where its count puts it; the fourth was predicted at 54.70 mV before
the meter was read.  The count explains the pin and the temperature
does not.

The temperature column exists to test the *measurement*, not the
oscillator.  It is the receiver's internal sensor -- the air in the
case, not the crystal, which is in an oven.  A tempco in the meter, the
leads, or whatever divides the pin down would move the reading with
ambient and fake a slope.  It does not.  (The reading is quantised to
0.273 C, so "the temperature held" only ever means "it did not cross a
step".)

Two consequences.

**The sliver mapping is dead.**  6.25 uV per count, against 9.5 uV for
a full -5 V to +5 V drive and 0.13 uV for a sliver.  The pin moves like
something driven across volts, so the unit does not need periodic
manual retrimming.

**The absolute level is still unexplained.**  At 36 percent the
specification mapping puts the pin at +1.80 V; it reads 52 mV, some 36
times less.  A divider scaling 1.8 V to 50 mV would scale the slope by
the same 36x, and the slope is not scaled: 6.25 uV/count across 2^20
counts is a 6.55 V span.  Whatever explains 52 mV has to leave the
slope alone, which rules out a plain divider.  The sign is inverted
too -- count up, voltage down -- consistently across all four points,
so something inverts between the DAC and the pin.

## What the count does with ambient

The count does follow the room: over 46 hours and 137,333 samples, with
the aging trend removed, it correlates with internal temperature at
r = +0.88.  This is the oven's normal residual, not a failure of it.

The coupling is **+109 counts per degree C**, and the count is a
frequency knob:

    109 counts/C  x  1.907x10^-4 %/count  x  2.0x10^-9 per %
        = 4.1x10^-11 per C

The 10811 specification is <2.5x10^-9 over 0 C to 71 C, an average of
about 3.5x10^-11 per C.  The measured residual sits right at it.  A
failed oven would show the crystal's raw coefficient, two to three
orders larger.

### The reported tempco

`:DIAGnostic:ROSCillator:TCOefficient?` returns -33.65 on this unit.
It is undocumented, so its units had to be inferred: **parts in 10^12
per degree C**, from -33.65x10^-12/C against the measured
+4.1x10^-11/C -- the same size, opposite in sign as a correction should
be.  The oscillator slows as it warms and the loop pushes the other
way.

The agreement proves less than it appears.  The receiver learns the
coefficient from the corrections it applies against GPS, which is the
regression above.

### It is not feedforward

Kusters' design paper puts the temperature loop in holdover:

> During the loss of the reference, HP SmartClock uses all of the data
> learned previously about the oscillator to control the oscillator
> [...] A control loop tracks temperature changes in the module and
> computes the correct offsets for the oscillator to remove temperature
> effects.

and the measurement of the response in lock:

> While locked to the GPS system, HP SmartClock employs enhanced
> learning to measure the aging and environmental response of the
> internal reference source.

Measure locked, apply in holdover.  So the diurnal swing in the EFC
while locked is not compensation; it is what is being compensated.

The bench agrees.  A feedforward term computed from the reported
temperature would kick the DAC 30 counts at each 0.273 C boundary.
Across the 49 sustained steps in the log -- level held a minute either
side, rather than dithering across a boundary -- the mean DAC change
ten seconds later is **+0.02 +/- 0.43 counts**.

### Why the coefficient never changes

> Constants related to the aging of the oscillator are stored in RAM
> and are redetermined each time the receiver is turned on.  Constants
> related to temperature performance are stored on EPROM, since
> temperature performance does not substantially change during periods
> when the oscillator is not powered.

The two kinds are kept differently on purpose.  A tempco that sits
still across days is the design working, not a value failing to update.

## Where the unit stands

Under the specification mapping, 36.06 percent of +/- 2.0x10^-7 is
7.2x10^-8 used with about 1.3x10^-7 left -- on the order of a decade of
headroom at typical aging.  Under the sliver mapping there would be
single-digit nanohertz per hertz of range, most of a year at best.

The measured 6.25 uV per count settles it for the specification
mapping.  That slope is measured over 767 of 2^20 counts and
extrapolated, so it establishes that the receiver drives volts rather
than millivolts; it does not certify linearity across a range nobody
has driven it over.  The receiver's own judgement agrees: the hardware
condition register has neither the near-full-scale nor the full-scale
EFC bit set, and its health monitor reports EFC OK.

## What is still unmeasured

Frequency pull per EFC unit on this unit.  Van Baak measures 5.2x10^-13
per unit on a Z3801A, but that is a different assembly -- the Z3801A is
reported to use a `-60161`, which does not appear among the 27 variants
in `10811-90027-1`, so its specification is not to hand.

Dividing this oscillator's span, 4.0x10^-7 end to end, across 2^20
counts gives about **3.8x10^-13 per count** if the receiver drives the
whole range.  That is a working number, not a measurement.

It does settle whether the Z3801A's EFC range is wider.  It is, by
about a third -- though only one side of the comparison has a
specification behind it:

| | Per count | Implied span |
| - | --------- | ------------ |
| 58503A, `-60159`, from specification | 3.8x10^-13 | +/- 2.0x10^-7 |
| Z3801A, `-60161`, from Van Baak's measurement | 5.2x10^-13 | +/- 2.7x10^-7 |

Two figures reached independently, from a datasheet and from a counter,
landing a third apart on parts that differ by one dash number.  The
`-60161` remains undocumented in anything to hand, so treat its row as
indicative rather than as a specification.

Measuring this unit properly still means logging `EFC:ABSolute?` against
a counter while the receiver corrects itself out of a long holdover.
