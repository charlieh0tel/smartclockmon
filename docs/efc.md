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
with an HP 34401A or a Keysight 34461A -- black lead on ground, red on
the pin, the same two points each time -- recorded with the raw value
the receiver reported at that moment.  Both are 6½-digit bench meters,
good to well under a millivolt on these readings; which took each row
was not recorded.

| When (UTC) | EFC pin | Raw | Relative | Temperature |
| ---------- | ------- | --- | -------- | ----------- |
| 2026-09-20 22:41 | 50.77 mV | 713587 | +36.1061 % | 37.40 C |
| 2026-09-21 01:52 | 55.5 mV  | 712820 | +35.9597 % | 36.04 C |
| 2026-09-22 01:35 | 52.58 mV | 713269 | +36.0453 % | 39.86 C |
| 2026-09-22 19:56 | 54.77 mV | 712948 | +35.9841 % | 36.86 C |
| 2026-09-23 02:25 | 55.35 mV | 712853 | +35.9657 % | 35.22 C |
| 2026-09-24 00:00 | 52.36 mV | 713254 | +36.0425 % | 41.22 C |
| 2026-09-24 01:09 | 56.88 mV | 712600 | +35.9178 % | 40.95 C |

The fifth row was taken minutes after a ninety-second holdover, at the
coldest case temperature of the seven; the sixth is the hottest.

| Against | Slope | R^2 |
| ------- | ----- | --- |
| Raw count | -6.40 +/- 0.23 uV/count | 0.9937 |
| Temperature | -0.14 mV/C | 0.026 |

The third row was taken four degrees hotter than the second and came
out in the middle, where its count puts it; the fourth was predicted at
54.70 mV before the meter was read.  The count explains the pin and the
temperature does not.

The sixth row fits less well.  The first five predicted 52.80 mV at its
count; it read 52.36, and at 0.35 mV it is the largest residual of the
seven, where none of the others is more than 0.19 mV off.  It is also
the hottest reading.  The seventh was taken 0.3 C cooler, predicted at
56.90 mV by the first six, and read 56.88.  The residuals of the count
fit against temperature go as -48 +/- 23 uV/C, R^2 0.46, over seven
points.

The temperature column exists to test the *measurement*, not the
oscillator.  It is the receiver's internal sensor -- the air in the
case, not the crystal, which is in an oven.  A tempco in the meter, the
leads, or whatever divides the pin down would move the reading with
ambient and fake a slope.  It does not.  (The reading is quantised to
0.273 C, so "the temperature held" only ever means "it did not cross a
step".)

Two consequences.

**The sliver mapping is dead.**  6.40 uV per count, against 9.5 uV for
a full -5 V to +5 V drive and 0.13 uV for a sliver.  The pin moves like
something driven across volts, so the unit does not need periodic
manual retrimming.

**The inverted sign is the oscillator's own.**  Count up, voltage
down, across all seven points.  Nothing inverts between the DAC and the
pin; the 10811's EFC input is specified inverting (`HP-10811AB-Manual`
section 2-13):

> As the EFC voltage goes positive the output frequency will go lower.
> Conversely, as the EFC voltage goes negative, the output frequency
> will go higher.

The chain closes: count up drives the pin down, and the pin down raises
the frequency.  Independently, the count rises as the case warms, which
is the direction needed to correct an oscillator whose coefficient is
negative.  The receiver's count is a monotonic frequency command.

**The slope is a designed round number.**  6.40 +/- 0.23 uV per
reported count.  Four of the twenty bits are dither, so a 16-bit DAC
LSB is 16 counts, or 100 uV exactly, and full scale is
65536 x 100 uV = **6.5536 V**.  The measured span is 6.71 +/- 0.24 V,
which is that value to within two-thirds of a standard error.

**The absolute level is still unexplained.**  At 36 percent the
specification mapping puts the pin at +1.80 V; it reads 52 mV, some 36
times less.  A divider scaling 1.8 V to 50 mV would scale the slope by
the same 36x, and the slope is not scaled.  Whatever explains 52 mV has
to leave the slope alone, which rules out a plain divider.

Extrapolating the fit, the pin reaches 0 V at count 721,500 +/- 300,
or **+37.6 +/- 0.1 percent** reported -- so the reported percentage is
offset from the pin voltage, and this unit, at +36.0 percent, is
sitting within a few millivolts of the oscillator's electrical centre.
That is an extrapolation about 8,400 counts beyond a fit spanning 987,
and the quoted error is the fit's alone; treat it as an indication of
where zero lies, not a measurement of it.  (An earlier version gave
+/- 15,500 counts, having scaled the slope's error by the whole count
rather than by the distance from the data.)

### At full scale

On 2026-09-26, between 17:30 and 17:37 UTC, the receiver held the
count at 0 -- reported -100.0000 percent, the hardware condition
register showing both EFC full-scale bits -- and the pin read
**4.568 V**, case temperature 28.1 C, on one of the same two meters.
(The events that put it there are under "The pull, measured" below.)

The seven-point fit predicted 4.614 V at count 0, 713,000 counts from
its data; the reading is 1.0 percent below that.  The count drives the
pin linearly to within a percent across the whole lower half of the
range, which the fit alone could not show.

The line from the seven points' mean (713,047 counts, 54.03 mV) to the
full-scale reading has a slope of **6.33 uV per count**, 1.3 percent
above the designed 6.25 uV (100 uV per 16-count DAC step), and within
the seven-point slope's error.  At exactly 6.25 uV the pin would read
4.511 V at count 0; the 57 mV difference is far beyond either meter's
error, so it is this unit's DAC chain spanning 1.3 percent more than
nominal, not the measurement.  It crosses 0 V at count 721,580,
**+37.63 percent** -- the zero the extrapolation above indicated, now
with a point at the far end behind it.  The receiver corroborated it:
with the crystal retrimmed to 10 MHz at 0 V, it relocked and settled
through +37.79 percent at 17:48 UTC and +37.67 at 17:51 (counts
722,390 and 721,776), where the line puts the pin at -5 mV and -1 mV.

## What the count does with ambient

The count does follow the room: over 46 hours and 137,333 samples, with
the aging trend removed, it correlates with internal temperature at
r = +0.88.  This is the oven's normal residual, not a failure of it.

The coupling is **+109 counts per degree C**, and the count is a
frequency knob:

    109 counts/C  x  3.94x10^-13 per count  (measured; see "The pull, measured")
        = 4.3x10^-11 per C

The 10811 specification is <2.5x10^-9 over 0 C to 71 C, an average of
about 3.5x10^-11 per C.  The measured residual sits right at it.  A
failed oven would show the crystal's raw coefficient, two to three
orders larger.

### The reported tempco

`:DIAGnostic:ROSCillator:TCOefficient?` returns -33.65 on this unit.
It is undocumented, and before the firmware was read its units were
inferred as parts in 10^12 per degree C, from -33.65x10^-12/C against
the measured +4.3x10^-11/C -- the same size, opposite in sign.  The
firmware says otherwise (`firmware.md`, "s, the oscillator current"):
the value is the constant c in the loop's EFC, u = K.f + B + I + c.s,
where s is the oscillator-current channel of the health monitor
(nominal 250), so its unit is **EFC counts per unit of oscillator
current**, and it is applied at every update, locked or not.  Nothing
in the image learns it: it is written only by this command's setter,
into the EEPROM calibration block, after a measurement the console
word `xcal` makes by fitting EFC against current.  The earlier
agreement in size was a coincidence of units, and the sign is the
sign of the oven current's coupling to the EFC, not of temperature's.

### The regression

If this 58503A applied that term as the Z3816A image does, the DAC
word (`:DIAGnostic:ROSCillator:EFControl:ABSolute?`) would move by
c times each step of the current channel
(`:DIAGnostic:ROSCillator:CURRent?`) at the update that saw it: the
channel here reads 93 to 118 in steps of 0.980, so -33.65 x 0.980 is
a jump of about 33 counts, down when the current rises.  Over the
locked record of 20 to 25 September 2026, 326,626 rows with both
values:

| At a current step | Steps | DAC change in the same row | 60 s later | 300 s later |
| ----------------- | ----- | -------------------------- | ---------- | ----------- |
| current up by one level | 2909 | +0.22 | -0.34 | -1.59 |
| current down by one level | 2905 | -0.13 | -0.05 | -0.48 |
| no change | 320,845 | 0.00 | -0.12 | -0.57 |

No jump: the mean change at a step is within a count of the rows with
no step, against 33 predicted.

That test is too blunt, though.  In the Z3816A image the s the loop
reads is not the fresh reading the query returns but the health
monitor's exponential average of it, s <- 0.1 x fresh + 0.9 x s on
each of its passes (`firmware.md`, "s, the oscillator current"), and
a one-level flicker of an 8-bit reading is mostly dither about a mean
that moves slowly.  So the sharper test regresses the ten-second DAC
change on the change of that average, for a range of monitor
cadences, over the whole record on a 10 s grid (31,161 points):

| Monitor pass assumed | Points | dDAC / ds, counts per unit | r |
| -------------------- | ------ | -------------------------- | - |
| 10 s | 30,995 | -2.2 | -0.016 |
| 20 s | 15,528 | -2.7 | -0.023 |
| 30 s | 10,352 | -0.9 | -0.006 |
| 60 s | 5,176 | -0.3 | -0.003 |

Against -33.65 predicted, a slope near zero with no correlation at
any cadence: on this unit the DAC does not follow the smoothed
current at the instant either.  Over the whole record, with a linear
aging trend fitted alongside (-11.7 counts per day), the DAC does fall
with the smoothed current, by **-53.1 counts per unit of the
channel**, but over hours, which is the loop correcting whatever the
oven current stands for rather than a term applied at the update.

So this 58503A does not apply its -33.65 the way the Z3816A image
applies its c.  A 58503A image of revision 3633 (`firmware.md`, "The
58503A image") has the term in the same form, on the same smoothed
current, and its `EFControl:ABSolute?` reports the DAC word with the
term in it; this receiver is revision 3704-C, whose image is not on
hand.  What this receiver does with its -33.65 is not settled by the
record, and the difference lies between the two revisions or in what
3704 reports.

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

There is a feedforward term, but not from temperature: the firmware
adds c times the oscillator current to the EFC at every update
(`firmware.md`, "The loop").  The oven current moves with ambient,
which is why a coefficient on it is called a temperature coefficient,
but the reported temperature is not an input to it, which is what the
step test above shows.

### Why the coefficient never changes

> Constants related to the aging of the oscillator are stored in RAM
> and are redetermined each time the receiver is turned on.  Constants
> related to temperature performance are stored on EPROM, since
> temperature performance does not substantially change during periods
> when the oscillator is not powered.

The two kinds are kept differently on purpose.  A tempco that sits
still across days is the design working, not a value failing to update.

## The pull, measured

On 2026-09-26 the oscillator's crystal was retrimmed by hand.  The
sequence, from the operator's counter and the daemon's log:

1. With the EFC input disconnected and floating, the crystal read more
   than 3 Hz flat of 10 MHz, and was trimmed to within a few
   millihertz.
2. Reconnected, the receiver came up on its stored count, 711,375
   (+35.68 percent, the pin near +65 mV), and its coarse frequency
   adjustment stepped the count to 0 within eleven seconds of a valid
   GPS reference (17:30:00 to 17:30:13 UTC).  At count 0 the
   operator's counter read the output **0.9 Hz high**, and the 1 PPS
   interval ramped at -88.7 ns/s, a fractional frequency of
   +8.87x10^-8: the two agree.
3. With the EFC input grounded, the crystal was trimmed to within
   **+/- 6 mHz** (6x10^-10) of 10 MHz.
4. Reconnected, still at count 0 (holdover, 17:39:32 to 17:40:43 UTC,
   the count creeping to 10), the interval ramped at **+284.0 ns/s**:
   the output **2.840x10^-7 low**.  A power cycle then ran the coarse
   adjustment again and the receiver relocked at +37.8 percent (above).

Step 4 is the measurement.  Between 0 V, where the crystal sat within
6x10^-10 of nominal, and the 4.568 V count 0 drives, the output moved
by -2.840x10^-7:

| | This unit, measured | Specification | Firmware assumes |
| - | ------------------- | ------------- | ---------------- |
| Per volt at the pin | **-6.22x10^-8** | at least -4.0x10^-8 | -- |
| Per count | **+3.94x10^-13** | 3.8x10^-13 | +6.25x10^-13 |

The specification column is the -60159's "> +/- 2.0x10^-7" over -5 V
to +5 V, spread across 2^20 counts as before; the measured gain is 1.55
times that minimum, inside it.  The firmware column is the loop's G
in the 58503A image, revision 3633 (`firmware.md`), where the bench
unit is 3704-C: the sign agrees -- count up, frequency up -- and the
magnitude is 0.63 of what the loop assumes.  Per count, the measured
figure is over the 721,580 counts from the 0 V point to count 0.

The sign is measured as well as specified: a positive pin lowered the
output, as `HP-10811AB-Manual` section 2-13 says it should.

Step 1 is a caution.  By the measured gain, the crystal after the
floating-input trim sat **3.7x10^-7 sharp** at the receiver's old pin
voltage -- the 0.9 Hz it still read at count 0, plus the 2.80x10^-7
between +65 mV and 4.568 V.  A floating EFC input is not 0 V, and a
trim made against one is off by most of the receiver's range.  Trim
with the input grounded.

Two figures now stand on counters, a third apart on parts that differ
by one dash number:

| | Per count | Over -5 V to +5 V, if linear |
| - | --------- | ---------------------------- |
| 58503A, `-60159`, this unit, measured | 3.94x10^-13 | +/- 3.1x10^-7 |
| Z3801A, `-60161`, Van Baak's measurement | 5.2x10^-13 | +/- 2.7x10^-7 |

The span column extrapolates each gain across the whole input; neither
oscillator has been driven to both ends.  The `-60161` remains
undocumented in anything to hand, so treat its row as indicative.

## Where the unit stands

Since the retrim the receiver locks with the pin within millivolts of
0 V, at +37.8 percent.  Downward, to count 0, it has **2.84x10^-7**
of pull, measured.  Upward the pin has not been driven: the count's
top end, 2^20 - 16, is 326,000 counts away, and neither the pin
voltage there nor the gain over that half is measured.

Which way the crystal ages decides which half matters.  The measured
downward half alone is 2.8 years at the -60159's specified maximum
aging of 1x10^-7 per year (`OCXO.md`).

During the coarse adjustment that ran into count 0 the hardware
condition register set both EFC bits, near full scale and at full
scale; with the retrimmed crystal both are clear.
