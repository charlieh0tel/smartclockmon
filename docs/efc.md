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

## Where the unit stands

At the observed 36.06 percent the oscillator is using about
**7.2x10^-8** of its **+/- 2.0x10^-7** electronic range, leaving roughly
1.3x10^-7 in the direction it has been moving.  Against a typical
1x10^-8 per year of aging after the first year, that is on the order of
a decade of headroom.  The receiver's own hardware register agrees:
neither the near-full-scale nor the full-scale EFC bit is set.

## The measured voltage does not fit

About 52 mV was measured across the EFC coax, centre to shield, while
`RELative` read 36.06 percent.  If the reported percentage spans the
oscillator's -5 V to +5 V input, that point should read **+1.803 V**.
The measurement is smaller by a factor of about 35.

Three explanations, in the order worth checking:

1. **The wrong coax.**  The 10811 has two SMB snap-on connectors, one
   for the 10 MHz output and one for EFC.  A meter on the output would
   read near zero.
2. **A gain stage.**  If the DAC swings a few hundred millivolts and an
   amplifier scales it to +/- 5 V, then 52 mV is the DAC side and the
   oscillator sees roughly 35 times more.
3. **A restricted window.**  If the receiver only drives a fraction of
   the +/- 5 V span, its pull would be a fraction of 2x10^-7 -- but with
   aging up to 1x10^-7 per year it would then rail within a year or two,
   which a product meant to run unattended would not do.

The prediction is testable with the meter already in hand: at 36.06
percent, the oscillator's EFC pin should sit near +1.80 V DC.

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
