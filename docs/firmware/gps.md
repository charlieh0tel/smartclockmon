# The GPS receiver link

The Oncore is on channel A of a 68681 DUART at `0x200000`, whose
sixteen registers sit on odd byte addresses (`0x200001` + 2*n).

| Function | Role |
| -------- | ---- |
| `FUN_0002dbdc` | DUART interrupt handler; reads the interrupt status at `0x20000b` and dispatches |
| `FUN_0002dab2` | channel A receive; copies bytes from `0x200007` into a 256-byte ring at `0x103595` (write index `0x103593`, read index `0x103594`) and posts event `0x4000` |
| `FUN_0002d6dc` | takes one byte from the ring |
| `FUN_00050314` | the message framer |

Channel B's receive routine (`FUN_0002db42`) records error bits and
buffers no data; `FUN_0002dc9a` is a channel B loopback self-test that
sends `DUART` and checks the echo.

## Framing

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

## The message table

`FUN_00050286` looks an ID up in a table of 64 descriptors of 60 bytes
at `0x56732` (`FUN_000560fc` returns descriptor *i* as
`0x56732 + 60 * i`).  Each holds a pointer to its two-character ID at
+0, its length at +28, a decoder at +0x20 and a post-handler at +0x38.
The two Time RAIM messages have the lengths the VP Oncore reference
gives:

| Entry | ID | Length | Decoder | Post-handler |
| ----- | -- | ------ | ------- | ------------ |
| 41 | `@@Bn`, six channels | 59 | `0x55bb0` | `0x5605a` |
| 53 | `@@En`, eight channels | 69 | `0x55c40` | `0x5605a` |

`FUN_00055f68`, reached from the post-handler, accepts `Bn` for a
six-channel receiver and `En` for an eight-channel one, by the channel
count at `0x100ebc + 0x606`.

## Decoding

Both decoders unpack through `FUN_00054ec8`, driven by a byte program
rather than code, so no instruction reads a field of the raw message
directly.  The opcodes:

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

Against the message layout in `VPCommands.pdf` (message byte 0 being
the first `@`), its fifth copy moves message bytes 16 to 25 -- hours,
minutes, seconds, pulse status, 1 PPS sync, Time RAIM solution status,
Time RAIM status, the two-byte one-sigma estimate, and the negative
sawtooth -- to record offsets 17 to 26.  The `Bn` program at `0x57e25`
differs only in its channel count.  The sawtooth is at offset 26 of
the decoded record.

## The engines on the bench

`:DIAGnostic:IDENtification:GPSystem?` returns the engine's identity,
a list of quoted `LABEL value` strings.  Read on 2026-09-26:

| | 58503A 3710A01056 | Z3801A 3542A01548 | Z3805A 3625A01487 |
| - | ----------------- | ----------------- | ----------------- |
| Copyright | Motorola 1991-1996 | Motorola 1991-1995 | Motorola 1991-1995 |
| Model | B4121P1115 | B1121P1114 | B1121P1114 |
| Software P/N | 98-P36830P | 98-P39972M | 98-P39972M |
| Version, revision | 8, 8 | 8, 4 | 8, 4 |
| Software date | 06 Aug 1996 | 13 Jul 1995 | 13 Jul 1995 |
| Serial | SSG0220999 | SSG0178541 | SSG0163878 |
| Manufactured | 7D01 | 6J25 | 6G09 |
| Options | IB | IB | IB |

The daemon recorded the Z3805A's on 2026-09-26 when it reattached.
The two Z380x engines are the same model with the same software; the
58503A's is a different model with later software.  From schema 9 the
daemon reads it once per connection, keeps it as answered in the
receiver table of each unit's log, and notes in the journal when it
is first seen or changes.

The Z3801A's engine answered while tracking no satellites on an
antenna the 58503A tracks on.

## Six or eight channels

Which engines each image can take, by the two message pairs the VP
Oncore reference gives in six- and eight-channel forms: position/status
`@@Ba` / `@@Ea` and Time RAIM `@@Bn` / `@@En`.  A message counts as
handled when the image holds a pointer to its ID:

| Image | `Ba` | `Ea` | `Bn` | `En` |
| ----- | ---- | ---- | ---- | ---- |
| Z3801A 3543 | yes | no | yes | no |
| Z3805A 3543B | yes | no | yes | no |
| 58503A 3633 | yes | yes | yes | yes |
| 58503A 3704 | yes | yes | yes | yes |
| Z3816A 4001 | yes | yes | yes | yes |

The 58503A images decide at run time.  In 3704:

- The message table (60-byte entries, `Bn` at `0x51b14`) has `Ea` at
  entries 50 and 54, decoders `0x506e2` and `0x5098a`.  Both set bit 5
  of the byte at `0x1013e2` and call `FUN_000509fe`, which stores 8 at
  `0x1013e0` when that bit is set and 6 when it is clear.  No other
  code sets or clears the bit by address.
- The descriptor's last long, at +0x38, is a send filter.  The command
  builder `FUN_00050bc8` calls it with the entry number and the
  command's first argument and sends nothing when it returns zero.
  `FUN_00050ad8`, the filter of `Ba`, `Ea` and `Ek` and, through
  `0x50b48`, of `Bn` and `En`, passes `Ba`, `Bk` and `Bn` (entries 30,
  39, 41) when the count is 6 and `Ea`, `Ek` and `En` (50 to 52) only
  when it is 8.  With the count at 8 it still passes the six-channel
  three when their first argument, the output rate, is 0: the
  reference's "output response message once".  So an eight-channel
  engine is polled with the six-channel messages but given continuous
  output only in the eight-channel ones.  Entry 54, the second `Ea`,
  has no filter and no destination.
- `Ca` and `Fa`, the six- and eight-channel self-tests, are filtered
  on bit 5 alone (`0x50ab8`, `0x50ac8`).
- Bits 6 and 7 of `0x1013e2` come from the engine's `@@Cj` identity,
  in `FUN_00050a16`, which addresses the byte as `0x592` off
  `0x100e50`.  Once a `Cj` has been received (`0x101258`, set by its
  decoder), bit 6 is set when the `OPTIONS LIST` value contains an
  `I`, and bit 7 when `SOFTWARE VER` × 10 + `SOFTWARE REV` is 84:
  software 8.4.  Run before a `Cj` arrives, it clears both.  `Bn` and
  `En` go out only with bit 6 set (`0x50b48`); entry 55, a second
  `@@Ar` (position fix algorithm), only with bit 7 (`0x50b6c`).  The
  field labels are matched from the table at `0x5262c`.

So the count is 6 until an `@@Ea` arrives from the engine and 8 after.
The three bench engines all report options `IB`, so each would get
Time RAIM commands; the two Z380x engines report software 8.4 and the
58503A's 8.8.  3633 has the same selector at `0x50802`, the same two
`bset` sites at `0x5051c` and `0x5078e`, and the same identity tests
at `0x50834` and `0x5088e`.  Neither Z380x image has an `Ea` or `En`
entry or these tests.  None of this has been run with an engine other
than the one each unit came with.

## Where the sawtooth is read

The one reader of offset 26 found is `FUN_0004c062`, at `0x4c2b4`,
which sign-extends it and prints it on a Time RAIM status page whose
format strings are:

    ALARM LIM  %-24s   TIME REF  %s
    SIGMA EST  %-24s   SAWT ERR  %+d ns

`SIGMA EST` is read from record offset 24 and `SAWT ERR` from offset
26, matching the layout above.  The page is reached through a pointer
table; the command that shows it has not been identified.

## The 58503B's height datum

The `@@Ba` and `@@Ea` descriptors (`0x591e8`, `0x596d4`) unpack into
`0x1012b4` through the byte programs at `0x5a1b0` and `0x5a1ba`
(`04 00 1d 00 ...`): month, day and year, a skipped byte, then 29
message bytes from the hour on.  Against the layout in
`VPCommands.pdf`, the GPS-ellipsoid height `hhhh` lands at `0x1012c8`
and the MSL height `mmmm` at `0x1012cc`.

Bit 4 of `0x1017e8` (`0x608` off `0x1011e0`) chooses between them:

| Clear | Set | Where |
| ----- | --- | ----- |
| averages `0x1012c8` into `0x101762` and copies it to `0x10176a` | averages `0x1012cc` into `0x10176a` | survey, `FUN_00057942` |
| zeroes the last byte, the height type (0 = GPS ellipsoid), of `@@Af`, `@@As` and `@@Au` (commands 5, 18 and 20) | leaves it as encoded | sender, `FUN_000584c0` |
| `%+9.2f m  (GPS)` | `%+9.2f m  (MSL)` | status screen, `0x51160` and `FUN_0005011a` |
| `GPS` | `MSL` | `Position hold started.  [%s Hgt = %+8d cm]`, `0x41bc8` |

The held position comes from the survey average, `0x101762`
(`FUN_0004ba9e`, `0x4bea2`), into `0x1028c0` and `0x1028fc`, which
`:GPS:POSition?` (`FUN_0003cbae`) returns.  Two diagnostic reports
print both heights regardless: `GPS HGT` and `MSL HGT` (`0x4e8fe`),
`HGT (msl)` and `HGT (gps)` (`0x4f276`).

No code found sets the bit.  The only writes to it are clears
(`0x56f5a`, `0x56f86`, `0x56fb6`), one on each engine-type path of
`FUN_00058098`.  A scan of every disassembled store whose operand can
reach `0x1017e8` and a byte search for set, OR and move encodings by
absolute address or displacement from `0x1011e0` found no other, and
cold start zeroes the byte.  So 1.01.04 surveys, holds, reports and
commands the ellipsoid height, as 097-58503-13 (3-17, 4-5) says.  A
write through a computed pointer would escape this search.

## In every image

The Z3801A, Z3805A, both 58503As, the Z3816A and the 58503B talk to an
Oncore in its binary `@@` messages; the 58503B names the engine
`Oncore`, as the Z3816A does.  The Z3815A's engine is a Furuno GT-74
(its identification template reads `MODEL # FURUNO GT-74`), on NMEA
sentences: it takes `GGA`, `GSA`, `GSV`, `RMC` and Furuno's own `anc`,
`ssd`, `tst`, `tps`, `rrm` and `rsd` (`0x5e398`), and sends `$PFEC,GP`
commands -- `set`, `srq`, `rrs`, `rrq`, `clr`, `ZDA`, `GLL`, and
`int,` with a sentence name to set its interval (`0x5e64c` to
`0x5e6ca`).  It keeps a `GT-74 Command Log` and a `GT-74 Messages`
screen.  Read on 2026-10-10.

**Time RAIM.**  The 58503B carries the Z3816A's `@@En` program, byte
for byte, at `0x5a1e7`.  The Z3815A, with no `@@` messages, fills a
record of the same layout from its engine's sentences: its Time RAIM
page reads the sawtooth as `move.b (0x1a,A3)` at `0x53df0`, offset 26
as in the others, and prints it on the same `SAWT ERR  %+d ns` line;
it has no `SIGMA EST` line.

**Where the sawtooth goes.**  A search of each image for byte reads at
offset `0x1a` from an address register finds, in the Z3816A, the
58503B and the Z3815A, one read of the decoded record each -- the Time
RAIM page (`0x4c2b4`, `0x4e5da`, `0x53df0`) -- and no other: the rest
read offset `0x1a` of a message-table descriptor (`0x504ae`,
`0x527ec`, `0x575c6`), or, in the Z3815A, the loop's state byte, in the
console's state printer at `0x2c3bc`, whose names include a state the
others lack, `n3 lock pll`.  So in those three, as in the Z3816A, the
sawtooth is displayed and not applied to the interval.  A read in
another form -- a word, or through an index -- would escape this
search.
