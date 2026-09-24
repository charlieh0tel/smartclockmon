# How the time interval is measured

What `:SYNChronization:TINTerval?` reports, and whether the Oncore's
sawtooth correction enters it, as read out of `third_party/z3816a.bin`.
Motorola 68000 code, loaded at address zero; every address below is in
that image.

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

## What is not established

- The disciplining loop was not traced.  It could use the sawtooth
  separately from the reported interval; nothing here shows either way.
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
