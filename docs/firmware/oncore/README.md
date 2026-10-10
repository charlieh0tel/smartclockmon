# Oncore message tables

`smartclock-cli dump-oncore` reads, out of an image for an Oncore
engine, the GPS task's message table, its request scripts and its
polling lists.  `<image>.txt` holds one image's, `models.md` the
messages of all six side by side; `make docs` regenerates them and a
test fails if they and the images disagree.  The Z3815A's engine is a
Furuno on NMEA sentences (`../gps.md`, "In every image"); its image has
no such table and is refused.  The addresses below are the 58503B
1.01.04's.

## Requests

The GPS task takes requests from its queue as an index, a mode and a
pointer to arguments.  `FUN_00052d14` sorts the index: below `0x40` a
message, `0x40` to `0x5a` a step, `0x5b` to `0x6e` a script.  The
other Oncore images have the same three ranges at other bounds, which
`<image>.txt` gives.

## The message table

`FUN_000584aa` returns entry *i* as `0x58ae0 + 60 * i`.  The table is
found from the strings `Ab` and `Ac`, each with a NUL before and after,
and the two entries that point at them: their distance is the entry
size, 60 bytes, or 56 in the Z3801A and Z3805A, whose entries end
before the send filter.  An entry holds:

| Offset | Size | What |
| ------ | ---- | ---- |
| `+0x00` | 4 | pointer to the two-letter ID, zero for an empty slot |
| `+0x04` | 1 | kind |
| `+0x06` | 16 | up to four argument encoders |
| `+0x16` | 4 | pointer to the query form, zero if none |
| `+0x1c` | 2 | length of the message the engine sends, `@@` to line end |
| `+0x20` | 24 | up to three pairs: a decoder and the record it fills |
| `+0x38` | 4 | send filter, zero if none (60-byte entries only) |

The sender, `FUN_000584c0`, composes only kinds 1 and 3; what bit 1
means was not traced.  An encoder takes its argument as a long, four
bytes in the argument block, and sends its low one, two or four bytes
(read in the Z3801A, `0x4ee4a` to `0x4eef6`).  A table may hold an ID
twice: the 58503B's entries `0x3a` to `0x3e` repeat `Ea`, `Ca` and
`Fa` without decoders, and `Ar`, each with another filter.  The table
runs to the first step index.

## Steps and scripts

`FUN_00052d5e` returns the script for an index through a jump table
whose every target loads it with `movea.l #script,a1`; the steps go
through a second jump table of the same shape.  Both start with
`suba.l #first,a0` and `cmpa.l #count-1,a0`, which give their bounds.
The steps' table covers one index fewer than the range before the
scripts.

A script is a zero-terminated list of pointers to records:

| Offset | Size | What |
| ------ | ---- | ---- |
| `+0` | 2 | a message's index, or a step's |
| `+2` | 1 | mode |
| `+4` | | the arguments, as longs |

Mode 0 sets the message from the record's own arguments, 1 queries it
with its query form, 2 sets it from the request's arguments, which the
task copies to `0x102536` (`0x53142` to `0x53160`).  `<image>.txt`
writes a record as its ID or `step-0x..`, a slash and the mode, and a
mode-0 record of a message with its arguments in parentheses, one
signed long for each of the message's encoders: encoder *k* sends
argument *k* (58503B `0x56cb0`, called with the encoder's slot).  A
step's arguments are not read.

## Polling lists

A polling list has a script's shape and is walked a record at a time,
round-robin: `FUN_00052542` takes the next record whose send filter
passes.  The 58503B walks `0x59f90` from `0x52214` and `0x59ff4` from
`0x52522`.  The lists are found by shape: a list of at least two
records, loaded as an immediate (`pea`, `move.l`, `movea.l` or `lea`),
that is not a script.

## What this does not see

Code also posts requests by index through the task's queue;
`../gps.md`, "Requests", names those found.
