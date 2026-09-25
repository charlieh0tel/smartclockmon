# What the firmware shows

What `third_party/z3816a-4001.bin` shows about how the receiver measures the
1 PPS time interval, how it disciplines the oscillator from it, and
whether the Oncore's sawtooth correction enters either.  Every address
below is in that image, which is loaded at address zero.

The processor is a CPU32 part: the reset code at `0x400` sets the
vector base with `movec` and programs the System Integration Module's
registers at `0xfffa00`, and the QSM at `0xfffc00` carries the SCI and
QSPI.  It also initialises and uses a register block at `0xfff900`
(`0x22118` onwards; a port whose bit 5 the firmware switches, and a
counter at `0xfff90c`/`0xfff90d` it reads to count 1 PPS edges).  That
is the General-Purpose Timer module, which the 68331 has alongside its
SIM and QSM (NXP's MC68331 page, <https://www.nxp.com/products/MC68331>)
and the 68332 does not -- the 68332 has a TPU there instead.  The
MC68331 User's Manual (`third_party/MC68331UM.pdf`) places the SIM at
$YFFA00 (Table D-3), the GPT at $YFF900 with the port GP data register
PORTGP at $YFF907 (Table D-2, D.5) and the QSM at $YFFC00 (Table
D-13), which is where this image finds them; bit 5 of PORTGP is the
pin PGP5/OC3/OC1 (section 7.2).  An owner's description of the
Z3801A's outer-oven circuit names the main CPU as "U33 / MC68331" and
the oven's control line as PGP5 (see "The ovens").

The reset code at `0x2466e` sets the SIM up as follows (register
names and encodings from MC68331UM appendix D; block sizes from Table
D-11):

| Register | Value | Meaning |
| -------- | ----- | ------- |
| SYPCR `0xfffa21` | `0xcc` | SWE, SWP, HME, BME: software watchdog on and prescaled by 512, halt and bus monitors on (D.3.12) |
| CSBARBT `0xfffa48` | `0x0006` | boot chip select: `0x000000`, 512 KB -- the ROM this image is |
| CSBAR0, 2, 3 | `0x1003` | `0x100000`, 64 KB -- the RAM: CSOR0 `0x6830` reads both bytes, CSOR2 `0x5030` writes the upper, CSOR3 `0x3030` the lower |
| CSBAR1 | `0x3000` | `0x300000`, 2 KB |
| CSBAR4 | `0xfff8` | `0xfff800`, 2 KB |
| CSBAR5 | `0x3040` | `0x304000`, 2 KB |
| CSBAR6 | `0x0006` | `0x000000`, 512 KB, CSOR6 `0x7070`: write only -- the boot ROM's own range, for writing it |
| CSBAR7 | `0x3020` | `0x302000`, 2 KB -- the word G is chosen by; CSOR7 `0x6870`: both bytes, read only, one wait state, and CSPAR1 `0x2af` makes CS7 a 16-bit port (Tables D-9, D-10, D-12) |
| CSBAR8 | `0x2000` | `0x200000`, 2 KB -- the DUART |
| CSBAR9 | `0x4001` | `0x400000`, 8 KB -- the checksummed block at `0x400080` |
| CSBAR10 | `0x5000` | `0x500000`, 2 KB |
| SYNCR `0xfffa04` | `0xcf80` | W = 1, X = 1, Y = 15: f = f_ref · 4 · 16 · 8 (section 4.3.2): 16.777 MHz from the 32.768 kHz reference the manual's frequency tables assume (4.3.2, appendix A) |
| PORTE0 `0xfffa11` | `0x40` | port E bit 6 high; `0x22e30` later pulses it low and high |

The SCI's baud rate is SCBR in SCCR0 at `0xfffc08`, f / (32 · SCBR)
(section 6.4.3.3).  `FUN_0002e610` sets it from a setting byte: 437,
218, 55 and 27 for settings 0 to 3, which at 16.777 MHz are 1200,
2405, 9533 and 19418 baud -- the manual's four rates, each within
0.7 %.  `FUN_0002ed56` sets PE and PT in SCCR1 (`0xfffc0a`) from the
byte at `0x10261d`: no parity for 0, even for 1, odd for 2.

This is the Z3816A's firmware.  No 58503A image is available, so what
follows describes the design family and is not a statement about the
58503A's own code.

## Summary

- The reported interval is **the mean of ten one-second readings**,
  stored as a single-precision float in seconds.  It changes once every
  ten seconds.
- Each reading comes from a counter in the FPGA: a coarse count of
  100 ns ticks plus an interpolator, with calibration terms applied.
- The firmware **decodes** the Oncore's negative sawtooth from the
  Time RAIM message, and prints it on a status page.  **No path was
  found from it into the interval measurement.**
- The oscillator is steered by a **proportional-integral loop on that
  same ten-second mean**, run every ten seconds.  Both gains follow
  from one time constant τ and one gain G: the proportional gain goes
  as 1/(Gτ) and the integral as 1/(4Gτ²).  **τ is 500 s** by default in
  the Z3816A and 1000 s in the Z3801A, from a block of defaults in ROM;
  after power-up the loop starts at 150 s and lengthens by 5 s every
  update until it reaches τ.  Its phase input is the
  interval mean; no read of the sawtooth was found in it.
- A separate task fits **a + b·t + c·ln t to 45-minute means of the
  EFC**, up to 64 of them (48 hours), and the loop adds the fitted
  slope to its integrator each update as predicted drift.
  `startup_pll` sets its first EFC from the same curve.
- The **outer oven** of a double-oven oscillator is switched, not
  regulated, by the firmware: one port bit, turned on when the
  oscillator current has stopped falling, and never turned off except
  from the console.  No loop in either image drives a heater.
- The firmware carries a **Forth interpreter** that calls itself
  pForth, whose words include the loop's own diagnostics.  It is a
  shell over compiled code: no word in the image is written in Forth.

## The GPS receiver link

The Oncore is on channel A of a 68681 DUART at `0x200000`, whose
sixteen registers sit on odd byte addresses (`0x200001` + 2*n).

| Function | Role |
| -------- | ---- |
| `FUN_0002dbdc` | DUART interrupt handler; reads the interrupt status at `0x20000b` and dispatches |
| `FUN_0002dab2` | channel A receive; copies bytes from `0x200007` into a 256-byte ring at `0x103595` (write index `0x103593`, read index `0x103594`) and posts event `0x4000` |
| `FUN_0002d6dc` | takes one byte from the ring |
| `FUN_00050314` | the message framer |

Channel B's receive routine (`FUN_0002db42`) records error bits only
and buffers no data; `FUN_0002dc9a` is a channel B loopback self-test
that sends the string `DUART` and checks the echo.

### Framing

`FUN_00050314` is a seven-state machine, its state at `0x1016e7`:

| State | Does |
| ----- | ---- |
| 0, 1 | expect `@` |
| 2 | first ID character |
| 3 | second ID character, then look the ID up for its length |
| 4 | body bytes; at the end, checksum (`FUN_00055e84`) against the last byte |
| 5, 6 | expect CR, then LF |

A complete message is left in the buffer at `0x1014dc`, starting with
the `@@`.

### The message table

`FUN_00050286` looks an ID up in a table of 64 descriptors of 60 bytes
at `0x56732` (`FUN_000560fc` returns descriptor *i* as
`0x56732 + 60 * i`).  Each holds a pointer to its two-character ID at
+0, its length at +28, a decoder at +0x20 and a post-handler at +0x38.
The two Time RAIM messages are there with the lengths the VP Oncore
reference gives:

| Entry | ID | Length | Decoder | Post-handler |
| ----- | -- | ------ | ------- | ------------ |
| 41 | `@@Bn`, six channels | 59 | `0x55bb0` | `0x5605a` |
| 53 | `@@En`, eight channels | 69 | `0x55c40` | `0x5605a` |

`FUN_00055f68`, reached from the post-handler, accepts `Bn` for a
six-channel receiver and `En` for an eight-channel one, by the channel
count at `0x100ebc + 0x606`.

### Decoding

Both decoders unpack through `FUN_00054ec8`, driven by a byte program
rather than by code, which is why no instruction reads a field of the
raw message directly.  The program's opcodes:

| Byte | Meaning |
| ---- | ------- |
| 1..127 | copy that many bytes |
| `0x00` | skip one destination byte |
| `0x80` | end |
| `0x81` | end of a repeat group |
| `0x82` | skip one source byte |
| `0x83` | skip one source byte, write zero |
| `0x84` | write a zero byte |
| `0x85` | write a sign-extension byte from the next source byte |
| other negative | start a repeat group of that count |

The `En` program, at `0x57e39`:

    02 84 84 03 00 84 03 04 00 0a 00 f8 01 00 04 81 80

Walked against the message layout in `VPCommands.pdf` (message byte 0
being the first `@`), its fifth copy moves message bytes 16 to 25 --
hours, minutes, seconds, pulse status, 1 PPS sync, Time RAIM solution
status, Time RAIM status, the two-byte one-sigma estimate, and the
negative sawtooth -- to record offsets 17 to 26.  The `Bn` program at
`0x57e25` is the same apart from its channel count.  **The sawtooth is
at offset 26 of the decoded record.**

### Where the sawtooth is read

The one reader of offset 26 found is `FUN_0004c062`, at `0x4c2b4`,
which sign-extends it and prints it on a Time RAIM status page whose
format strings are:

    ALARM LIM  %-24s   TIME REF  %s
    SIGMA EST  %-24s   SAWT ERR  %+d ns

`SIGMA EST` is read from record offset 24 and `SAWT ERR` from offset
26, which agrees with the layout above.  The page is reached through a
pointer table; the command that shows it has not been identified.

## The interval

### What the query returns

Three nodes in the command tree are named `TINTerval`.  Walking their
parents:

| Node | Path | Query handler |
| ---- | ---- | ------------- |
| `0x5f854` | `:SOURce:SYNChronization:TINTerval` | `FUN_0003f8fa` |
| `0x60dbc` | `TINTerval`, parents not resolved | `FUN_0003f8fa` |
| `0x63fb2` | `...:PTIMe:TINTerval` | `FUN_0003b052` |

`:SOURce` is optional, so the first is `:SYNChronization:TINTerval?`.
Its handler copies the 32-bit value at `0x102c0c` into its reply,
tagged with the halfword `0xfff6` (−10), when the flag at `0x102c10` is
set.  That value is a single-precision float in seconds: `pll_normal`
stores it there as the float sum of the readings divided by their
count, converted to float (below).  The receiver prints the value to
10⁻¹⁰ s: seven readings -- one in a recorded transcript, six from a
58503A through the daemon -- are all whole tenths of a nanosecond,
which is what a −10 resolution would give.  That the reply formatter
uses the tag that way was not traced.

The `PTIMe` node's handler, `FUN_0003b052`, answers from a different
place: under the lock `FUN_00023488` takes, it converts the double at
`0x102666` -- the latest one-second reading, see "One reading" below
-- to a float for the reply, with the halfword at `0x102670` as its
tag, when the byte at `0x10266e` is set, and otherwise fails with code
0xc.  So `:PTIMe:TINTerval?` is one reading and
`:SYNChronization:TINTerval?` is the mean of ten.

### The ten-second average

`FUN_0004824a` writes both.  Each second it takes a reading and, if it
is valid, adds it to a running sum with a count at `0x102735`.  A
counter at `0x102734` runs to ten; at ten, the sum is divided by the
count, the flag is set and the mean is stored at `0x102c0c`.

Measured on a 58503A, 397 of 400 consecutive distinct values were each
held for exactly ten one-second polls, which is this schedule seen from
outside.

### One reading

`FUN_000489de` calls `FUN_0004467a`, which fills a double at
`0x102666`:

1. Two readiness checks (`0x44256`) poll an FPGA status byte, up to 512
   times each.
2. `FUN_00044542` reads the coarse count as three bytes, and
   `FUN_00044ae2` converts it, with its sign, to a double.
3. `FUN_00044592` reads the interpolator; an offset is subtracted and
   the result divided by a scale, both from a calibration block, and
   added to the count.
4. The sum is multiplied by `0x3e7ad7f29abcaf48`, which is 1.0e-7: the
   coarse count is of 100 ns ticks, a 10 MHz clock.
5. A calibration double is subtracted.
6. The long at `0x1023d0` is converted and added.

`0x1023d0` is not a live value.  `FUN_0004045c` loads it, with
`0x1023cc` and `0x1023d4`, from a checksummed 64-byte block at
`0x400080`, and `FUN_000404be` writes them back: it is a stored
constant.

On failure the reading is replaced by a sentinel double and one of four
error codes is logged.

### The FPGA

The counter is read through a port at `0x304000`.  `FUN_000440a6`
writes a register index into a control byte, shadowed at `0x102673`,
and reads back four bits; `FUN_000444e2` reads two nibbles for a byte,
and the count is assembled from three bytes, register selectors `0x16`,
`0x14` and `0x12`.

## The disciplining loop

### The stages

The firmware's names for them, from its state strings at `0x2c5be` to
`0x2c758`: `coarse`, then `fine` -- with the sub-states `fine start`,
`fine sync meas`, `fine sync 1`, `fine sync 2`, `fine meas 1`,
`fine meas delay`, `fine meas 2`, `fine slew`, `fine slew meas`,
`fine fine slew` -- then `startup pll`, then `normal pll`.

The stage lives in the byte at `0x10282c`, the first of the loop's
state block.  The console word that prints `PLL state: ` switches on it
at `0x2bba8`:

| Value | Name |
| ----- | ---- |
| 0 | `invalid` |
| 1 | `powerup`, with sub-states from `start` through `checking time` (`0x2c67f` to `0x2c74c`) |
| 2 | `powerup recovery` |
| 3 | `holdover` |
| 4 | `holdover recovery` |
| 5 | `startup pll` |
| 6 | `normal pll` |
| 7 | `diag` |
| 8 | `idle` |
| 9 | `fatal error` |

The pSOS tasks are created at `0x231f0` to `0x232d4`: `gpsm` (entry
`0x50bf2`), `hmon` (`0x33dc4`), `pllp` (`FUN_0004b088`, task id at
`0x103d4e`) and `curv` (`0x450d4`, task id at `0x103d5e`); the SCPI
task, `sci` (entry `0x39706`, task id at `0x103d5a`), is created by
`FUN_00039732`.  The Z3805A's `:DIAGnostic:OS:PROCess?` lists the same
names (see `z3801-tree.md`).
The loop below runs in `pllp`; the fit in "The aging fit" runs in
`curv`.

### Fine acquisition

`FUN_00048db6` is a state machine that takes fifty one-second readings
of the interval (state 5, `DAT_00102845 < 0x33`), accumulates the sums
of a straight-line fit, and sets the EFC from the fitted slope -- the
frequency offset.  Its messages say so:

    i= %d, ti= %.1f, y0= %.2e, x0= %.2e
    y0 = %f, efc =  %d
    fine - ti too far off
    fine - frequency too far off
    Bad TI in frequency measurement

### The loop

`FUN_0004824a` is `pll_normal`: its failure
message is `pll_normal - Error with measurement` (`0x487f8`) and its
report is

    %d/%d/%d %02d:%02d:%02d efc= %d ti= %.1f loop= %d curr= %.2f

It keeps its state in a block at `0x10282c`.  Each second it adds that
second's reading (see "One reading" above) to a sum at `0x10272c`;
every tenth second it runs the update below, in single-precision
floating point.  Written out, with the addresses the values live at:

| Symbol | Address | What it is |
| ------ | ------- | ---------- |
| x̄ | `0x102c0c` | the mean of the ten readings -- what the query returns |
| x₀ | `0x102c1c` | the setpoint subtracted from it: cleared at `0x4b1dc`, otherwise written only by the console word `phase_off` (`0x2b87c`), which nothing else calls |
| f | `0x102be0` | the filtered error, kept between updates |
| I | `0x102728` | the integrator |
| B | `0x102be4` | a base EFC |
| τ | `0x102548` | the loop's time constant |
| G | `0x102c28` | a gain; `0x102c30` and `0x102c34` hold 1/G |
| c | `0x1023cc` | a constant from the checksummed block at `0x400080` |
| s | -- | the oscillator current: `FUN_000324b0(6)`, channel 6 of 8 measured channels |
| M | `0x102c38` | a clamp, set to 6.25 × 10⁻¹⁰ / \|G\| |
| u | `0x10285e` | the EFC value the update produces |

The update:

    e  = x̄ − x₀
    f ← (1 − a)·f + a·e                     a = 29.75 / τ
    d  = clamp(p·2700 / q + r, −M, M)        p = [0x102bdc], r = [0x102bd8],
                                             q = [0x100c08], seconds
    I ← I + 10·k·(f + d / (2700·k))          k = 1 / (4·G·τ²)
    u  = K·f + B + I + c·s                   K = 1 / (G·τ)

and `FUN_00033798(u)` converts the result.  On entry the integrator is
cleared and B is set to the EFC in force less c·s, so that starting the
loop does not move the oscillator.

q is the seconds counter `FUN_00023818` returns, which `FUN_00023624`
advances once a second; it lies in the region a warm restart preserves
(see "τ and G").  p and r are two coefficients of a fit to the EFC's
own history, described under "The aging fit" below: with y = q / 2700,
d = b + c / y is the slope of the fitted curve a + b·y + c·ln y at the
present moment, in EFC units per 2700 s.  The integrator's second term
is then 10·d / 2700: the EFC change the fit predicts over the ten
seconds between updates.

That is a proportional-integral loop on the prefiltered interval error,
with K ∝ 1/τ and the integral gain ∝ 1/(4τ²).  Taking the oscillator as
the integrator that turns an EFC change into phase, with G as its gain,
the loop gains are 1/τ and 1/(4τ²), which in the continuous
approximation is a second-order loop with damping ratio 1.  The factor
10 matches the ten seconds between updates.

### Starting up

`startup_pll`, at `0x47bac`, runs before `pll_normal` and applies the
same update law, with two differences: its time constant is its own,
at `0x102be8`, and it recomputes K, k and a from it on every update
rather than once on entry.  That time constant is set to 150 s at
`0x47af6` and `0x4af58`.

Its first state counts ten-second means whose magnitude is below
150 ns -- the double 1.5 × 10⁻⁷, loaded as two halves at `0x47e60` and
`0x47e66` -- and clears the count at the first that is not; sixteen in
a row (`0x47e46`) end the state.  Then, at `0x47ede` to `0x47fce`, it
lengthens its constant by 5 s (`0x47efe`) each update until that is
within 5 s of τ, sets it to τ, and hands over to `pll_normal`.  The
Z3801A's `FUN_000442ca` does the same.

At `0x475b8` to `0x476b4` it also sets the EFC from the aging fit
below: u = FUN_00033798(a + b·y + c·ln y + c·s), with y = q / 2700 and
the ln term left out when y ≤ 1.  Which of its states runs that code
was not traced.

### The aging fit

The `curv` task fits a curve to the EFC's history, and the loop above
feeds the curve's slope into its integrator.

**Sampling.**  `FUN_00044cbe`, which `pll_normal` calls on each pass
(`0x48a66`, `0x48be2`), works on a ring of 64 samples at `0x102876`:
tail byte at `0x102876`, head byte at `0x102877`, and four arrays of
64 -- e at `0x102880`, weight bytes at `0x102980`, y at `0x1029c0`, w at
`0x102ac0`.  While the stage is 6, or 5 with the byte at `0x102838` not
1, each call adds u − c·s -- the EFC in force at `0x10285e` less the
oscillator-current term -- to a sum at `0x102bc4` and counts it at
`0x10287e`.  When q reaches the deadline at `0x102bc0` the deadline
moves on by 2700 s and, if anything was counted, one sample is
written at the head:

    e = sum / count
    y = q / 2700
    w = y·ln(y / (y − 1)) + ln(y − 1) − 1        or −1 when q ≤ 2700
    weight = 3 if count = 2700; 2 if count ≥ 675; else 1;
             and 1 regardless while q < 10800

then the sum and count are cleared and event 0x100 is sent to `curv`
(`FUN_0004507a`).  With the debug byte at `0x102c13` set, each sample
is printed as `e_avg= %.1f time= %f weight= %d log= %.2f tfom= %.1e`
(`0x463a6`), time being y / 32 -- days.

w is ∫ ln t dt over the window from y − 1 to y, since ∫ ln t dt =
t·ln t − t: the mean of ln t over the 2700 s the sample averages.  So
the curve fitted below, a + b·y + c·w, is the window mean of
a + b·t + c·ln t.

**The fit.**  `FUN_00045116` runs on each event.  Its record is at
`0x1026a2`: mode byte at +0, a at +0x12, b at +0x16, c at +0x1a, rms at
+0x22, HQ at +0x26; its debug lines are `pts= %d a= %.1f b= %.1f c=
%.1f rms= %.1f` (`0x46415`), `HQ= %.1e mode= %d` (`0x46441`) and
`failed line fit` (`0x46455`).  It counts n₂, the samples with weight
2 or more, and n₁, weight 1 or more, and finds the oldest and newest y
(`FUN_00045680`).  Then, first match wins:

| Mode | When | Fit |
| ---- | ---- | --- |
| 4 | n₂ = 64 and the newest y > 128 | b = (Σe over the newest 32 − Σe over the oldest 32) / 1024, a = ē − b·(y_newest − 32.5), c = 0 (`FUN_00045392`) |
| 3 | the oldest y > 128 and n₂ > 2 | a straight line through the weight-2 samples (`FUN_00045ce0`), c = 0 |
| 3 or 2 | n₂ > 5 | the straight line, then the three-term fit (`FUN_000455de`); the latter is taken, and the mode is 2, when its rms is below 0.75 of the line's or n₂ < 16 |
| 3 or 2 | n₁ > 9 | the same on the weight-1 samples |
| 1 | n₁ > 2 | the straight line through the weight-1 samples |
| 0 | otherwise | a = ē, b = c = 0 (`FUN_00045570`) |

`FUN_00045ce0` is an unweighted least-squares line of e on y,
returning the slope, the intercept and the rms of the residuals over
n − 2; it fails, and clears its results, with fewer than three points
or no spread in y.  The three-term fit forms a first c and a step from
the line (`FUN_00045782`), refits the line to e − c·w with c moved by
that step while the rms falls (`FUN_0004587a`), and sets c from the
last three trials (`FUN_00045a78`).  Every fit but mode 0 ends by
resetting a so that the curve passes through the newest sample
(`FUN_0004571c`).  HQ is `FUN_00045f94`'s result for modes 2 to 4 and
4.32 × 10⁻⁴ for modes 0 and 1; what it measures was not traced.  128 in
units of 2700 s is 96 hours.

**Back to the loop.**  `curv` then sends event 0x100 to `pllp`.  At the
start of each sampling pass `FUN_0004508e` asks for that event
(`FUN_000274d6`) and, when it has arrived, copies a to `0x102bd4`, b to
`0x102bd8` -- the loop's r -- c to `0x102bdc` -- its p -- and HQ to
`0x102866`.  All four sit in the loop block a warm restart preserves,
so after such a restart the loop starts with the last fit.  After a
power-up they are zero, and stay zero until a fit with at least three
samples -- mode 1 or above -- so d is zero for the first hours of the
loop.  The `holdover` stage, `FUN_000473de` (its return of 1 moves
the stage byte to 4 at `0x4b36a`), seeds a with u − c·s when a is
zero (`0x47440` to `0x47460`).

The console's `last efc average = %.1f` (`0x2c3a5`) prints the newest
e; `dmes_curv` (`0x2befe`) sets the byte at `0x103d8a`.

### s, the oscillator current

`FUN_000324b0(n)` returns channel n of eight measured channels, each a
48-byte record at `0x102258` + 0x30·n: the live value when the flag at
`0x10225c` + 0x30·n is set, and otherwise a default from a ROM table at
`0x325b2` + 0x2a·n.  The channels' names follow that table at
`0x326e6`, in order:

    12B  5V  12C  -12B  -12C  -12D  Oscillator current  Antenna current

and their defaults are 12, 5, 12, −11.5, −11.5, −11.5, 250 and 50.  The
loop reads channel 6, the oscillator current, so the term c·s is a
stored constant times the oscillator current.  The Z3801A's image reads
channel 3 of its own function, `FUN_00022fd2`; its report strings list
Temperature, 5V, +15V, −15V, Oven, Double oven and Antenna current, but
which of those is its channel 3 was not traced.

### τ and G

`FUN_0002b358` sets τ from its argument.  Nothing calls it directly; a
pointer to it sits in the console's word table at `0x2d368`, beside the
name `loop_time`, and the message `max loop time = %d` (`0x2c370`)
prints τ.  It is the only code that writes τ's address, and the start-up
code clears the RAM τ lives in, so its value in service comes from
somewhere else: a 25-byte block of defaults in ROM.

τ lives at offset 0x14 of a 25-byte block at `0x102534`.  The
initialisation routine `FUN_00022c7c` fills that block one of two ways:

- from ROM, `memcpy(0x102534, 0x40174, 0x19)` (`0x22dd8`, `0x22e06`),
  whose bytes at `0x40188` are `43 fa 00 00`, the float 500.0.  The same
  routine first copies a longer block of defaults, `0x400de` to
  `0x40173`, into `0x10249e` to `0x102532`;
- or from the region `0x100000` to `0x100c3b`, which the start-up code
  does not clear (its clear runs from `0x100c3c`) and which the pllp
  task keeps writing its state into (`0x4b5fc`: the τ block to
  `0x100b82`, the loop block to `0x100622`, another to `0x100004`).
  That copy is taken when a byte checksum of the region (`FUN_00040056`
  over 0xb9c bytes) matches the word stored at its start, the flag word
  at `0x100002` is 1, the byte at `0x10262c` is clear and bits 7 and 6
  of the reset-status register at `0xfffa07` are clear -- EXT and POW
  in MC68331UM D.3.4: the reset was neither external nor a power-up
  (the other bits are SW, HLT, LOC, SYS and TST; `FUN_00022c7c` at
  `0x22dc4` also asks for the register to be exactly SYS, a RESET
  instruction); then the loop
  block, the health-monitor records and the τ block are all restored
  event 0x19 is posted (`FUN_0003ec76`) and `Power on` is written to
  the log (`FUN_00041180(1)`, `0x4b188`).  Otherwise the defaults are
  loaded and `System preset` is logged (`FUN_00041180(0x1b)`,
  `0x4b196`).

So a value set with `loop_time` survives a reset that leaves RAM
intact, and 500 s is what the loop uses after a power-up.  The startup
ramp (above) runs its own constant from 150 s to τ − 5 in steps of 5 s
per ten-second update, so it reaches 500 s about 700 s after it
begins.

The Z3801A's image does the same with its block at `0x101e6e`, filled
from ROM `0x2f846` (`0x12ce0`), whose float at offset 0x14 is
`44 7a 00 00`: 1000.0.  Its ramp from 150 s to 1000 s takes about
1700 s.

The rest of the 25-byte block, from its ROM defaults
`00 00 00 01 00 00 01 00 | 00 00 00 00 | 00 00 00 00 | 00 00 00 00 |
43 fa 00 00 | 00` and the code that names its bytes by address:

| Offset | Default | Read and written by |
| ------ | ------- | ------------------- |
| +0, word | 0 | nothing but the block copies |
| +2 | 0 | an alarm summary: `FUN_00049f76`, called from `pllp`, gathers bits from `FUN_0003ed82`, the +0x18 flag, `FUN_000324fc` and `FUN_00049f06` into the byte at `0x102c16` and sets +2 when any is set (`0x4a008`); the health monitor folds it into a status bit (`0x33d12`); a SCPI handler at `0x3d3d0` returns it |
| +3 | 1 | a descriptor at `0x431bc` only |
| +4 | 0 | a SCPI handler at `0x3d3b6` returns it |
| +5 | 0 | a SCPI handler at `0x3d458` returns it |
| +6 | 1 | set to 1 when the loop starts (`0x4af60`); consulted when holdover begins (`0x472bc`, under the stage byte's move to 3); returned by the handler at `0x3f57a`, whose node carries the keyword `REC` |
| +7 | 0 | set to 1 by the `powerup` sub-state machine `FUN_0004a34a` (`0x4a764`); tested by SCPI handlers at `0x3c330` and `0x3c6de` |
| +8, long | 0 | written by a SCPI setter (`0x3c35c`, through `FUN_00038f04`); its address is handed to the `powerup` sub-state machine (`0x4a382`) |
| +0xc, +0x10 | 0 | no reader found by address |
| +0x14, float | 500 | τ |
| +0x18 | 0 | a holdover-recovery flag: `FUN_000473ac`, called from the `holdover recovery` stage (`FUN_000478ce`, message `holdover recovery - Error with measurement`), counts its calls at `0x102bf6` and sets the flag with event 0x2e when the count passes the limit at `0x102566`; the `powerup` machine clears it (`0x4a094`) and so does the loop's start (`0x4afa0`); it is bit 0x20 of the alarm summary above |

Bytes +2 to +6 and +8 also appear, each with its address, in 28-byte
descriptor records at `0x43168` to `0x43228` and `0x4369e`, alongside
the same getter `FUN_00022bb8`; what those records serve was not
traced.  The event codes passed to `FUN_0003ec76` -- 0x19 to 0x5c,
posted to the queue at `0x10356e` -- are not the log's codes: the log
writer `FUN_00041180` takes a code from 0 to 0x1b, looks up its text --
`Log cleared`, `Power on`, `Re-boot`, ... `System preset` (`0x41562`
to `0x41834`) -- and writes the entry through `FUN_00041138` under the
lock at `0x1023fa`.

G is a constant.  `FUN_0004b088` reads the hardware word at
`0x302000` and passes −1.25 × 10⁻¹² if bit 8 is set and
−2.125 × 10⁻¹² if it is clear (`0x4b14c` to `0x4b166`); the Z3801A's
image passes a fixed +6.25 × 10⁻¹³ (`0x475bc`).  That word is the
whole of what the firmware does with chip select 7: the reset code
makes `0x302000` a 2 KB, 16-bit, read-only block with one wait state
(see the chip-select table above), `0x4b14c` is the only access to it
in the image -- every other form of the address was searched for --
and only bit 8 of the word is ever looked at.  So it is an input port
the processor reads once, when the loop task starts, and what drives
its bit 8 -- a link, a switch, or a signal from the oscillator or
DAC board -- is on the board, not in the image.  The Z3801A's reset
code sets no chip select there and its image never reads the address.
What the sign of G stands for was not traced.

`FUN_0004b022(G)` stores G, sets both gain constants to 1/G, sets M to
6.25 × 10⁻¹⁰ / |G|, and sets a second limit at `0x102c3c` to
5.787 × 10⁻¹⁴ / G -- 5.787 × 10⁻¹⁴ being 5 × 10⁻⁹ per day expressed per
second.  Its one caller is
`FUN_0004b088`.

### How this was read

The arithmetic is done by a software floating-point library, which the
decompiler shows only as calls with the operands hidden.  Each routine
was identified by running it in an emulator (Unicorn, on a 68000 core)
on known inputs, and the update above transcribed from the disassembly
of `0x4850c` to `0x48674`.  Operands go in D0 and D1 and the result
comes back in D0:

| Routine | Operation |
| ------- | --------- |
| `0x64b6a` | D0 − D1 |
| `0x64b68` | D1 − D0 |
| `0x64b8e` | D0 + D1 |
| `0x65df4` | D0 × D1 |
| `0x652ba` | D0 ÷ D1 |
| `0x652b8` | D1 ÷ D0: exchanges the two and falls into the divide |
| `0x234f2` | absolute value of the float on the stack |
| `0x65130` | compare D0 with D1 |
| `0x65ce2`, `0x65c3c` | integer to float |
| `0x65a90`, `0x65b22` | float to integer |
| `0x659f8` | float to double, in D0:D1 |
| `0x65bdc` | integer to double |
| `0x657a6` | double to float |
| `0x652ae` | divide the float at A0 by D1, in place |
| `0x64b5e` | add D1 to the float at A0, in place |
| `0x64b54` | subtract D1 from the float at A0, in place |
| `0x64e26` | double D0:D1 + double A0:A1 |
| `0x660d4` | double D0:D1 × double A0:A1 |
| `0x6553e` | double A0:A1 ÷ double D0:D1 |
| `0x6519a` | compare doubles: negative when A0:A1 < D0:D1 |
| `0x684e4` | natural log of the double on the stack |
| `0x693f0` | square root of the double on the stack |

The constants, as single-precision floats: `0x41ee0000` is 29.75,
`0x4528c000` 2700, `0x40800000` 4, `0x41200000` 10, `0x302bcc77`
6.25 × 10⁻¹⁰, `0x29824fff` 5.787 × 10⁻¹⁴, and `0x4e6e6b28` 10⁹, which
scales the interval to nanoseconds for the report.  In the aging fit:
`0x40a51800 00000000` is the double 2700, `0xbff00000 00000000` the
double −1, `0x3f400000` 0.75, `0x44800000` 1024, `0xc2020000` −32.5,
`0x42780000` 62, `0x42000000` 32, `0x39e27e0f` 4.32 × 10⁻⁴, and the
integers `0xa8c` 2700, `0x2a3` 675, `0x2a30` 10800.

## The ovens

Both images name a `doven` console word, and the Z3801A's health
monitor names a "Secondary oven voltage", so the question was whether
the processor regulates the outer oven of a double-oven oscillator.  It
does not; it switches it.

### The switch

`doven` (`0x2bf5e`) sets or clears bit 5 of the port at `0xfff907`.
Two other places set that bit and nothing else clears it:

- the power-up state machine, `FUN_000498a8`, on entering its state 2,
  `external oven warmup` (`0x4996c`);
- the recovery state machine, `FUN_00049bc2`, on entering its state 0
  (`0x49c00`).

Both also set the byte at `0x10222b` to 1, which nothing reads.  The
status screen's `Oven Pwr` field is produced by a different routine,
`0x4e9d2`, which was not traced.

### When it is switched on

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

`FUN_0003254a` is the whole of the decision: it returns true when the
float at `0x102358` exceeds 0.9.  That float is field `+0x08` of
health-monitor record 6, the oscillator current's (records are 48 bytes
at `0x102230`; see below), and it is set to 1.0 by that channel's own
routine `FUN_00032048` when the current has settled: every 75th call
it takes the change in the filtered current since the last such call
and marks the channel settled if the change is greater than −10 and the
current is below 650 -- the channel descriptor's two limits -- or if
the seconds counter at `0x100c08` has passed 900.  On the channel's
first reading the flag is cleared and the change is seeded at −10.

So the outer oven is powered once the inner oven's current has stopped
falling, or after fifteen minutes regardless.  State 2 lasts one
reading; nothing waits for the outer oven to warm, and nothing measures
it afterwards.

### The Z3801A

The same two machines (`FUN_00045ec6` at `0x45f84`, `FUN_000461c0` at
`0x461fe`) set the same bit and a flag at `0x101b6f`; its default
time constant is 1000 s, against the Z3816A's 500 s.  Its decision,
`FUN_000230b8`, reads health-monitor channel 5 -- named `Oven` by the
console's print word (`0x1adfe`) -- and returns true when the filtered
value is below −2.0.  That channel's reader, the `adc_oven` word,
computes 0.0743·ADC₅ − 0.0472·ADC₄ (`0x1f0b6`); what the two inputs
are was not traced.

The Z3801A also monitors a "Secondary oven voltage": state 1 of its
supply monitor `FUN_00022c44` reads the `adc_doven` word
(0.0743·ADC₁ − 0.0472·ADC₄), but only while `0x101b6f` is set, that is
once the outer oven is on, and raises the alarm above 6.8 with no lower
limit.  The Z3816A's monitor has no oven channel at all: its eight are
`12B`, `5V`, `12C`, `-12B`, `-12C`, `-12D`, `Oscillator current` and
`Antenna current`.

### What others have measured

None of the following was measured here; it is what owners have
reported, and it agrees with the code.

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
  (<https://time-nuts.febo.narkive.com/sZqRYuxp/10811-outer-oven-controller>);
  Jarl Risum wrote that it holds 60 to 65 °C in normal operation
  (<https://www.mail-archive.com/time-nuts@lists.febo.com/msg06566.html>).
  The code settles only the processor's part: once the enable is set it
  is never cleared, so what the heater does after that is the analogue
  controller's doing.
- An owner's "Z3801A Outer Oven Description", with a schematic drawn
  from their own unit, was on ko4bb.com and survives as a PDF attached
  to a time-nuts message of December 2022
  (<https://febo.com/pipermail/time-nuts_lists.febo.com/2022-December/106993.html>);
  a copy is `third_party/Z3801A-Outer-Oven-Description.pdf`.  The page
  does not name its author; the PDF's metadata names David G. Mason as
  its maker, in 2018.  It places the whole controller on the
  power-supply board: an AD586 reference feeding a Wheatstone bridge
  whose NTC (100 kΩ at 25 °C, β 4850, so 16.21 kΩ at about 62.5 °C)
  is in the outer oven; an LT1077 op-amp "used as a PI servo
  controller"; and an LT1270 boost regulator that drives the 18.9 Ω
  heater directly, from 0 to about 11 W.  The processor's part is a
  DG211 analogue switch: "The main CPU controls the oven through P2/8.
  If the voltage at U103/pin 1 is below ~2.4V the non-inverting input
  of U102 is pulled towards +15V, and the output pin 6 saturates
  instantly near 13.5V.  In turn, U104 shuts down and heater power is
  zero.  On the main CPU board the control pin has a pull-down resistor
  to ground (10k), and a 1k series resistor to pin 10 / PGP5 of the
  main CPU U33 / MC68331."  The servo output "is available at P2/9",
  divided to 3.27 to 4.22 V into pin 3 of "the 8-bit AD-converter U35 /
  ADC0838", which "allows the main CPU to measure the percentage of
  heating power in the ON state".

That description and the code meet exactly.  PGP5 is bit 5 of the GPT's
port GP, which is the `0xfff907` bit the firmware sets; the heater is
off until the processor raises it, regulated by the analogue PI loop
once it does, and never turned off again by the firmware.  P2/9, the
servo output, is what the Z3801A's firmware watches as "Secondary oven
voltage" once the oven is on; its limit of 6.8 is in the units the
firmware's channel reader produces, not volts at P2/9, and the two
scales were not related.  An ADC0838 is also what the firmware's ADC
reader talks to: it sends a start bit and channel select over the QSPI
and keeps eight bits of the reply.  All of this rests on the report,
not on this bench.

### What `hdac` is not

The console words `hdac_write` and `hdac_all` (`0x2b2ec`, `0x2b302`)
write six 6-bit values, packed two to a word into a buffer at
`0x1036aa` and sent as QSPI device 2 (`0x2e460`, `0x2e4e2`).  Apart
from the console, the only code that writes them is the time-interval
counter's interpolator calibration, `FUN_00043b56`: it steps DAC
channels 2 and 3 (`0x43a9c`, `0x439be`) until the interpolator's
readings at its two reference points fall in 1..25 and 201..225 with a
spread of exactly 200, and stores the result.  It is run from
`FUN_00048aea`, which the power-up machine's state 0 calls with 1 and
the recovery machine with 0.  These DACs trim the counter, and have
nothing to do with an oven.

### The health monitor

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
`FUN_000324b0(n)` returns the filtered value or the default, and is
what `pll_normal` reads for its oscillator-current term.  The same loop
counts 1 PPS edges through the `0xfff90c`/`0xfff90d` counter every
0xcc passes (`FUN_00033ad0`: `GPS 1pps signal is dead`, `... frequency
is incorrect`, expecting 8 to 12 in ten samples) and does the same for
an internal 1 PPS (`FUN_00033c4a`, setting `0x102229`, which the loop
treats as an error).

## The debug console

The firmware contains a Forth interpreter, identified by
`pForth $Revision: 1.2 $` at `0x28fa4`.

Its dictionary runs from about `0x2a400` to `0x2ad00`: 152 words, each
an entry of a link to the previous entry, a code pointer, two 16-bit
fields and the name.  The names are lower case -- `dup`, `swap`, `if`,
`do`, `loop`, `:`, `create`, `does>`, `words`, `sin`, `cos` -- and
include words that call the real-time system directly: `spawn`,
`suspend`, `resume`, `priority`, `my_pid`, `send_x`, `request_x`,
`jam_x`, `delete_x`, `signal_v`, `wait_v`, `dev_open`, `dev_read`,
`dev_write`, `dev_ctrl`.  That layout and those names are not Phil
Burk's portable pForth; what the name stands for here is not known.

Every word's code pointer is a 68000 routine of its own -- only one pair
share one -- so none is a Forth definition, which would point at a
common routine that runs a list of other words.  No Forth source text is
stored in the image.  The interpreter is a shell over compiled code, not
a language any of the firmware is written in; `:` can still define
words at run time, and a word named `startup` exists, so whether Forth
is loaded from elsewhere at boot is not something the image can show.

A second table, around `0x2d300`, holds the firmware's own words, each
a name followed by the address of its code.  Among them:

    loop_time   pll_rep    efc_rep    pr_efc      efc_write   efc_wr
    fpll_restart          ppll_debug  lock        phase_off   hpr_pll
    dmessage    dmes_pllp  dmes_gpsm  dmes_hmon   dmes_klok   dmes_curv
    dmes_scpi   dmes_spoo  dmes_root  dmes_all
    adc_read    pr_adc_avg hdac_write hdac_all    doven
    gps_query   gps_query_all         pr_time_raim            pr_satview
    pr_hold_cause         wr_eeprom   clear_nv    master_reset  crash

### Getting to it

`:SYSTem:LANGuage` is the way in.  097-59551-02 4-15 documents two
values, `INSTALL` and `PRIMARY`; the handler, `FUN_0003fe8a`, accepts a
third.  It upper-cases its argument and compares it with `PFORTH`
(`0x4003f`) and then `INSTALL`; a match records which at `0x103326` --
0 for `PFORTH`, 1 for `INSTALL` -- and returns `0xffffffc4`, which ends
the SCPI parser's loop.  Anything else goes to `FUN_0003b7d8`.

The comparison is guarded by a check of which port the command came
from, `FUN_00039480`: only port 1 may change the language.  That is the
rule 097-59551-02 states for the 59551A, whose front-panel PORT 2 is a
second SCPI port that "cannot be used to upgrade the Receiver
firmware".  The `*IDN?` handler keeps a reply buffer per port for the
same reason.  In this image `FUN_00039480` returns 1 unconditionally,
so the Z3816A accepts the command on the one port it has.

`FUN_000395fc`, the SCPI task, runs the parser on its port and, when the
parser returns, reads `0x103326`: for `PFORTH` it calls `0x2fe06` with
the code at `0x230ba`; for `INSTALL` it calls `0x23036`; then it ends
itself through `0x271ac`.  The choice is held only in RAM -- nothing
else writes `0x103326` and nothing reads it at startup -- so a power
cycle starts the SCPI task again.

`0x2fe06` runs the function it is given at once when the byte at
`0x10385e` is clear, and otherwise queues it to `0x10385a` for another
task to run.  `0x230ba` is the console's start: it initialises the
interpreter (`0x2af04`), defines `ps`, `mem_rep` and `s_rep`
(`0x232f6`), registers the diagnostic words (`0x2c22e`), evaluates the
phrase `0 !iodev` (`0x28f98`) and prints `pForth $Revision: 1.2 $`
(`0x28fa2`), then spawns the interpreter (`0x2b038`).  `!iodev`
(`0x2a454`) closes the current device and opens the new one through
`trap #4`, the pSOS device supervisor at `0x24dae`, whose device number
is D0 with the major number in its high byte and whose function code
is D7, 0 to 5 -- `emit` writes with code 4 (`0x2932c`) and `expect`
reads with code 3 (`0x29f3a`).  So the console's port is pSOS device
0.  Which driver major number 0 selects is in the I/O switch table the
pSOS configuration at `0x102cfc` points to; that table is built at
run time and was not located in ROM.  The same `de_open(0)` is made at
`0x2f286`, and `de_open(1)` at `0x2f34a`.

The word list (`0x2a500` to `0x2b100`, 89 kernel words, and the 69
diagnostic words from `0x2c800`; every code pointer in both tables is
an even ROM address) is a stock kernel
plus pSOS wrappers -- `spawn`, `delete`, `suspend`, `resume`,
`priority`, `send_x`, `request_x`, `signal_v`, `wait_v`, `dev_init`,
`dev_open`, `dev_close`, `dev_read`, `dev_write`, `dev_ctrl`, `!iodev`
-- and the diagnostic words.  It has no `bye`, `quit` or `exit`, and
no word that starts the SCPI task again; `FUN_00039732`, which creates
that task (named `sci`, entry `0x39706`, task id `0x103d5a`), is not in
the table.

### Which port

One.  The Z3801A manual (097-z3801-01) describes a single serial
interface, the RS-422 port on the rear I/O Port 1 connector, J3.  The
firmware drives two serial devices:

- the processor's own SCI, with the message exchanges `sciR` and `sciW`
  and the driver strings at `0xba16` and `0x2f395`;
- channel A of the 68681 DUART, `drta_send_message` and
  `drta_get_byte` (`0x2df43`), which is the GPS receiver link described
  above.

Channel B of the DUART buffers no data: its receive routine records
error bits only, its transmit register (`0x200017`) is written by
nothing but a loopback self-test that sends the string `DUART` and
checks the echo, and the remaining references reset it.  So the console, like SCPI, is on
the SCI, and nothing in the firmware runs a console on another port.
Whether channel B is wired to a header on the board is not something
the image can show.

The Z3801A's image, revision 3543, does it the other way.  It carries no
SCI driver: its host port is channel B of the DUART, with the exchanges
`drtR` and `drtW` (`0x460c`) and the interrupt messages
`DUARTB isr signaling ...` (`0x4686`), while `drta_get_byte` is still the
GPS link on channel A.  The Z3805A's own `:DIAGnostic:OS` listing names
both `sciR`/`sciW` and `drtR`/`drtW` (`z3801-tree.md`), so its firmware,
3543B, is neither image exactly.

### The other image

Everything above was read from the Z3816A's image, revision 4001.  The
Z3801A's, revision 3543, was checked against it; what follows was
confirmed in its bytes.

**The same.**  CPU32: the reset vector points at `0x550`, which sets
the status register and the vector base with `movec`; the code before
it, `0x400` to `0x54f`, writes channel B's transmit register beside the
string `DUART`, a loopback test.  The landmarks of every section above
are present at their own addresses: `pll_normal`'s and `startup_pll`'s
failure messages (`0x44e94`, `0x44e2e`), the fine stage's
(`0x45e4a`), the loop report (`0x44e6d`), `PFORTH`/`INSTALL`/`PRIMARY`
(`0x2f711`), the pForth banner (`0x18ad0`), `loop_time` (`0x1d172`),
`max loop time` (`0x1c0b8`), `SAWT ERR` (`0x4d57a`), and the loop's
constants 29.75, 6.25 × 10⁻¹⁰ and 5.787 × 10⁻¹⁴.  The counter's port
routine (`0x4076a`) is instruction for instruction the Z3816A's
(`0x440a6`), and so is the routine that joins two of its four-bit
registers into a byte (`0x40ba6`, against `0x444e2`).  The interval
query's handler (`0x2efb8`) copies a float in seconds from `0x102530`
with the same `0xfff6` tag, and `pll_normal` stores the float mean
there.  The unpacker takes the same opcodes.

**Different.**

- *GPS messages.*  No `@@En` anywhere: of the two Time RAIM messages it
  handles only the six-channel `@@Bn`.  The message table's entries are
  56 bytes apart, where the Z3816A's are 60; `@@Bn`'s, at `0x5074c`,
  gives length 59, decoder `0x4f81a` and destination `0x100f63`, and
  its unpacker program at `0x50fa4` is

      02 00 03 00 85 03 04 00 0a 00 fa 01 00 04 81 80

  The Time RAIM page reads the sawtooth as `move.b (0x1a,A3)` at
  `0x486ec` with A3 = `0x100f62`: record offset 26, as in the other
  image.
- *Chip selects.*  Its reset code, at `0x550`, programs the same SIM
  the same way but for the map: CSBARBT and CSBAR6 `0x0005`, 256 KB
  at 0, and CSBAR1 and CSBAR7 `0x0405`, 256 KB at `0x40000` -- the
  image's two halves; CSBAR0, 2, 3 `0x1003`, the 64 KB of RAM; CSBAR5
  `0x3003`, 64 KB at `0x300000`; CSBAR8 `0x2000`, the DUART; CSBAR9
  `0x4001`, 8 KB at `0x400000`; CSBAR4 `0x5000` and CSBAR10 `0xfff8`,
  2 KB each.  SYNCR is `0xcf00`, the same 16.777 MHz.  Nothing in it
  reads `0x302000`.
- *Serial ports.*  The host port is DUART channel B, with the exchanges
  `drtR` and `drtW` and `DUARTB isr` messages; the SCI is switched off
  -- the one reference to its registers, at `0x120da`, clears SCCR1.
- *The language command.*  Its handler (`0x2f57a`) compares the port the
  command came from with the descriptor that `0x28a46` returns,
  `0x28cdc`, before it looks at `PFORTH`: a real check, where the
  Z3816A's always passes.  `0x28cdc` is the parser's one descriptor --
  two function pointers and the prompts `E%+04d> ` and `scpi > ` -- and
  its reader `FUN_00028a92` fills the buffer at `0x102b20` from stream
  0 (`FUN_0001dbc4`, `FUN_0001711e`).  The image has no second
  descriptor: the other copy of those prompts, at `0x7ee6`, is
  referenced by nothing.
- *The loop.*  The term the Z3816A takes from `FUN_000324b0(6)` comes
  from `FUN_00022fd2(3)` (`0x4496e`).  A field the Z3816A initialises
  to 10⁻⁷ is 10⁻⁸ here (`0x44a3a`).  With no valid reading in ten
  seconds, `pll_normal` sets the mean to 0 and still runs the update
  (`0x44bb4` falls through to `0x44bd2`), where the Z3816A skips it.
  Its G is the fixed +6.25 × 10⁻¹³ noted above, and `pll_debug`
  (`0x1b4d8`) stores to `0x102539`.

## Restarting

The image has one way to restart the processor from software: `trap
#12`, whose handler at `0x24b28` (vector 44 of the ROM table at
`0x20000`) masks interrupts, runs `FUN_00022172` -- which turns off
the GPT and QSM interrupt sources at `0xfff920` and `0xfffc1a` to
`0xfffc1f`, clears SCCR1 so the SCI stops, and calls `FUN_0002dc72` --
then executes the `RESET` instruction, reloads the VBR, and jumps
through the ROM reset vector to the reset code at `0x2466e`.  RSR then
shows SYS and nothing else, which is the reset-status value
`FUN_00022c7c` asks for (see "τ and G").

Two actions reach it, and the SCPI handlers that name them, through
descriptor lists at `0x44c9a` to `0x44cb2` of six-byte entries (a word
and a function), are:

| Command | Handler | Action | What it does first |
| ------- | ------- | ------ | ------------------ |
| `:SYSTem:PON` | `FUN_0003ff52`, list at `0x44ca0` | `FUN_000496f8` | zeroes the whole region `0x100000` to `0x100c3b` that a warm restart would restore -- the loop state, the health records, the τ block -- so the restart loads the defaults, as a power-up does |
| `:SYSTem:PRESet` | `FUN_0003ff78`, list at `0x44ca6` | `FUN_000496ca` | writes two settings records to the EEPROM at `0x4000c0` (`FUN_0004206a`, twice through `FUN_0004203e` and `FUN_000408fe`), clears the flag word at `0x100002` and recomputes the region's checksum into `0x100000` |

The `:GPS:POSition` handler `FUN_0003c268` passes the list starting at
`0x44c9a`, six bytes before `:SYSTem:PON`'s; what the parser does with
a list and where it stops were not traced.  `PON` is a keyword only
the Z3816A image has (`0x5a280`); the Z3801A's image has the same
`:SYSTem:PRESet` action, `FUN_00045cca` (`0x2f66a` passes its list at
`0x41392`), and no `PON`.  Owners report that `:SYSTem:PON` is accepted
by newer firmware and refused by older, which matches.

`*TST?` is reported by owners to restart the receiver as well, with a
minute of GPS reacquisition after it.  Its node in the common-command
table (`0x5cb36`) carries the integer reply formatter `FUN_00035a60`
and no handler of its own, and no third path to `trap #12` exists, so
how it restarts was not traced.

`:SYSTem:LANGuage "INSTALL"` takes a different exit: `FUN_00023036`
executes `trap #11`, whose handler at `0x24b18` masks interrupts, runs
the same `FUN_00022172`, and jumps through vector 43 of the table at
address 0 -- the boot ROM's own table, not the one at `0x20000` --
which is how the installer in the low half of the image is entered.

None of these is a command this project sends: `:SYSTem:PRESet` and
`:SYSTem:LANGuage` are on its never-send list, and `:SYSTem:PON` is not
in its command table.

### `:DIAGnostic:GPSystem:UTC`

The node's handler `FUN_0003a878` passes the record at `0x4322c`,
whose subject is the byte at `0x102616`, with the same getter and
setter as the τ-block bytes.  The GPS task reads that byte to choose
between two paths in `FUN_0002075a` and `FUN_00049dfc`, requires it in
`FUN_0002130c` when it parses a message, and folds it into bit 0x40 of
a status byte in `FUN_00023bbc`.  Which Oncore message each path sends
was not traced; the command table already carries the query form as
`:DIAGnostic:GPSystem:UTC?`, discovered on a 58503A, and the set form
with 0 or 1 is what owners describe.

## What is not established

`hardware-investigations.md` lists what a bench would settle of the
following, and how.

- Which SCPI keywords the handlers that return τ-block bytes hang
  from, other than `REC` for +6; what +3, +4, +5, +7 and +8 mean; and
  what the rest of the ROM defaults, `0x400de` to `0x40173`, hold.
- What HQ (`FUN_00045f94`) measures, and what the fit's mode is used
  for beyond the debug print; `FUN_00045a78`, which sets c from three
  trials, was not transcribed.
- The console's `current drift = %.1e / day` (`0x2c3bf`) prints the
  float at `0x102bcc` times 5.4 × 10⁻⁸.  Nothing that writes `0x102bcc`
  was found.
- What the Z3801A's `Oven` and `Secondary oven voltage` channels
  measure, beyond the ADC inputs and coefficients above.
- The units of the oscillator current: nominal 250 and limit 650 after
  a scale of 4.489 per ADC count.
- That `0xfff907` bit 5 reaches P2/8 and that the ADC is an ADC0838
  are an owner's report of a Z3801A board, not something traced here;
  and how the Z3801A firmware's "Oven" and "Secondary oven voltage"
  values relate to the volts at P2/9 was not worked out.
- Which CPU32 part this is: the register map matches the MC68331
  manual's and the `0xfff900` block rules out the 68332, but the part
  name comes from an owner's description of the board, not from the
  image.
- What drives bit 8 of the read-only port at `0x302000`, which
  chooses between the Z3816A's two values of G.  The image reads it
  once and looks at nothing else in that block.
- Which driver pSOS device 0, the console's, selects: the I/O switch
  table was not found in ROM.  Whether any sequence of the console's
  pSOS words gets SCPI back short of a power cycle was not tried.
  Nothing was sent to a receiver to find out; setting the language is
  one of the commands this project never sends.
- That the SCI is the port wired to J3: the firmware's SCPI port is the
  one it creates `sciR` and `sciW` for, but the board was not traced.
- What drives the 59551A's PORT 2.  DUART channel B, idle here, is the
  device a single-port unit would leave spare, but no image of a
  59551A's firmware is at hand.
- Only byte loads of offset 26 of the form `move.b (0x1a,An),Dn` were
  searched for.  A reader using another addressing form would have
  been missed.
- None of this has been checked against a 58503A image.

## Note on the disassembly

Ghidra resolves the table address of this compiler's switch idiom,
`move.w (d8,PC,Dn*2),Dn` followed by `jmp (d8,PC,Dn)`, twelve bytes
late.  The table starts at the `jmp`'s base, and offsets are taken
from there.  Case bodies reached only through such a table are left
undisassembled until disassembly is forced at the resolved targets.
