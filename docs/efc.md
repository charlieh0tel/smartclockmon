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
It is undocumented, and before the firmware was read its units were
inferred as parts in 10^12 per degree C, from -33.65x10^-12/C against
the measured +4.1x10^-11/C -- the same size, opposite in sign.  The
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

## Where the unit stands

Reading the reported percentage as position in the pull range,
36.06 percent of +/- 2.0x10^-7 is 7.2x10^-8 used with about 1.3x10^-7
left -- on the order of a decade of headroom at typical aging.  Under
the sliver mapping there would be single-digit nanohertz per hertz of
range, most of a year at best.

The offset above argues for more headroom still, not less.  If the pin
is 0 V near +37.6 percent, this unit at +36.0 percent has pulled only
about 1.6 percentage points, some 3x10^-9, and has most of the range in
both directions.  Both readings are comfortable and the difference
between them does not matter yet, so the conservative one is quoted.

The measured 6.40 uV per count settles it for the specification
mapping.  That slope is measured over 987 of 2^20 counts and
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
