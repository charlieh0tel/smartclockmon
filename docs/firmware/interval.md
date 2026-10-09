# The interval

## What the query returns

Three nodes in the command tree are named `TINTerval`.  Their parents:

| Node | Path | Query handler |
| ---- | ---- | ------------- |
| `0x5f854` | `:SOURce:SYNChronization:TINTerval` | `FUN_0003f8fa` |
| `0x60dbc` | `:SOURce:PTIMe:TINTerval` | `FUN_0003f8fa` |
| `0x63fb2` | `:DIAGnostic:PTIMe:TINTerval` | `FUN_0003b052` |

(Parents are resolved as in `README.md`, "Note on the disassembly".)  `:SOURce`
is optional, so the first is `:SYNChronization:TINTerval?` and the
second `:PTIMe:TINTerval?`; they share a handler.  A bench 58503A polled once a second for 187 s
answered `:PTIM:TINT?` with its `:SYNC:TINT?` value every time, held
for ten polls like it.  The handler copies the 32-bit value at
`0x102c0c` into its reply, tagged with the halfword `0xfff6` (−10),
when the flag at `0x102c10` is set.  The value is a single-precision
float in seconds: `pll_normal` stores there the float sum of the
readings divided by their count, converted to float (below).  The
receiver prints it to 10⁻¹⁰ s: seven readings -- one from a recorded
transcript, six from a 58503A through the daemon -- are all whole
tenths of a nanosecond, as a −10 resolution would give.  How the reply
formatter uses the tag was not traced.

The `PTIMe` node's handler, `FUN_0003b052`, answers from elsewhere:
under the lock `FUN_00023488` takes, it converts the double at
`0x102666` -- the latest one-second reading, see "One reading" below
-- to a float for the reply, tagged with the halfword at `0x102670`,
when the byte at `0x10266e` is set, and otherwise fails with code
0xc.  So `:DIAGnostic:PTIMe:TINTerval?` is one reading, and
`:SYNChronization:TINTerval?` and `:PTIMe:TINTerval?` the mean of
ten.  A Z3805A answered the `DIAGnostic` form in the keyword sweep
(`scpi/undocumented.md`, "`:DIAGnostic:PTIMe:TINTerval`").  The bench 58503A
answered it once a second for an hour on 2026-09-25, and its
ten-second value was the mean of those readings to 0.15 ns rms
(`../hardware-investigations.md`, item 8).

## The ten-second average

`FUN_0004824a` writes both.  Each second it takes a reading and, if
valid, adds it to a running sum with a count at `0x102735`.  A
counter at `0x102734` runs to ten; at ten, the sum is divided by the
count, the flag is set and the mean is stored at `0x102c0c`.

On a 58503A, 397 of 400 consecutive distinct values were each held for
exactly ten one-second polls: this schedule seen from outside.

## One reading

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

`0x1023d0` is a stored constant, not a live value: `FUN_0004045c`
loads it, with `0x1023cc` and `0x1023d4`, from a checksummed 64-byte
block at `0x400080`, and `FUN_000404be` writes them back.

On failure the reading is replaced by a sentinel double and one of four
error codes is logged.

## The FPGA

The counter is read through a port at `0x304000`.  `FUN_000440a6`
writes a register index into a control byte, shadowed at `0x102673`,
and reads back four bits; `FUN_000444e2` reads two nibbles for a byte;
the count is assembled from three bytes, register selectors `0x16`,
`0x14` and `0x12`.

## In every image

The same code is in all seven images, at other addresses: the
`SYNChronization` and `PTIMe` forms share a query handler that returns
the float mean of ten one-second readings, tagged `0xfff6`, while its
flag is set; the `DIAGnostic` form has its own handler, which returns
the latest reading, a double converted to float, with the tag stored
beside it; one routine counts readings to ten and stores the mean.
Each image also holds the 1.0e-7 tick constant (`0x3e7ad7f29abcaf48`)
and the FPGA port `0x304000` and calibration block `0x400080`
addresses.  Read with Ghidra 12.1.3 on 2026-10-09; the mean and its
flag sit four bytes apart in each.

| Image | `:SYNC:TINT?` handler | Mean, flag | Averaging | `:DIAG:PTIM:TINT?` handler | Reading, flag |
| ----- | --------------------- | ---------- | --------- | -------------------------- | ------------- |
| Z3816A 4001 | `0x3f8fa` | `0x102c0c`, `0x102c10` | `0x4824a` | `0x3b052` | `0x102666`, `0x10266e` |
| Z3801A 3543 | `0x2efb8` | `0x102530`, `0x102534` | `0x44928` | `0x2a75a` | `0x101f94`, `0x101f9c` |
| Z3805A 3543B | `0x300b2` | `0x102538`, `0x10253c` | `0x448fc` | `0x2b896` | `0x101f9c`, `0x101fa4` |
| 58503A 3633 | `0x2fbf6` | `0x10284e`, `0x102852` | `0x4491e` | `0x2b344` | `0x1022a8`, `0x1022b0` |
| 58503A 3704 | `0x2fe82` | `0x10285e`, `0x102862` | `0x4495a` | `0x2b3ec` | `0x1022ae`, `0x1022b6` |
| Z3815A 4010 | `0x44112` | `0x102c5c`, `0x102c60` | `0x4ddea` | `0x3ec0a` | `0x10268c`, `0x102694` |
| 58503B 1.01.04 | `0x3ff46` | `0x1032b0`, `0x1032b4` | `0x498ee` | `0x3ae28` | `0x1029ee`, `0x1029f6` |

Two differences:

- **The 58503B counts to a stored limit.**  Its averaging routine ends
  the run when the count reaches the byte at `0x1032ae` rather than
  ten; the one routine that writes that byte, an initializer
  (`0x4c6de`), sets it to 10.  So the mean is still of ten readings.
- **Fine acquisition writes the mean too in 3704 and the 58503B.**
  The state machine that prints `i= %d, ti= %.1f, ...` (`loop.md`,
  "Fine acquisition") stores into the mean and sets its flag in those
  images (`0x454d4`, `0x4a356`).  What it stores is for the loop to
  say.  The Z3815A's mean has four more writers, not yet identified;
  in the other images the reference search found only the averaging
  routine, and in the Z3816A a further writer of the flag (`0x4b088`)
  that did not decompile.
