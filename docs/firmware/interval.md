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
(`z3801-tree.md`, "time interval, unlocked reading").  The bench 58503A
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
