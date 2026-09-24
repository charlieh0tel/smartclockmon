# What the firmware shows

What `third_party/z3816a.bin` shows about how the receiver measures the
1 PPS time interval, how it disciplines the oscillator from it, and
whether the Oncore's sawtooth correction enters either.  Every address
below is in that image, which is loaded at address zero.

The processor is a CPU32 part, a 68331 or 68332: the reset code at
`0x400` sets the vector base with `movec` and programs the System
Integration Module's registers at `0xfffa00`.

This is the Z3816A's firmware.  No 58503A image is available, so what
follows describes the design family and is not a statement about the
58503A's own code.

## Summary

- The reported interval is **the mean of ten one-second readings**,
  stored in tenths of a nanosecond.  It changes once every ten seconds.
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
- The firmware carries a **pForth interpreter** whose words include
  the loop's own diagnostics.

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
Its handler returns the 32-bit value at `0x102c0c` with a decimal
exponent of -10 -- tenths of a nanosecond -- when the flag at
`0x102c10` is set.  The `PTIMe` node's handler was not traced.

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
| s | -- | what `FUN_000324b0(6)` returns, read once per call |
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

### τ and G

`FUN_0002b358` sets τ from its argument.  Nothing calls it directly; a
pointer to it sits in the console's word table at `0x2d368`, beside the
name `loop_time`, and the message `max loop time = %d` (`0x2c370`)
prints τ.

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

## The debug console

The firmware contains pForth, a portable Forth interpreter:
`pForth $Revision: 1.2 $` at `0x28fa4`, the core words (`loop`,
`+loop`, `again`, `interpret`) and a table of the firmware's own words
around `0x2d300`, each a name followed by the address of its code.
Among them:

    loop_time   pll_rep    efc_rep    pr_efc      efc_write   efc_wr
    fpll_restart          ppll_debug  lock        phase_off   hpr_pll
    dmessage    dmes_pllp  dmes_gpsm  dmes_hmon   dmes_klok   dmes_curv
    dmes_scpi   dmes_spoo  dmes_root  dmes_all
    adc_read    pr_adc_avg hdac_write hdac_all    doven
    gps_query   gps_query_all         pr_time_raim            pr_satview
    pr_hold_cause         wr_eeprom   clear_nv    master_reset  crash

The string `PFORTH` is stored at `0x4003f`, immediately before
`INSTALL` and `PRIMARY` -- the two values 097-59551-02 4-15 documents
for `:SYSTem:LANGuage`.

The loop's report above is printed only when the flag at `0x102c13` is
set.

## What is not established

- The loop's starting τ, how `startup pll` sets or grows it, and its
  largest value.
- Where G comes from: `FUN_0004b088`, its one caller, would not
  decompile.
- What p, q, r and the term d are, and what `FUN_000324b0(6)` and
  `FUN_00023818` read.
- What the loop's setpoint x₀ holds, and what sets it.
- That setting `:SYSTem:LANGuage` to `PFORTH` opens the console: the
  code that selects a language was not traced, and nothing was sent to
  a receiver to find out.  Setting the language is one of the commands
  this project never sends.
- Which word, if any, sets the report flag at `0x102c13`.
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
