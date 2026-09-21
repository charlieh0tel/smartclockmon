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

Measured with an HP 34401A, black lead on ground and red on the EFC pin
at the oscillator, the same two points each time.

| When (UTC) | EFC pin | Raw | Relative | Temperature |
| ---------- | ------- | --- | -------- | ----------- |
| 2026-09-20 22:41 | 50.77 mV | 713587 | +36.1061 % | 37.40 C |
| 2026-09-21 01:52 | 55.5 mV  | 712820 | +35.9597 % | 36.04 C |

Earlier, less precisely: about 52 mV at the coax and about 50 mV at the
pin, both near raw 713352 to 713426.  Those are consistent with the
first row and add nothing to the slope, since the count has barely
moved.

### What the pair says, and what it does not

Between those two rows the count fell 765 and the pin rose 4.73 mV.
That is **6.2 uV per count**, against 9.5 uV per count if the receiver
drives the full -5 V to +5 V and 0.13 uV per count if it drives only a
sliver.  The sliver mapping predicts 0.1 mV of movement where 4.73 mV
was measured, a factor of 47 out; the full-span mapping is within 35
percent.  So the pin moves like something driven across volts, not
millivolts, which makes the "receiver drives a sliver and the unit needs
retrimming" branch much less likely.

Two things stop this settling it.

The sign is backwards.  The count went down and the voltage went up.
Either something inverts between the DAC and the pin, or the reasoning
above is measuring the wrong thing.

The temperature fell 1.365 C over the same interval, so the count and
the temperature moved together and the pair cannot say which drove the
voltage.  A tempco of 3.5 mV/C in the measurement path would account for
the whole 4.73 mV with no relationship to the DAC at all.  Across the
whole log the two correlate at r = 0.79, so this is not a remote
possibility.

Note also that the receiver's reported temperature is quantised to
0.273 C -- ten distinct values in 9000 samples -- so "the temperature
did not change" only ever means "it did not cross a step".

### The measurement that would settle it

Separating the DAC from the temperature needs a pair taken while the
temperature is flat and the count is not.  The log has those: windows
where the reported temperature holds one quantisation step for ten
minutes while the count moves 300 to 400.  Over such a window the
predictions are far enough apart to decide it:

| If the pin follows | Change over ~375 counts |
| ------------------ | ----------------------- |
| The DAC, at full-span sensitivity | about 3.6 mV |
| Temperature only | at most 0.95 mV, bounded by the 0.273 C step |

So: two readings ten to fifteen minutes apart during a thermally quiet
stretch.  The daemon records the count and the temperature, so this
needs the two meter readings and nothing else.

The absolute level remains unexplained either way.  At 36 percent the
specification mapping puts the pin at +1.80 V and it reads tens of
millivolts, and a slope near the full-span figure makes that harder to
explain rather than easier: a divider that scaled 1.8 V down to 50 mV
would scale the slope down with it, and it is not scaled down.

## Where the unit stands, conditionally

Under the specification mapping, 36.06 percent of +/- 2.0x10^-7 is
7.2x10^-8 used with about 1.3x10^-7 left, on the order of a decade of
headroom at typical aging.

Under the measured mapping there is far less: single-digit nanohertz
per hertz of range, most of a year at best.

These differ by more than an order of magnitude, so the earlier claim of
a decade of headroom should not be relied on until the measurement above
is done.  The second paired reading leans towards the specification
mapping -- the pin moves microvolts per count, not tenths of a
microvolt -- but leans is all it does while the temperature confound
stands.  What does hold regardless is the receiver's own judgement: the
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
