# The ovens

Both images name a `doven` console word, and the Z3801A's health
monitor names a "Secondary oven voltage".  The processor switches the
outer oven of a double-oven oscillator; it does not regulate it.

## The switch

`doven` (`0x2bf5e`) sets or clears bit 5 of the port at `0xfff907`.
Two other places set that bit and nothing else clears it:

- the power-up state machine, `FUN_000498a8`, on entering its state 2,
  `external oven warmup` (`0x4996c`);
- the recovery state machine, `FUN_00049bc2`, on entering its state 0
  (`0x49c00`).

Both also set the byte at `0x10222b` to 1, which nothing reads.  The
status screen's `Oven Pwr` field comes from a different routine,
`0x4e9d2`, not traced.

## When it is switched on

The power-up machine's states, named by the print switch at `0x2bbe8`:

| State | Name | Leaves when |
| ----- | ---- | ----------- |
| 0 | start | always; runs `FUN_00048aea(1)`, below |
| 1 | internal oven warmup | `FUN_0003254a` returns true |
| 2 | external oven warmup | always, after one interval reading; posts event `0x24` |
| 3 | warmed up, waiting for GPS | `0x102c23` set, `0x102229` clear, bit 5 of `0x10283d` clear, byte at `0x102852` ≥ 14 |
| 4 | coarse | `FUN_00048bec` returns 1 |
| 5 | fine | `FUN_00048db6` (above) returns 1 |
| 6, 7 | waiting; calculating leapseconds | -- |

`FUN_0003254a` makes the decision alone: it returns true when the
float at `0x102358` exceeds 0.9.  That float is field `+0x08` of
health-monitor record 6, the oscillator current's (records are 48
bytes at `0x102230`; see below), and the channel's own routine
`FUN_00032048` sets it to 1.0 when the current has settled: every 75th
call it takes the change in the filtered current since the last such
call and marks the channel settled if the change is greater than −10
and the current is below 650 -- the channel descriptor's two limits --
or if the seconds counter at `0x100c08` has passed 900.  On the
channel's first reading the flag is cleared and the change seeded at
−10.

So the outer oven is powered once the inner oven's current has stopped
falling, or after fifteen minutes regardless.  State 2 lasts one
reading; nothing waits for the outer oven to warm, and nothing measures
it afterwards.

## The Z3801A

The same two machines (`FUN_00045ec6` at `0x45f84`, `FUN_000461c0` at
`0x461fe`) set the same bit and a flag at `0x101b6f`.  Its decision,
`FUN_000230b8`, reads health-monitor channel 5 -- named `Oven` by the
console's print word (`0x1adfe`) -- and returns true when the filtered
value is below −2.0.  That channel's reader, the `adc_oven` word,
computes 0.0743·ADC₅ − 0.0472·ADC₄ (`0x1f0b6`); what the two inputs
are was not traced.

The Z3801A also monitors a "Secondary oven voltage": state 1 of its
supply monitor `FUN_00022c44` reads the `adc_doven` word
(0.0743·ADC₁ − 0.0472·ADC₄), but only while `0x101b6f` is set -- once
the outer oven is on -- and raises the alarm above 6.8, with no lower
limit.  The Z3816A's monitor has no oven channel (`loop.md`, "s, the
oscillator current").

## What others have measured

None of the following was measured here; owners reported it, and it
agrees with the code.

- On a Z3805A the outer oven's enable is pin 8 of the power board's
  connector P2: 0 V with the heater off, about 4.5 V with it on, and
  pulling it to +5 V by hand turned the heater on, after which it was
  "being controlled by temperature controller on the power board", with
  the controller's test point TP104 "modulating around 15.75 V".  The
  processor reads the heater voltage back on P2 pin 9: 1.95 V on a
  working unit, 8.14 V on the faulty one.  The heater measured 19 Ω and
  had 15.27 V across it when working.  (John Stuart, time-nuts, April
  2014: <https://www.febo.com/pipermail/time-nuts/2014-April/084494.html>,
  `.../084502.html`, `.../084512.html`, `.../084517.html`.)  Jarl Risum
  wrote in the same thread that his Z3805A's outer oven circuit is
  identical to the Z3801's, and that the processor monitors the heater
  voltage through P2/9.
- A page on the Z3801A describes the same pin: a logic level on P2 pin
  8 that turns the heater voltage off at zero and needs more than 2.4 V
  to enable, with TP104 near 5 V when off and 14 to 16.2 V when
  working.  Its author found units in which the processor never raised
  the pin, and recovered them by forcing it high until the firmware
  did.  (<https://www.realhamradio.com/oven-confusion.htm>.)
- Whether the outer oven then runs continuously is disputed on
  time-nuts: Bob Camp called it "simply a warmup heater" that "drops
  out in normal operation"
  (<https://www.febo.com/pipermail/time-nuts/2013-July/078414.html>);
  Jarl Risum wrote that it holds 60 to 65 °C in normal operation
  (<https://febo.com/pipermail/time-nuts_lists.febo.com/2020-April/099815.html>).
  The code settles only the processor's part: once set, the enable is
  never cleared, so what the heater does after that is up to the
  analog controller.
- An owner's "Z3801A Outer Oven Description", with a schematic drawn
  from their own unit, was on ko4bb.com and survives as a PDF attached
  to a time-nuts message of December 2022
  (<https://febo.com/pipermail/time-nuts_lists.febo.com/2022-December/106993.html>);
  a copy is `third_party/community/Z3801A-Outer-Oven-Description.pdf`.  The page
  does not name its author; the PDF's metadata names David G. Mason as
  its maker, in 2018.  It places the whole controller on the
  power-supply board: an AD586 reference feeding a Wheatstone bridge
  whose NTC (100 kΩ at 25 °C, β 4850, so 16.21 kΩ at about 62.5 °C)
  is in the outer oven; an LT1077 op-amp "used as a PI servo
  controller"; and an LT1270 boost regulator that drives the 18.9 Ω
  heater directly, from 0 to about 11 W.  The processor's part is a
  DG211 analog switch: "The main CPU controls the oven through P2/8.
  If the voltage at U103/pin 1 is below ~2.4V the non-inverting input
  of U102 is pulled towards +15V, and the output pin 6 saturates
  instantly near 13.5V.  In turn, U104 shuts down and heater power is
  zero.  On the main CPU board the control pin has a pull-down resistor
  to ground (10k), and a 1k series resistor to pin 10 / PGP5 of the
  main CPU U33 / MC68331."  The servo output "is available at P2/9",
  divided to 3.27 to 4.22 V into pin 3 of "the 8-bit AD-converter U35 /
  ADC0838", which "allows the main CPU to measure the percentage of
  heating power in the ON state".

The description agrees with the code.  PGP5 is bit 5 of the GPT's
port GP, the `0xfff907` bit the firmware sets; the heater is off until
the processor raises it, regulated by the analog PI loop once it does,
and never turned off again by the firmware.  P2/9, the servo output, is
what the Z3801A's firmware watches as "Secondary oven voltage" once the
oven is on; its limit of 6.8 is in the units of the firmware's channel
reader, not volts at P2/9, and the two scales were not related.  The
firmware's ADC reader also talks to an ADC0838: it sends a start bit
and channel select over the QSPI and keeps eight bits of the reply.

## What `hdac` is not

The console words `hdac_write` and `hdac_all` (`0x2b2ec`, `0x2b302`)
write six 6-bit values, packed two to a word into a buffer at
`0x1036aa` and sent as QSPI device 2 (`0x2e460`, `0x2e4e2`).  Apart
from the console, the only code that writes them is the time-interval
counter's interpolator calibration, `FUN_00043b56`: it steps DAC
channels 2 and 3 (`0x43a9c`, `0x439be`) until the interpolator's
readings at its two reference points fall in 1..25 and 201..225 with a
spread of exactly 200, and stores the result.  `FUN_00048aea` runs it,
called with 1 by the power-up machine's state 0 and with 0 by the
recovery machine.  These DACs trim the counter, not an oven.

## The health monitor

Task `hmon`'s loop is `FUN_00033dc8`.  Every tenth pass it advances
`FUN_00032414` one channel through eight descriptors of 42 bytes at
`0x32596`:

| Offset | Holds |
| ------ | ----- |
| +0 | raw-to-value routine: supplies are ADC × 0.0763; the oscillator current is ADC × 4.489 |
| +4 | per-call routine; `FUN_00031f3e` for most, `FUN_00032048` for the oscillator current |
| +8, +0x10 | check and act routines, present for the oscillator current only |
| +0x14, +0x18 | low and high limits: 11..13, 4.5..5.5, 11..13, −12.5..−10.5 ×3, −10..650, 0..0 |
| +0x1c | the value reported while the channel is disabled: 12, 5, 12, −11.5 ×3, 250, 50 |
| +0x20 | name |
| +0x24, +0x25 | event codes posted on leaving and re-entering tolerance |

Each channel keeps a 48-byte record at `0x102230` + 48·n: the filtered
value at +0x28 (0.9 of the old, 0.1 of the new, per `FUN_00032048` and
the Z3801A's `FUN_00022b88`), an enabled flag at +0x2c, an
out-of-tolerance flag at +0x2e, and for the oscillator current the
previous value at +0, its change at +4 and the settled flag at +8.
`FUN_000324b0(n)` returns the filtered value or the default; it is
what `pll_normal` reads for its oscillator-current term.  The same
loop counts 1 PPS edges through the `0xfff90c`/`0xfff90d` counter
every 0xcc passes (`FUN_00033ad0`: `GPS 1pps signal is dead`, `...
frequency is incorrect`, expecting 8 to 12 in ten samples) and does
the same for an internal 1 PPS (`FUN_00033c4a`, setting `0x102229`,
which the loop treats as an error).

## In every image

Every image has the `doven` console word, setting and clearing bit 5
of `0xfff907` (an `ori.b #0x20` and an `andi.b #0xdf`), the
`internal oven warmup` and `external oven warmup` state names and
`hdac_write`.  What differs is whether anything but the console raises
that bit.  Each image's reset code sets the port's direction register,
DDRGP at `0xfff906` (MC68331UM D.2.4: a 1 makes the pin an output), and
its data, PORTGP at `0xfff907`.  Read on 2026-10-09:

| Image | DDRGP, PORTGP at reset | Raised by the power-up and recovery machines |
| ----- | ---------------------- | -------------------------------------------- |
| Z3816A 4001 | `0xbc`, `0x80` (`0x22130`) | yes (`0x4996c`, `0x49c00`) |
| Z3801A 3543 | `0xbc`, `0x80` (`0x12068`) | yes (`0x45f84`, `0x461fe`) |
| Z3805A 3543B | `0xbc`, `0x80` (`0x12068`) | yes (`0x45f4c`, `0x461c6`) |
| 58503A 3633 | `0xbc`, `0x80` (`0x12124`) | yes (`0x46016`, `0x462aa`) |
| 58503A 3704 | `0xbc`, `0x80` (`0x121ae`) | no |
| Z3815A 4010 | `0x00`, `0x00` (`0x221c6`) | no |
| 58503B 1.01.04 | `0xbc`, `0x80` (`0x22204`) | no |

`0xbc` makes PGP5 an output and `0x80` leaves it low, so every image
but the Z3815A starts with the outer oven off.  In 3704 the two
machines are 3633's, compared word by word (3633 from `0x45f00`, 3704
from `0x46020`), with four changes besides addresses and offsets that
moved: the `ori.b #0x20,0xfff907` is gone from both, leaving nothing in
its place -- each still enters `external oven warmup` and sets its flag
(`0x101e75`, at `0x46136` and `0x463c6`), and the recovery machine's
jump table is eight bytes, that instruction, shorter; a status byte at
+0x11 of the loop block became a word at +0x10, as in 3704's
`pll_normal`; a `clr.b 0x102862`, 3704's interval-mean flag
(`interval.md`), was added; and one `tst.b (A2)` was dropped.  So in
3704 and the 58503B only `doven` raises the outer oven's enable.  The Z3815A makes all of port GP inputs at reset, and
nothing in its image changes DDRGP afterwards, so its `doven` writes a
pin that does not drive.

The Z3801A's "Secondary oven voltage" monitor is in the Z3801A and
Z3805A images only; 58503A 3633 has the `adc_doven` word that reads
it, but not the message.  The channel names are in `loop.md`, "In
every image".
