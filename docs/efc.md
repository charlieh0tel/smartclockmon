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
| 2026-09-22 01:35 | 52.58 mV | 713269 | +36.0453 % | 39.86 C |
| 2026-09-22 19:56 | 54.77 mV | 712948 | +35.9841 % | 36.86 C |

The temperature column is the receiver's internal sensor: the air
inside the case, not the crystal.  The crystal sits in an oven with its
own control loop holding it at a fixed temperature, so ambient has no
direct path to the EFC the oscillator needs -- if it had one, the oven
would not be doing its job.  The column is here to test the
*measurement*, not the oscillator: a tempco in the meter, the leads or
whatever divides the pin down would move the reading with ambient and
fake a slope.  What residual ambient sensitivity a working oven leaves
is a thousandth of the outside swing, far below anything four points
could see.

Earlier, less precisely: about 52 mV at the coax and about 50 mV at the
pin, both near raw 713352 to 713426.  Those are consistent with the
first row and add nothing to the slope, since the count has barely
moved.

### What the four say

The third reading settles what the pair could not, by breaking the
thing that made the pair ambiguous.  It was taken at 39.86 C, the
hottest of the four and nearly four degrees above the second, and its
voltage came out in the middle -- exactly where its count puts it.  Had
temperature been driving the pin, the hottest point would have been the
most extreme, and it is not.

The fourth was taken as a prediction rather than a measurement: its
count of 712948 put the pin at 54.70 mV before the meter was read, and
the meter said 54.77.  No reading is more than 0.12 mV off the line.

Fitting all four:

| Against | Slope | R^2 |
| ------- | ----- | --- |
| Raw count | -6.25 uV/count | 0.998 |
| Temperature | -0.69 mV/C | 0.281 |

The count explains the pin; the temperature does not.  The correlation
of r = 0.79 across the whole log is real but is the count and the
temperature drifting together over a day, not a causal path from the
sensor to the pin.

The count itself is a different matter, and does track ambient closely;
see "What the count does with ambient" below.  That is the oven's
normal residual, not a failure of it.

So the slope stands at **6.25 uV per count** over a 767 count span,
against 9.5 uV per count for a full -5 V to +5 V drive and 0.13 uV per
count for a sliver.  The sliver mapping is out by a factor of 48 and is
finished: the pin moves like something driven across volts.

Two things from the pair survive into the rest.

The sign is still backwards -- count up, voltage down -- and now
consistently so across four points, which makes it a property of the
path rather than noise in a pair.  Something inverts between the DAC
and the pin.

And the absolute level is still wrong, in a way the confirmed slope
makes worse rather than better.  At 36 percent the specification
mapping puts the pin at +1.80 V; it reads 52 mV.  A divider scaling
1.8 V to 50 mV would scale the slope by the same 36x, and the slope is
not scaled: extrapolating 6.25 uV/count across the 20 bit range gives a
6.55 V span, which is the right order for a real EFC drive and the
wrong answer for the level measured on it.  Whatever explains 52 mV has
to leave the slope alone, which rules out a plain divider.

### What the pair said, and what it did not

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
voltage.  Not through the oscillator -- the oven stands between
ambient and the crystal -- but through the measurement: a tempco of
3.5 mV/C anywhere in the path from pin to meter would account for the
whole 4.73 mV with no relationship to the DAC at all.  Across the
whole log the two correlate at r = 0.79, so this is not a remote
possibility.

Note also that the receiver's reported temperature is quantised to
0.273 C -- ten distinct values in 9000 samples -- so "the temperature
did not change" only ever means "it did not cross a step".

### The measurement that settled it

The plan here was a pair taken during a thermally quiet window, chosen
so the count moved and the temperature did not.  What arrived instead
was better and required no waiting: a third point at a temperature well
outside the range of the first two.  A thermally flat pair would have
shown the count moving the pin with the temperature held still; a
thermally distant third point shows the pin ignoring a four degree
excursion and following the count anyway, which is the same conclusion
from the opposite direction and is harder to argue with.

## What the count does with ambient

The pin follows the count and not the room.  The count, though, does
follow the room, and closely: over 46 hours and 137,333 samples, with
the aging trend removed, the DAC count correlates with the internal
temperature at r = +0.88.

This is worth being careful about, because the obvious reading of it is
alarming and wrong.  The crystal is in an oven with its own control
loop.  If the room reached the crystal, the oven would not be doing its
job.

It has not.  The coupling is about **+109 counts per degree C**, and
the count is a frequency knob: 2^20 counts span +/- 2.0x10^-7, so that
is

    109 counts/C  x  1.907x10^-4 %/count  x  2.0x10^-9 per %
        = 4.1x10^-11 per C

The 10811 specification is <2.5x10^-9 over 0 C to 71 C, an average of
about 3.5x10^-11 per C.  The measured residual sits right at it.  An
oven that had stopped working would show the crystal's raw coefficient,
which is two to three orders larger.  What we are seeing is an oven
doing its job to specification and the GPS loop cleaning up what is
left.

### This decodes the reported tempco

`:DIAGnostic:ROSCillator:TCOefficient?` returns -33.65 on this unit and
has never moved.  It is undocumented -- discovered, not in the manual --
so its units were an open question.  They are **parts in 10^12 per
degree C**: the reported -33.65x10^-12/C against a measured
+4.1x10^-11/C, the same size, and opposite in sign exactly as a
correction should be.  The oscillator slows as it warms; the loop
pushes the other way.

The agreement is weaker evidence than it looks.  The receiver is told
to spend its first 24 hours locked so it can learn its oscillator, and
the natural way to learn a temperature coefficient is the regression we
just did.  So the match confirms the units and little else -- we may
simply have recomputed the receiver's own homework.

### Is it feedforward?

The manual puts the compensation in holdover:

> In the absence of GPS, SmartClock operates in "holdover" mode, which
> maintains precise time and frequency over an extended duration by
> predicting and compensating for aging and temperature effects.

and the learning in lock.  But it never says the temperature term is
switched off while locked, and there is a reason it might not be: a
term enabled only at the instant GPS drops puts a step into the EFC at
the worst possible moment, where applying it always makes the entry to
holdover bumpless and the closed loop simply absorbs it.

That predicts something checkable.  The reported temperature is
quantised to 0.273 C, and 0.273 C is 30 counts.  If the receiver
computed a feedforward term from that reading, every quantisation step
would kick the DAC by 30 counts at once.

Aligning on the 49 sustained steps in the log -- those where the level
held for a minute either side, rather than dithering across a boundary
-- the mean DAC change ten seconds after a step is **+0.02 +/- 0.43
counts**.  Thirty counts is excluded by some seventy standard errors.
The DAC eases through a temperature step; it does not jump.

So there is no feedforward keyed to the reported value.  This does not
rule out a term computed from a finer internal reading than the one
published over SCPI, which would produce no step to find.

The clean test is holdover: with the loop open, any movement of the EFC
with temperature has to be feedforward, because nothing else is left to
cause it.  That means initiating holdover deliberately, which is a
decision about the bench and not about this document.

## Where the unit stands

Under the specification mapping, 36.06 percent of +/- 2.0x10^-7 is
7.2x10^-8 used with about 1.3x10^-7 left, on the order of a decade of
headroom at typical aging.

Under the measured mapping there is far less: single-digit nanohertz
per hertz of range, most of a year at best.

These differ by more than an order of magnitude.  The third reading
decides between them: at 6.25 uV per count the pin moves microvolts per
count, not tenths of a microvolt, and it does so independently of
temperature, so the **specification mapping is the one to use** and the
decade of headroom stands.  It is still a slope measured over 767 of
2^20 counts and extrapolated, so it says the receiver drives volts
rather than millivolts; it does not certify linearity across the range
nobody has driven it over.  What holds regardless is the receiver's own
judgement: the hardware condition register has neither the
near-full-scale nor the full-scale EFC bit set, and its health monitor
reports EFC OK.  The
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
