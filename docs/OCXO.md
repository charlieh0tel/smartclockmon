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

The EFC input is a -5 V to +5 V span covering ±2.0x10^-7.  The receiver
reports its position on that span as a percentage, and across the 2^20
counts of its internal value that works out to about **3.8x10^-13 per
count**, assuming it drives the whole range.

At the 36.06 percent observed, the oscillator is using **7.2x10^-8** of
its **±2.0x10^-7**, leaving roughly 1.3x10^-7 in the direction it has
been moving.  Against a typical 1x10^-8 per year of aging after the
first year, that is on the order of a decade of headroom.  The
receiver's hardware condition register agrees: neither the
near-full-scale nor the full-scale EFC bit is set.

See `efc.md` for how the receiver reports and scales that value.

## The Z3801A's oscillator is a different part

The Z3801A is reported to use a **10811-60161**, which appears in none
of the sources here: not among the 27 variants in `10811-90027-1`, and
not among the six on the etoysbox page.  Van Baak measured 5.2x10^-13
per EFC count on one, implying about ±2.7x10^-7, a third wider than the
-60159.  That is a measurement of a single oscillator, not a
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
