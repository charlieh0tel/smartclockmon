# The debug console

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

Every word's code pointer is a 68000 routine of its own -- only one
pair share one -- so none is a Forth definition, which would point at a
common routine that runs a list of other words.  No Forth source text
is stored in the image.  The interpreter is a shell over compiled
code, not a language any of the firmware is written in; `:` can still
define words at run time, and a word named `startup` exists, so the
image cannot show whether Forth is loaded from elsewhere at boot.

A second table, around `0x2d300`, holds the firmware's own words, each
a name followed by the address of its code.  Among them:

    loop_time   pll_rep    efc_rep    pr_efc      efc_write   efc_wr
    fpll_restart          ppll_debug  lock        phase_off   hpr_pll
    dmessage    dmes_pllp  dmes_gpsm  dmes_hmon   dmes_klok   dmes_curv
    dmes_scpi   dmes_spoo  dmes_root  dmes_all
    adc_read    pr_adc_avg hdac_write hdac_all    doven
    gps_query   gps_query_all         pr_time_raim            pr_satview
    pr_hold_cause         wr_eeprom   clear_nv    master_reset  crash

## Getting to it

`:SYSTem:LANGuage` is the way in.  097-59551-02 4-15 documents two
values, `INSTALL` and `PRIMARY`; the handler, `FUN_0003fe8a`, accepts a
third.  It upper-cases its argument and compares it with `PFORTH`
(`0x4003f`) and then `INSTALL`; a match records which at `0x103326` --
0 for `PFORTH`, 1 for `INSTALL` -- and returns `0xffffffc4`, which ends
the SCPI parser's loop.  Anything else goes to `FUN_0003b7d8`.

The comparison is guarded by a check of the port the command came
from, `FUN_00039480`: only port 1 may change the language.  That is
the rule 097-59551-02 states for the 59551A, whose front-panel PORT 2
is a second SCPI port that "cannot be used to upgrade the Receiver
firmware".  The `*IDN?` handler keeps a reply buffer per port for the
same reason.  In this image `FUN_00039480` returns 1 unconditionally,
so the Z3816A accepts the command on its one port.

`FUN_000395fc`, the SCPI task, runs the parser on its port and, when
the parser returns, reads `0x103326`: for `PFORTH` it calls `0x2fe06`
with the code at `0x230ba`; for `INSTALL` it calls `0x23036`; then it
ends itself through `0x271ac`.  The choice is held only in RAM --
nothing else writes `0x103326` and nothing reads it at startup -- so a
power cycle starts the SCPI task again.

`0x2fe06` runs the function it is given at once when the byte at
`0x10385e` is clear, and otherwise queues it to `0x10385a` for another
task to run.  `0x230ba` is the console's start: it initializes the
interpreter (`0x2af04`), defines `ps`, `mem_rep` and `s_rep`
(`0x232f6`), registers the diagnostic words (`0x2c22e`), evaluates the
phrase `0 !iodev` (`0x28f98`) and prints `pForth $Revision: 1.2 $`
(`0x28fa2`), then spawns the interpreter (`0x2b038`).  `!iodev`
(`0x2a454`) closes the current device and opens the new one through
`trap #4`, the pSOS device supervisor at `0x24dae`, whose device number
is D0 with the major number in its high byte and whose function code
is D7, 0 to 5 -- `emit` writes with code 4 (`0x2932c`) and `expect`
reads with code 3 (`0x29f3a`).  So the console's port is pSOS device
0.  Major number 0 is the SCI: the start-up code at `0x57fc4` copies
the pSOS configuration from `0x581bc` to `0x102cfc` and the I/O switch
table from `0x58670` to `0x102c90` (the configuration's word at +0x22,
the highest major, is 2, and its long at +0x24 is `0x102c90`); the
table is eighteen `jmp` stubs, six per major, and major 0's are the
SCI driver's -- `FUN_0002ed56`, the initialization that creates
`sciR` and `sciW`, then `0x2ecae`, `0x2ebc0`, `0x2f15c`, `0x2f10e`
and `0x2efb6` for open, close, read, write and control.  So the
console reads and writes the same port as SCPI.  The same
`de_open(0)` is made at `0x2f286`, and `de_open(1)` at `0x2f34a`.

The word list (`0x2a500` to `0x2b100`, 89 kernel words, and the 69
diagnostic words from `0x2c800`; every code pointer in both tables is
an even ROM address) is a stock kernel plus pSOS wrappers -- `spawn`,
`delete`, `suspend`, `resume`, `priority`, `send_x`, `request_x`,
`signal_v`, `wait_v`, `dev_init`, `dev_open`, `dev_close`, `dev_read`,
`dev_write`, `dev_ctrl`, `!iodev` -- and the diagnostic words.  It has
no `bye`, `quit` or `exit`; the way back to SCPI is `halt` (see
"Leaving it").

## Leaving it

`halt`, the console's own exit, is the same in every image (read from
the images, October 2026; tried on a Z3805A and a 58503A, below).  It
reads a hook cell and jumps through it, or executes `trap #14` when
the cell is zero.  The interpreter's setup clears the cell and the
console's start sets it at once, so in practice the hook runs.  The
hook raises the console task's priority to 26, has the deferred-call
routine run the SCPI task's creator at priority 25 -- the creator the
root task uses at boot, which looks the task up by name and creates
and starts it -- clears the console's task id and deletes the console
task.  The new SCPI task, below the console's 26, cannot run until the
console is gone.

| Image | `halt` | Hook cell | Hook (set at) | Creator |
| ----- | ------ | --------- | ------------- | ------- |
| Z3801A 3543 | `0x18f94` | `0x1009fe` | `0x12f50` (`0x12fc0`) | `0x28e92` |
| Z3805A 3543B | `0x1928c` | `0x1009fe` | `0x12f50` (`0x12fc0`) | `0x29fce` |
| 58503A 3633 | `0x19282` | `0x100bda` | `0x52584` (`0x1306a`) | `0x29450` |
| 58503A 3704 | `0x193f0` | `0x100bde` | `0x13124` (`0x13194`) | `0x294f8` |
| Z3816A 4001 | `0x29468` | `0x100c4e` | `0x2305a` (`0x230d0`) | `0x39732` |
| Z3815A 4010 | `0x29a2a` | `0x100cb6` | `0x23116` (`0x23192`) | `0x3cb3c` |
| 58503B 1.01.04 | `0x294f6` | `0x100f6e` | `0x23112` (`0x2318e`) | `0x38e1c` |

So `halt` returns the port to SCPI without restarting the processor,
without the installer, without an EEPROM write and without the
checksums the installer route costs, and needs no address.  It leaves
two things undone.  The console's 4 KB dictionary, allocated with pSOS
call 8 (Z3801A `0x1aa40`), is never returned -- the only callers of
call 9 are the SCPI task's own parser buffer and the C library's
`free`.  Yet two visits per unit on 2026-10-04 showed no loss in
`mem_rep` (below).  And in the first five images the console does not
close device 0 before deleting itself, whereas the SCPI task frees its
buffer and closes its stream before handing over (`0x28fac` to
`0x28fbc`).  The Z3815A's and 58503B's hooks add one call between
raising the priority and handing back, `0x309a0` and `0x2e8de`: each
passes device 0 and function code 2 to the `trap #4` device
supervisor, a close (`de_close(0)`), so those two consoles close their
device before they go.

On 2026-10-04 the bench Z3805A (3625A01487, 3543B-A, 19200 8N1) was
taken into the console and out with `halt` twice.  Each time `halt`
answered `scpi > ` at once, and the unit was in `"PRIMARY"` with an
empty error queue; `*IDN?` and ordinary queries answered as before.
`mem_rep` in the second visit matched the first: 18,790 bytes free,
and the console's two segments, 4,114 and 2,854 bytes, at the same
addresses (`0x10af2e`, `0x10a408`).  The bench 58503A (3710A01056,
3704-C, 19200 8N1), whose hook differs, did the same that day:
`scpi > ` at once both times, `"PRIMARY"`, an empty error queue, and
16,248 bytes free in both visits with the console's segments at
`0x10a71c` and `0x109bf6`.  On both units two visits leaked nothing
`mem_rep` shows.

## Reading memory through it

The console's `c@` and `@` read the running unit's memory, so its ROM
and EEPROM can be read without opening it.  Loops are compile-only
here, so one word does it, and defining it touches only the dictionary
in RAM:

    : rd ( addr n -- ) over + swap do i @ u. 4 +loop ;

In `hex`, `<addr> <n> rd` prints the 32-bit words from `addr` for `n`
bytes.  `smartclock-cli read-memory` does the rest: it sends an empty
line and, when no console prompt comes back within three seconds,
sends `:SYSTem:LANGuage "PFORTH"` and waits for one; then it defines
`rd`, reads 1 KB per request with retries, each after draining the
line and taking a fresh prompt, writes the bytes to a file and, given
`--compare`, checks each kilobyte against an image of the same length,
failing on any difference.  Then, unless given `--stay-in-console`, it
returns the port to SCPI, after a failed read as well.  It sends
`halt` and checks that `:SYSTem:LANGuage?` answers `"PRIMARY"`
("Leaving it").  If not, it falls back to the installer: it checks
that the cell and `trap #11` of exactly one known image are where that
image has them, sends `<cell> execute`, checks that the installer
answers `:SYSTem:LANGuage?` with `"INSTALL"`, sends `:SYSTem:LANGuage
"PRIMARY"`, and checks for `"PRIMARY"` again ("Forced installer entry
with an unusable primary").  Nothing is erased or programmed.  The
procedure:

1. Stop the unit's daemon, which holds the port:
   `sudo systemctl stop smartclockd@<port>`.
2. Read the ROM, 512 KB at address 0, and the EEPROM, 8 KB at
   `0x400000` (chip select 9, as the reset code programs it):

       smartclock-cli --device /dev/<port> --framing 7O1 read-flash \
           --out rom.bin --compare third_party/firmware/z3801a-3543.bin
       smartclock-cli --device /dev/<port> --framing 7O1 read-eeprom \
           --out eeprom.bin

   `read-flash` and `read-eeprom` are `read-memory` with those two
   ranges fixed.

3. Start the daemon again.  With `--stay-in-console`, first leave the
   console with `halt` by hand; if neither way out worked, power cycle
   the unit.

On 2026-09-28 the bench Z3801A (3542A01548, 3543-A) read 4 bytes this
way and came back to SCPI on 3543-A, in PRIMARY, with an empty error
queue, without a power cycle.  On 2026-10-04, after `read-memory`
changed to leave with `halt`, `read-eeprom` on the bench Z3805A and
58503A each came back through `halt`, in PRIMARY with an empty error
queue, without the installer fallback.  The installer route restarts
the primary, and the restart writes the EEPROM: on 2026-09-28, between
two `read-eeprom` runs that returned that way, one 44-byte record was
added at `0x1140`, after the last one there.

On the bench Z3801A (3542A01548, 3543-A), on 2026-09-26, the ROM came
back in 689 seconds and is byte-identical to `z3801a-3543.bin`:
SHA-256 `29e33b6d85b7371cef16cbf68b071f8a4ca047a8ab391d3c1e8087e553199bed`
for both.  So the image is this unit's firmware, not only its
revision.  Its EEPROM took 6 seconds.  It begins with the model
`Z3801A`, the serial `3542A01548` and `AQ`; holds two records at
`0x0c0` and `0x140`, each starting with the revision `3543` -- the
two settings records `:SYSTem:PRESet` writes at `0x4000c0` in the
Z3816A image; and from `0x1c0` holds the diagnostic log, starting
with `Log cleared`.  The rest of its layout is not worked out.  The
first session is `docs/z3801a-pforth.txt`.

The bench 58503A (3710A01056, 3704-C) works the same way at 19200 8N1.
The same day its ROM came back in 647 seconds and is
`third_party/firmware/58503a-3704.bin`, SHA-256
`d13b9ff1e4a0a59517aac4d066c60e22b290cf4ff6810c5c2bf01f1bc9491ca3`:
the same reset vector as 3633, its revision string `3704` at
`0x1309c`, and 345 of its 512 kilobytes different from the 3633
image.  Its EEPROM is `third_party/firmware/eeprom/58503a-3710A01056-eeprom.bin`.
While the console runs, only the SCPI task has ended: the loop, the
GPS task and the health monitor carry on, and the front panel with
them.

The bench Z3805A (3625A01487, 3543B-A) also works the same way, at
19200 8N1.  On 2026-09-27 its ROM came back in 654 seconds and is
`third_party/firmware/z3805a-3543b.bin`, SHA-256
`216daf929b293be02bfd92ed61cca8c7e70d577696f2567a12f01905f6998792`:
the same reset vector, its revision string `3543B` at `0x12eee`, where
the Z3801A image has `3543`, and 281 of its 512 kilobytes different
from `z3801a-3543.bin`.  Its console words are the Z3801A's, all 242.
Its EEPROM, `third_party/firmware/eeprom/z3805a-3625A01487-eeprom.bin`, opens with
`Z3805A`, `3625A01487` and `AS` where the Z3801A's has `AQ`; its
diagnostic log is stamped with calendar dates where the Z3801A's uses
hex.  The session is `docs/z3805a-pforth.txt`.

The 58503A's and the Z3801A's firmware words differ.  The 58503A's has
`force_ext_1pps` and `force_gps_1pps` where the Z3801A's has
`force_1pps`, and lacks the Z3801A's `adc_5v`, `adc_p15v`, `adc_m15v`,
`adc_oven`, `adc_doven`, `adc_ant_curr` and `adc_temp`; the rest are
the same.  The sessions are `docs/58503a-pforth.txt` and
`docs/z3801a-pforth.txt`.

## Which port

One.  The Z3801A manual (097-z3801-01) describes a single serial
interface, the RS-422 port on the rear I/O Port 1 connector, J3.  The
firmware drives two serial devices:

- the processor's own SCI, with the message exchanges `sciR` and `sciW`
  and the driver strings at `0xba16` and `0x2f395`;
- channel A of the 68681 DUART, `drta_send_message` and
  `drta_get_byte` (`0x2df43`), the GPS receiver link described above.

Channel B of the DUART buffers no data (`gps.md`, "The GPS receiver link"), its
transmit register (`0x200017`) is written only by the loopback
self-test, and the remaining references reset it.  So the console,
like SCPI, is on the SCI, and nothing in the firmware runs a console
on another port.  The image cannot show whether channel B is wired to
a header on the board.

The Z3801A's image, revision 3543, does the reverse.  It has no SCI
driver: its host port is DUART channel B, with the exchanges `drtR`
and `drtW` (`0x460c`) and the interrupt messages `DUARTB isr signaling
...` (`0x4686`), while `drta_get_byte` is still the GPS link on
channel A.  The Z3805A's own `:DIAGnostic:OS` listing names both
`sciR`/`sciW` and `drtR`/`drtW` (`scpi/undocumented.md`), so its firmware,
3543B, matches neither image exactly.

## The Z3801A image

Everything above was read from the Z3816A's image, revision 4001.  The
Z3801A's, revision 3543, was checked against it; what follows was
confirmed in its bytes.

**The same.**  CPU32: the reset vector points at `0x550`, which sets
the status register and the vector base with `movec`; the code before
it, `0x400` to `0x54f`, writes channel B's transmit register beside the
string `DUART`, a loopback test.  Every section's landmarks are present
at their own addresses: `pll_normal`'s and `startup_pll`'s failure
messages (`0x44e94`, `0x44e2e`), the fine stage's (`0x45e4a`), the
loop report (`0x44e6d`), `PFORTH`/`INSTALL`/`PRIMARY` (`0x2f711`), the
pForth banner (`0x18ad0`), `loop_time` (`0x1d172`), `max loop time`
(`0x1c0b8`), `SAWT ERR` (`0x4d57a`), and the loop's constants 29.75,
6.25 × 10⁻¹⁰ and 5.787 × 10⁻¹⁴.  The counter's port routine
(`0x4076a`) matches the Z3816A's (`0x440a6`) instruction for
instruction, as does the routine that joins two of its four-bit
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
- *Chip selects.*  Its reset code, at `0x550`, programs the SIM the
  same way except for the map: CSBARBT and CSBAR6 `0x0005`, 256 KB
  at 0, and CSBAR1 and CSBAR7 `0x0405`, 256 KB at `0x40000` -- the
  image's two halves; CSBAR0, 2, 3 `0x1003`, the 64 KB of RAM; CSBAR5
  `0x3003`, 64 KB at `0x300000`; CSBAR8 `0x2000`, the DUART; CSBAR9
  `0x4001`, 8 KB at `0x400000`; CSBAR4 `0x5000` and CSBAR10 `0xfff8`,
  2 KB each.  SYNCR is `0xcf00`, the same 16.777 MHz.  Nothing in it
  reads `0x302000`.
- *Serial ports.*  The host port is DUART channel B, with the exchanges
  `drtR` and `drtW` and `DUARTB isr` messages; the SCI is off -- the
  one reference to its registers, at `0x120da`, clears SCCR1.
- *The language command.*  Its handler (`0x2f57a`) compares the port
  the command came from with the descriptor `0x28a46` returns,
  `0x28cdc`, before it looks at `PFORTH`: a real check, where the
  Z3816A's always passes.  `0x28cdc` is the parser's one descriptor --
  two function pointers and the prompts `E%+04d> ` and `scpi > ` -- and
  its reader `FUN_00028a92` fills the buffer at `0x102b20` from stream
  0 (`FUN_0001dbc4`, `FUN_0001711e`).  The image has no second
  descriptor: nothing references the other copy of those prompts, at
  `0x7ee6`.
- *The loop.*  The term the Z3816A takes from `FUN_000324b0(6)` comes
  from `FUN_00022fd2(3)` (`0x4496e`).  A field the Z3816A initializes
  to 10⁻⁷ is 10⁻⁸ here (`0x44a3a`).  With no valid reading in ten
  seconds, `pll_normal` sets the mean to 0 and still runs the update
  (`0x44bb4` falls through to `0x44bd2`), where the Z3816A skips it.
  Its G is the fixed +6.25 × 10⁻¹³ noted above, and `pll_debug`
  (`0x1b4d8`) stores to `0x102539`.

## Resetting the GPS engine from the console

The Oncore's `@@Cf` sets the engine to its defaults and clears its
almanac, which makes a unit whose engine has held an almanac for years
search the sky afresh.  In 3543 only the console word `master_reset`
sends it; nothing reachable from SCPI does, nor does a power cycle or
`:SYSTem:PRESet`.

- *The send path.*  The Oncore descriptor table is at `0x4fe8c`, 56
  bytes an entry; `Cf` is entry 45 (`0x50864`), sent bare.  The one
  builder, `0x4f8f0`, is called only by the GPS task `FUN_0004bf08`,
  which takes request codes from the queue at `0x103678`: 0 to 0x30 one
  command, 0x47 to 0x59 a script of commands (selector `0x4bdc2`).  The
  one script holding `Cf` is `0x50c88`, run for code 0x47 only.
- *`master_reset`* (code `0x1b26c`) stores 25 in the mailbox
  `0x102562`; the loop task's dispatcher `FUN_00047074` turns that into
  code 0x47.  The GPS task first clears its own state (`0x4beb4`,
  `0x100c68` to `0x1011c1`, the same routine a cold start runs), then
  sends `Ci`, `Cf`, `Cj`, and a fixed set of HP defaults: among them
  `At` 0, position hold off, and `Ad`/`Ae`/`Af`/`As` with a position
  held as constants in ROM.  It writes nothing to the EEPROM, does not
  restart the processor, and does not send the unit's stored mask
  angle, cable delay or position.  Those reach the engine from the
  start-up sequence (`0x46802`), which a cold start and `pll_restart`
  run.
- *The others.*  `init_gps` (`0x1b28a`, code 0x48, script `0x50cf0`)
  re-sends the fixed option set, with no `Cf`.  `clear_nv` (`0x1bba6`)
  writes 0xFF over the first byte of both settings records, as
  `:SYSTem:PRESet` does.  `wr_eeprom` is a raw one-byte write;
  `disable_gps_cmds` sets one flag.  `gps_change` posts whatever code
  it is given, so could send `Cf` or run the whole `master_reset`
  script; which of its arguments is the code was not traced.
- *3543B* has the same logic at shifted addresses: table `0x4fece`,
  `Cf` at `0x508a6`, mailbox `0x10256c`, `master_reset` at `0x1b564`,
  and byte-identical scripts.

On 2026-09-27 `master_reset` was run on the bench Z3801A and Z3805A:
both engines then reported Bad Almanac, no satellite visible, and the
ROM's position, and started a blind search.  The Z3801A's 2016 almanac
was gone.  Not established: whether the engine accepts `Ci` 0, which
the VP Oncore reference does not list, and whether anything re-sends
the unit's settings before the next start-up sequence.

## Reading the GPS engine from the console

Read from 3543, and from 3543B at shifted addresses; none of it has
been run on a unit.  A word's arguments go to C in the order typed.

- `1 abr_stat` prints a `gs:` line on every `@@Ba`, once a second,
  with each channel's PRN, mode, signal strength and status
  (`FUN_000491ce`); `0 abr_stat` stops it.
- `print_bc` prints the `@@Bk` record, including `RECEIVER OSC
  OFFSET`, raw, in the VP reference's 0.1 m/s (`0x48480`).  The
  start-up scripts ask for `Bk` once; `0 ext_msg_rate` asks again.
- The engine's `@@Ca` self-test result is kept at `0x100c36` (3543B:
  `0x100c38`), with the bits the VP reference gives: channels 1 to 6
  correlation, 1 kHz presence, ROM, RAM, EEPROM, DCXO SPI, RTC.  It is
  run at power-up and by `*TST?` and `:DIAGnostic:TEST?`, both of which
  return the loop to power-up; `hex 100c36 w@ u.` reads the last result
  without running it.
- `code p1 p2 p3 p4 gps_change` sends entry `code` of the Oncore table
  (`0x4fe8c`): `41 0 0 0 0 gps_change` sends `@@Ca`, which leaves the
  engine idle until `46 1 0 0 0 gps_change` (`@@Cg 1`).

## `:DIAGnostic:TEMPerature?`

Undocumented, and the same code in both images.  The query handler
(`FUN_0002aae2` in the Z3801A image, `FUN_0002b6a0` in the 58503A's)
calls a routine (`0x1f010`, `0x1f1f0`) that reads ADC channel 0
through the converter routine (`0x1efc8`, `0x1f1a8`: command word
`channel << 11 | 0xc000`, result shifted right by two), keeps the low
byte, converts it to a float and multiplies by 0.273
(`0x3e8bc6a8`), with no offset.  So the answer is 0.273 °C per count
from a 0 V zero, at most 69.6.  In the Z3801A image nothing else reads
channel 0: the other eleven calls to the converter pass channels 1 to
6, or one from a register.  Both images carry the same
`Temperature: %f` and `Temperature: %.2f` strings.

The bench 58503A reads 34 to 38 °C, about 130 counts.  The bench
Z3801A (3543-A) and the Z3805A read 0 or 1 count on every poll logged
-- 1,422 and 36,867 of them -- so on those boards channel 0 is at or
near 0 V.  Whether a sensor is absent there or wired elsewhere is on
the board, not in the image.  The Z3801 dialect does not poll it.

## The 58503A image

`third_party/firmware/58503a-3633.bin` is a 58503A's program flash, revision
3633 (the string before the second copyright notice, at `0x12fea`),
Hewlett-Packard 1993, assembled from four AM29F010 dumps as
`third_party/NOTICE` describes.  It enters at `0x550` like the Z3801A's
image and is 42 % byte-identical to it, the block `0x70000` to
`0x7ffff` wholly so.  The bench receiver reports 3704-C, a later
revision, so what follows is read from 3633 and stated of it.  Its
SCPI tree has the same node layout, so the paths resolve the same
way; `scpi/58503a-3633.txt` lists them and `scpi/58503a.md` checks
them against the command table.

- *The loop.*  `pll_normal` is `FUN_0004491e` (message `pll_normal -
  Error with measurement` at `0x44ecc`), `startup_pll` reports from
  `0x44e66`.  The constants are the Z3816A's: 29.75 at `0x44a00`,
  2700 at `0x44750` and `0x447b6`, 150 s at `0x441cc` and `0x474c8`,
  G = +6.25 × 10⁻¹³ at `0x476bc` -- the Z3801A's value and sign;
  the bench 58503A (3704-C) measures +3.94 × 10⁻¹³, 0.63 of it
  (`efc.md`) -- and the clamp 6.25 × 10⁻¹⁰ at `0x475cc`.  The update
  has the c·s term: at `0x4480a` to `0x4481e`, `0x44a18` and `0x44d26`
  it multiplies the float at `0x102014` by D5, the value
  `FUN_00023120(3)` returned at `0x44964`, and adds it to the EFC.
- *`:DIAGnostic:ROSCillator:TCOefficient`.*  Its node (`0x5fe98`)
  names the query handler `FUN_0002b534` and the setter
  `FUN_0002b4fa`, both on the record at `0x337d4`: the cell
  `0x102014` -- the loop's c above -- limits −200 and 200 (`0x337c4`,
  `0x337c8`), getter `FUN_00012bac`, setter `FUN_000308f4`.  So on
  this revision the coefficient is the loop's constant on the
  oscillator current, as on the Z3816A.
- *The oscillator current.*  `FUN_00023120(n)` is a switch over eight
  channel records at `0x101e78` + 0x30·n, value at +0x28, live flag at
  +0x2c, a ROM default when the flag is clear; channel 3's default is
  250.0, the Z3816A's oscillator-current default, and the health pass
  `FUN_00022dec` writes it through `FUN_00022d30`: the same
  exponential average, s ← 0.1·fresh + 0.9·s, which sets the live
  flag on its first call.  The report strings name the channels
  Temperature, 5V, +15V, −15V, Oven, Double oven and Antenna current
  (`0x1c26c` on), and one format says `Oven current`.
- *The queries.*  `:DIAGnostic:ROSCillator:CURRent?` (`FUN_0002b47e`)
  returns `FUN_0001f36e`: a fresh ADC read, shifted right by two,
  converted and scaled -- not the loop's average.
  `:EFControl:ABSolute?` (`FUN_0002b4dc`) returns the float at
  `0x1024a0`, which `FUN_0001b7b0` sets to sixteen times the DAC word
  as it is written, so it is u after conversion, c·s included.
  `:DIAGnostic:PTIMe:TINTerval?` (`0x2b344`) is the one-second
  reading and `:SYNChronization:TINTerval?` and `:PTIMe:TINTerval?`
  (both `0x2fbf6`) the mean, as the bench 58503A showed.
- *The EFC scale.*  `:EFControl:RELative?` (`FUN_0002b48e`) returns
  (ABS − 2¹⁹) / 2¹⁹ × 100, which is the command table's
  "value / 2²⁰ × 200 − 100".  The EFC writer `FUN_0002334e` clamps u
  to 0 and 1 048 560 (2²⁰ − 16) and posts events 0x48 and 0x49 at the
  rails, and `FUN_0001b7b0` records 16 × the word it sends, so the DAC
  is written as a 16-bit word and ABS is that word times sixteen.
  (The bench 3704's ABS readings are not multiples of sixteen; 3704
  writes its cell from the same places, so why is not worked out.)
- *The channels.*  The reader's eight cases carry these defaults:
  0 → 25.0, 1 → 4.0, 2 → 15.0, 3 → 250.0, 4 → 5.0, 5 → 4.0, 6 → −15.0,
  7 → 50.0.  With the report strings, 0 is the temperature, 2, 4 and 6
  the +15 V, 5 V and −15 V rails, 3 the oscillator current and 7 the
  antenna current; 1 and 5, at 4.0, are the two oven readings, whose
  tolerance messages call one `Primary oven voltage` (`0x23cb6`) --
  which of the two is not determined here.
- *The console.*  The diagnostic word table has 63 words, among them
  `cal` where the Z3816A has `xcal` -- the same routine, printing
  `curr= %d efc= %.1f, sec remaining= %d` and `tempco = %f` and
  storing nothing (`0x1bf0c`) -- `doven`, `dmes_curv`, `pll_debug`,
  `pr_pll`, `pll_restart`, `phase_off`, `eman` and `master_reset`; the
  interpreter's banner `pForth $Revision` is at `0x18dbe`.  It has
  `:SYSTem:PON` (keyword `0x561df`, handler `0x3038e`, action
  `0x45da2`, which zeroes `0x100000` to `0x100bc7` and executes `trap
  #12`), as 3704 does (handler `0x3061a`, action `0x45e94`); only the
  Z3801A and Z3805A images lack it.

Revision 3704, read from the bench receiver itself
(`third_party/firmware/58503a-3704.bin`), has the same term in the same three
places.  Its `pll_normal` is `FUN_0004495a` (message at `0x44f22`); at
`0x44848`, `0x44a54` and `0x44d7c` it multiplies the float at
`0x102014` -- the cell its `TCOefficient` record also names -- by the
oven current held in D6, and the sum goes to the EFC writer
`FUN_00023592`, which clamps it as 3633's `FUN_0002334e` does.  The
current comes from channel 6, not 3: 3704's reader, `FUN_000222a6`,
takes a channel's filtered value from `0x101ea0` + 0x30·n or, while
the channel is not live, a default from a table of 42-byte
descriptors, and that table names the channels Temperature, 5V,
(none), Oven, 15V, −15V, Primary oven current and Antenna current, the
sixth with the default 250 that 3633 gives its channel 3.  Its
`EFControl:ABSolute?` handler, `FUN_0002b58a`, returns the cell at
`0x1024aa`, written from the same places 3633 writes its `0x1024a0`.
Channel 6 has its own per-call routine in the health monitor,
`FUN_00021dba` (the others share `FUN_00021d34`), and its own check and
act routines: on its first call it sets the channel's live flag and
stores the reading; on every later one it keeps s ← 0.1·fresh + 0.9·s
(`0x3dcccccd`, `0x3f666666`) and clears s if it falls below
`0x1e3ce508`, about 10⁻²⁰.  So once the monitor has reached channel 6
the loop uses the averaged measurement, not the default.
`:DIAGnostic:ROSCillator:CURRent?` (`FUN_0002b526`) returns a fresh
conversion of the same channel 6 (`FUN_00021ca2(6)`).

So the bench receiver's lack of response to the oven current in its
record (`efc.md`, "The regression") is not explained by the term being
absent, by the loop reading a default, or by the regression using a
different current; its cause is not worked out.  The running unit's
average at `0x101fc0`, its live flag at `0x101fc4` and the coefficient
at `0x102014` are readable in the console with `hex 101FC0 @ .` and
the like.

## In every image

Every image carries `pForth $Revision: 1.2 $`, the `PFORTH` value of
`:SYSTem:LANGuage` and the kernel word list with `halt`, whose code is
the same five instructions in each (the table in "Leaving it").  The
diagnostic words differ.  Against the Z3816A's, from the table of code
addresses and names around `loop_time` (read on 2026-10-09):

| Image | Words | Adds | Lacks |
| ----- | ----- | ---- | ----- |
| Z3816A 4001 | 80 | | |
| Z3801A 3543, Z3805A 3543B | 85 | `adc_5v`, `adc_ant_curr`, `adc_doven`, `adc_m15v`, `adc_oven`, `adc_p15v`, `adc_temp`, `force_1pps`, `pr_1pps` | `clr_satview`, `force_ext_1pps`, `force_gps_1pps`, `pr_satview` |
| 58503A 3633 | 86 | the seven `adc_` words, `pr_1pps` | `clr_satview`, `pr_satview` |
| 58503A 3704 | 79 | `pr_1pps` | `clr_satview`, `pr_satview` |
| Z3815A 4010 | 89 | `efc_comp`, `efc_comp_debug`, `n3_duart_res`, `n3_phase`, `pr_1pps`, `pr_bad_act`, `puck`, `s3_gps_avail`, `s3_hw_fail`, `s3_out_reg`, `s3_testmode`, `set_mux` | `cal`, `clr_satview`, `pr_satview` |
| 58503B 1.01.04 | 78 | | `cal`, `phase_off` |

So the 58503B has no `phase_off`, the one writer of the loop's phase
setpoint x₀ (`loop.md`, "The loop"), and 3704 drops the `adc_` words
3633 has.

The 58503B's own console session is
`third_party/firmware/58503b-L85376-console.log`, from another owner's
unit, L85376 at 1.01.04-D: it entered with `:SYST:LANG "PFORTH"`, which
answered with the banner; the prompt reads `p4th D >` and, after `hex`,
`p4th X >`, giving the number base; it read the flash with three words
of its own, as "Reading memory through it" does; and it left with
`watch`, after which the port answered `scpi >` and `:SYST:LANG?` gave
`"PRIMARY"`.  `watch` is a kernel word in every image (`0x2a0fa` in the
58503B's) that begins by saving every register to a block at
`0x101098`; what it does after that, and whether the unit restarted on
the way back to SCPI, was not traced.

