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
and the 68332 does not -- the 68332 has a TPU there instead.  The 68334,
68336 and 68376 also carry a GPT; which of these parts this is, the
image does not show.

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
  as 1/(Gτ) and the integral as 1/(4Gτ²).  Its phase input is the
  interval mean; no read of the sawtooth was found in it.
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
uses the tag that way was not traced.  The `PTIMe` node's handler was not traced.

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
`fine fine slew` -- then `startup pll`, then `normal pll`.  The
Z3805A's `:DIAGnostic:OS:PROCess?` lists a pSOS task `pllp` (see
`z3801-tree.md`); which task runs the functions below was not traced.

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
| x₀ | `0x102c1c` | the setpoint subtracted from it |
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
                                             q = what FUN_00023818 returns
    I ← I + 10·k·(f + d / (2700·k))          k = 1 / (4·G·τ²)
    u  = K·f + B + I + c·s                   K = 1 / (G·τ)

and `FUN_00033798(u)` converts the result.  On entry the integrator is
cleared and B is set to the EFC in force less c·s, so that starting the
loop does not move the oscillator.

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

Read from the Z3801A's image, where the same function is `FUN_000442ca`:
once the ten-second means have settled, it lengthens its time constant
by 5 s an update until it reaches τ, adjusting the integrator at each
step so that the EFC does not jump, and then hands over to
`pll_normal`.  The Z3816A's matching code, `0x47ede` to `0x47fce`,
holds the 5.0 at `0x47efe`; the settling condition was not confirmed
there.

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
prints τ.

G is a constant.  `FUN_0004b088` reads the hardware word at
`0x302000` and passes −1.25 × 10⁻¹² if bit 8 is set and
−2.125 × 10⁻¹² if it is clear (`0x4b14c` to `0x4b166`); the Z3801A's
image passes a fixed +6.25 × 10⁻¹³ (`0x475bc`).  What the bit or the
sign stand for was not traced.

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
| `0x64b8e` | D0 + D1 |
| `0x65df4` | D0 × D1 |
| `0x652ba` | D0 ÷ D1 |
| `0x652b8` | D1 ÷ D0: exchanges the two and falls into the divide |
| `0x234f2` | absolute value of the float on the stack |
| `0x65130` | compare D0 with D1 |
| `0x65ce2` | integer to float |
| `0x65a90` | float to integer |
| `0x659f8` | float to double, for printing |
| `0x652ae` | divide the float at A0 by D1, in place |
| `0x64b5e` | add D1 to the float at A0, in place |

The constants, as single-precision floats: `0x41ee0000` is 29.75,
`0x4528c000` 2700, `0x40800000` 4, `0x41200000` 10, `0x302bcc77`
6.25 × 10⁻¹⁰, `0x29824fff` 5.787 × 10⁻¹⁴, and `0x4e6e6b28` 10⁹, which
scales the interval to nanoseconds for the report.

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
`0x461fe`) set the same bit and a flag at `0x101b6f`.  Its decision,
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
- *Serial ports.*  The host port is DUART channel B, with the exchanges
  `drtR` and `drtW` and `DUARTB isr` messages; the SCI is switched off
  -- the one reference to its registers, at `0x120da`, clears SCCR1.
- *The language command.*  Its handler (`0x2f57a`) compares the port the
  command came from with the descriptor that `0x28a46` returns,
  `0x28cdc`, before it looks at `PFORTH`: a real check, where the
  Z3816A's always passes.  Which port `0x28cdc` describes was not
  traced.
- *The loop.*  The term the Z3816A takes from `FUN_000324b0(6)` comes
  from `FUN_00022fd2(3)` (`0x4496e`).  A field the Z3816A initialises
  to 10⁻⁷ is 10⁻⁸ here (`0x44a3a`).  With no valid reading in ten
  seconds, `pll_normal` sets the mean to 0 and still runs the update
  (`0x44bb4` falls through to `0x44bd2`), where the Z3816A skips it.
  Its G is the fixed +6.25 × 10⁻¹³ noted above, and `pll_debug`
  (`0x1b4d8`) stores to `0x102539`.

## What is not established

- τ's own value in service.  Startup clears RAM from `0x100c3c` to
  `0x110000`, which includes τ, and the only code that writes it is
  `loop_time`; no text in either image runs `loop_time`.  Its other
  references read it: `pll_normal` at `0x48260`, and `startup_pll` at
  `0x47eda` and `0x47f6c`.  Where a working value comes from was not
  found.
- The condition under which `startup_pll` starts lengthening its time
  constant: reported from the Z3801A's image as sixteen consecutive
  means within ±150 ns, but no constant of 1.5 × 10⁻⁷ is stored in
  either image as a float or a double.
- What p, q, r and the term d are.  `FUN_00023818` returns the counter
  at `0x100c08`, compared with 900 as seconds by the warmup and used as
  q; what increments it was not traced.
- What the Z3801A's `Oven` and `Secondary oven voltage` channels
  measure, beyond the ADC inputs and coefficients above.
- The units of the oscillator current: nominal 250 and limit 650 after
  a scale of 4.489 per ADC count.
- What the `0xfff900` block's bit 5 drives, beyond the firmware's
  naming of it: `doven`, and the state it is switched on in.
- Which CPU32 part this is.  The `0xfff900` block rules out the 68332;
  the 68331 is the simplest part that has it.
- x₀ is cleared at `0x4b1dc` and otherwise written only by `phase_off`;
  whether anything calls `phase_off` other than the console was not
  traced.
- What the console does on the port once started, and whether any word
  returns the port to SCPI short of a power cycle: the code at `0x230ba`
  and `0x2fe06` was not traced.  Nothing was sent to a receiver to find
  out; setting the language is one of the commands this project never
  sends.
- That the SCI is the port wired to J3: the firmware's SCPI port is the
  one it creates `sciR` and `sciW` for, but the board was not traced.
- What drives the 59551A's PORT 2.  DUART channel B, idle here, is the
  device a single-port unit would leave spare, but no image of a
  59551A's firmware is at hand.
- Only a sample of the firmware word table's code pointers was
  checked; they pointed at 68000 code.
- Only byte loads of offset 26 of the form `move.b (0x1a,An),Dn` were
  searched for.  A reader using another addressing form would have
  been missed.
- The `PTIMe` node's `TINTerval` handler, `FUN_0003b052`, was not
  traced.
- None of this has been checked against a 58503A image.

## Note on the disassembly

Ghidra resolves the table address of this compiler's switch idiom,
`move.w (d8,PC,Dn*2),Dn` followed by `jmp (d8,PC,Dn)`, twelve bytes
late.  The table starts at the `jmp`'s base, and offsets are taken
from there.  Case bodies reached only through such a table are left
undisassembled until disassembly is forced at the resolved targets.
