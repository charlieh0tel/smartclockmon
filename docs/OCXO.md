# The oscillator

The development 58503A carries an **HP 10811-60159**.  This collects
what is known about it, and what is not.

## Identification

`*IDN?` does not report the oscillator, and no SCPI command does; the
part number came from the unit itself.  `10811-90027-1` section 12
defines it as:

> **10811-60159** — 10811-60158 with shock mount studs
>
> The performance specifications for the 10811-60158/60159 are the same
> as the 10811D/E with the following exceptions.

So it is a 10811D/E with a tighter frequency-adjustment and aging
specification, in a shock-mounted package.

## Specification

From `10811-90027-1` section 12, with the 10811D/E base in section 1 for
comparison.

| | 10811D/E base | **-60158 / -60159** |
| - | ------------- | ------------------- |
| Coarse tuning range | > ±1x10^-6 (±10 Hz) | > ±5x10^-7 (±5 Hz) |
| **EFC, over -5 V to +5 V** | >= 1x10^-7 (1 Hz) total | **> ±2.0x10^-7 (±2.5 Hz)** |
| Aging, per day | < 5x10^-10 | < 2.5x10^-10 |
| Aging, per year | < 1x10^-7, typically 1x10^-8 after the first year | same |

Output is 10.000000 MHz at 0.55 V +/- 0.05 V rms into 50 ohms, harmonic
distortion < -25 dBc.

### Allan deviation, -60159

| Averaging time | sigma_y(tau) |
| -------------- | ------------ |
| 0.001 s | < 1.5x10^-10 |
| 0.01 s | < 1.5x10^-11 |
| 0.1 s | < 5.0x10^-12 |
| 1 s | < 9.8x10^-13 |
| 10 s | < 5.0x10^-12 |
| 100 s | < 1.0x10^-11 |
| 1000 s | < 1.0x10^-11 typical |

### Phase noise, -60159

| Offset | dBc/Hz |
| ------ | ------ |
| 1 Hz | < -95 |
| 10 Hz | < -125 |
| 100 Hz | < -135 |
| 1 kHz | < -145 |
| 10 kHz | < -150 |

### Power

Oven circuit 12 to 30 Vdc, 11 W maximum at turn on, dropping to about
2 W steady state at 25 C in still air at 20 V.  That turn-on surge is
what `:DIAGnostic:ROSCillator:CURRent?` is watching.

Warm-up, from the 10811A/B manual: within 5x10^-9 of final value 10
minutes after turn-on at 25 C and 20 Vdc, where final value means the
frequency 24 hours after turn-on.

## Connections

The 10811 brings out 10 MHz and EFC on two **SMB snap-on coax**
connectors.  They are identical, which matters: a meter on the wrong one
reads near zero rather than the expected EFC voltage.

## What this means for the receiver

The EFC input is a -5 V to +5 V span covering at least ±2.0x10^-7,
which across the 2^20 counts of the receiver's internal value is about
3.8x10^-13 per count.

On this unit both ends of that are measured (`efc.md`).  The receiver
drives the pin at 6.33 uV per count, crossing 0 V at +37.6 percent and
reaching 4.568 V at count 0.  The oscillator pulls **-6.22x10^-8 per
volt**, 1.55 times the specified minimum, which is **3.94x10^-13 per
count**.  Since a retrim on 2026-09-26, made with the EFC input
grounded, the receiver locks with the pin within millivolts of 0 V, and
has 2.84x10^-7 of pull below it, measured.

The EFC input must be grounded, not left floating, when the crystal is
trimmed: a trim against a floating input landed 3.7x10^-7 away from
where the receiver needed it, and the receiver drove its EFC to full
scale trying to reach it.

## The Z3801A's oscillator is a different part

The Z3801A is reported to use a **10811-60161**, which appears in none
of the sources here: not among the 27 variants in `10811-90027-1`, and
not among the six on the etoysbox page.  Van Baak measured 5.2x10^-13
per EFC count on one, implying about ±2.7x10^-7, a third more per count
than this unit's measured 3.94x10^-13.  That is a measurement of a single oscillator, not a
specification, and should be treated as indicative.

## Sources

- `third_party/10811-variants-90027-1.pdf` — HP drawing A-10811-90027-1
  rev H, 26 October 1999.  27 variants; the -60159 is section 12.
- `third_party/HP-10811AB-Manual.pdf` — HP 10811A/B operating and
  service manual.  Base part specification, EFC description, warm-up.
- <http://etoysbox.jp/Memo/3_Test_Equipments/HP_10811_OCXO/HP_10811_OCXO_Spec.html>
  — independent listing; gives the -60159 the same coarse tuning range
  and the same EFC figure over the same -5 V to +5 V input.
- <http://www.leapsecond.com/pages/z3801a-efc/> — Tom Van Baak on the
  Z3801A's EFC: the 20-bit internal value, the 16-bit AD569 with
  dithered low bits, and 5.2x10^-13 per count measured against a
  counter.
