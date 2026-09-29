# Recovery hypotheses

Research question: can a jumper or the eight-position S1 force recovery
when PRIMARY has valid checksums but cannot accept the command to enter
INSTALL?  The boards of interest are the Z3801A and Z3805A, with a
58503A available for comparison.  No S1 position has been identified.

These are hypotheses, not instructions for changing switches or wiring.
The investigation below used the firmware dumps and the MCU manual;
no receiver was accessed.  Results recorded on 2026-09-28.

## Candidate mechanisms

| Hypothesis | Mechanism | Current evidence |
| ---------- | --------- | ---------------- |
| 1. Alter primary flash reads | Address aliasing or select gating makes primary checksums fail while protected boot code remains readable. | Fits the reset code; modeled against both dumps below.  S1 wiring is unknown. |
| 2. Enable background debugging | A contact enables BDM through BKPT at reset; a debugger redirects execution to the protected installer. | MCU support and installer entry are established; board access and recovery are untested.  A switch alone does not select INSTALL. |
| 3. Select alternate boot storage | Bank selection changes the reset vectors or startup code presented to the CPU. | No alternate recovery image or selection circuit identified. |

The related 55300A manual assigns S1 B1 to serial-port defaults at
power-up and B2 to password enable (097-55300-01, figures 3-14 and
3-15A).  Those assignments are not established for these boards.  See
[firmware.md](firmware.md#forced-installer-entry-with-an-unusable-primary)
for that reference and the independent BDM candidate.

## 1. Alter primary flash reads

### What the firmware establishes

The Z3801A 3543 and Z3805A 3543B dumps have identical reset code from
`0x550` through `0x745`.  All 502 bytes were disassembled.  There is
no switch-byte test on this path: four lane checksum comparisons decide
whether to enter installer startup at `0x746` or jump to PRIMARY at
`0x744`.

Both configure two separate 256 KiB flash banks.  Addresses below are
CPU byte addresses, not individual flash-chip addresses.

| CPU range | Read select and setup | Write select and setup |
| --------- | --------------------- | ---------------------- |
| `0x00000`--`0x3ffff` | CSBOOT: CSBARBT = `0x0005`, CSORBT = `0x6870`, written at `0x578`/`0x580` | CS6: CSBAR6 = `0x0005`, CSOR6 = `0x7070`, written at `0x5e8`/`0x5f0` |
| `0x40000`--`0x7ffff` | CS1: CSBAR1 = `0x0405`, CSOR1 = `0x6870`, written at `0x598`/`0x5a0` | CS7: CSBAR7 = `0x0405`, CSOR7 = `0x7070`, written at `0x5f8`/`0x600` |

MC68331UM sections 4.8.1.2 and 4.8.1.3, tables 4-21 and 4-22, decode
these settings: 256 KiB blocks, both byte lanes, reads or writes only,
one internally generated wait state, supervisor and user spaces.
The physical connections between those selects and the flash parts
still need tracing.

The low bank contains both the protected installer and part of PRIMARY:

- `0x00000`--`0x0ffff`: protected boot region.
- `0x10000`--`0x3ffff`: first part of PRIMARY, including its vectors.
- `0x40000`--`0x7ffff`: remainder of PRIMARY.

The installer copy records begin at `0xbbc` and end with the terminator
at `0xb328` in both dumps.  Parsing every record confirms that their
source bytes are below `0x10000`.  The checksum-failure startup, copy
routine and installer source therefore remain readable if a fault
leaves the entire protected region unchanged.  This establishes a
necessary condition, not successful execution on faulty hardware.

### Read-fault modeling

For each altered read view, the calculation sums even and odd bytes
separately, modulo 65536.  Data ends at `0x3fffb` and `0x7fffb`; each
bank's final four bytes contain the two interleaved stored sums.  The
model alters those stored-sum reads too, as actual address aliasing or
bank deselection would.  It does not modify either source dump.

| Modeled read behavior | Protected region unchanged? | Z3801A 3543 checks | Z3805A 3543B checks |
| --------------------- | --------------------------- | ----------------- | ------------------ |
| Normal reads | Yes | Pass | Pass |
| Upper bank returns `0xff` for every byte | Yes | Fail | Fail |
| Upper bank returns `0x00` for every byte | Yes | **Pass** | **Pass** |
| Upper bank aliases the lower bank | Yes | Fail | Fail |
| Flash reads with CPU address bit 16 cleared | Yes | Fail | Fail |
| Flash reads with CPU address bit 17 cleared | Yes | Fail | Fail |

The address-bit cases were checked for the even lane alone, odd lane
alone and both lanes; all six cases fail on each image while preserving
every protected byte.  For interleaved eight-bit chips, CPU address bit
16 corresponds to chip address bit 15, and CPU bit 17 to chip bit 16,
assuming conventional sequential wiring.  That correspondence still
needs a board trace.  The model changes the address seen by flash; it
does not model grounding a driven CPU address line.

For the all-`0xff` case, each upper lane sums 131070 data bytes to
`0xfe02`, while its stored sum reads `0xffff`: failure is deterministic
under that assumed read value.  All-zero data and stored sums instead
compare equal.  A floating bus is not guaranteed to read either value.

The MCU supplies the read acknowledgment internally for these settings.
Consequently, externally gating the flash output while leaving the
MCU's chip-select configuration intact could complete bus cycles with
incorrect data rather than automatically produce a bus error.  That
is an inference from the manual; actual board loading and decode logic
are unmeasured.

### Implications for S1

The strongest specific versions of hypothesis 1 are:

1. A contact gates the upper bank's read enable, without disturbing
   the low bank.  It must produce checksum-breaking read values; mere
   deselection is insufficient evidence because zero-filled reads pass.
2. A contact causes flash address bit 15 or 16 to read low through
   suitable isolation or selection logic.  The corresponding CPU
   address alias preserves the installer while failing the checks on
   both existing images, even when it affects only one byte lane.

Neither needs the CPU to read S1 as a byte.  The firmware observes the
result indirectly through its checksum comparisons.

Disabling the entire low bank would also hide the reset code and is
not this mechanism.  A write-protect switch alone would not change
read checksums or prevent the jump to PRIMARY.  A global data-bit fault
would also affect boot instructions; the model does not support it as
a clean recovery method.

Even if a read override gets INSTALL running in RAM, normal flash
access must be restored before programming and verifying PRIMARY.
Whether a switch can be restored while running, whether its state is
latched at reset, and whether it also affects write addressing are
unresolved.  No live-switching procedure is established here.

### What would confirm or reject it

On an unpowered board, trace S1 to the flash address pins, read-enable
and chip-enable logic, and the CPU's CSBOOT/CS1/CS6/CS7 signals.
Record the assembly revision and switch numbering.  A connection to a
flash-side address selector or upper-bank read gate supports this
hypothesis.  A connection only to unrelated GPIO or debug signals
would redirect the investigation.

The model verifies checksum consequences for known images, not the
electrical circuit, an S1 assignment, an installer session, or recovery
of a real unit.  Those remain open.
