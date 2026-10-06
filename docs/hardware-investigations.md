# Hardware investigations

Questions a bench can settle and the firmware images cannot.  Each
item names the open question in `docs/firmware/README.md` it would close, what
to measure, and what the answer changes.  Receiver access needs
authorization under `AGENTS.md`; `smartclock-cli flash` has a specific
exception for firmware installation.  Items that need the console are
marked as wanting a spare unit.

Stop the daemon before any direct-mode work; it holds the port.

## 1. The word that chooses G

*Open item:* what drives bit 8 of the read-only port at `0x302000`
(chip select 7), which picks −1.25 × 10⁻¹² or −2.125 × 10⁻¹² per EFC
unit on the Z3816A.

- [ ] TODO: needs a Z3816A, which the bench lacks: trace CS7 to the
  device it strobes and what feeds its D8.

- Follow the MC68331's CS7 pin (CSPAR1 field 1; pin table in
  MC68331UM appendix D) to the device it strobes -- a buffer, a latch,
  or a switch pack.  Note what feeds its D8 line.
- If it is a link or switch, record its position and the unit's
  oscillator option.  If it is a signal, trace it to the oscillator or
  DAC board.
- Cross-check against the actual EFC sensitivity (item 2).

*Changes:* the G row of the loop table loses "by a hardware bit", and
the two Z3816A gains get names.

## 2. The actual EFC sensitivity and its sign

*Open item:* G is the sensitivity the firmware *assumes*, negative on
the Z3816A and positive on the Z3801A; neither is measured.

- [ ] TODO: measure G on the Z3801A and the Z3805A as on the 58503A.

- With the receiver in holdover or with the loop open (a `phase_off`
  setpoint is console-only; do not use it -- instead compare EFC
  against a frequency counter over a long locked run), log
  `:DIAGnostic:ROSCillator:EFControl:RELative?` and the 10 MHz
  frequency against an external reference.
- Fit frequency against EFC.  The slope in fractional frequency per
  percent, scaled by the EFC word per percent, is the real G.

*Measured on the bench 58503A, 3704-C, 2026-09-26* (`efc.md`, "The pull,
measured"): **+3.94 × 10⁻¹³ per count**, count up raising the frequency,
from a retrim that left the crystal on 10 MHz at 0 V and the receiver
holding count 0, 4.568 V, with the output 2.840 × 10⁻⁷ low.  The
58503A image, revision 3633, assumes G = +6.25 × 10⁻¹³: the sign
agrees and the real gain is 0.63 of the assumed.  Still open for the
Z3816A, whose image picks one of two negative values, and the Z3801A.

*Changes:* whether the loop's poles sit where the derivation puts
them (−1/(2τ)) or are scaled by the ratio of real to assumed G.

## 3. Which connector the SCI reaches

*Open item:* the SCPI port is the processor's own SCI in the Z3816A
image; that it is the RS-422 port on J3 is inferred, not traced.

- [ ] TODO: with a board open, trace the SCI's and the DUART channel B's
  pins to their connectors.

- Follow the SCI's TXD/RXD pins (PQS7/PQS6 on the MC68331) to the
  level converter and connector.
- On the Z3801A, follow the 68681 DUART's channel B (TXB/RXB) the
  same way, since that image drives its host port there.

*Changes:* closes "That the SCI is the port wired to J3".

## 4. Whether DUART channel B reaches a header

*Open item:* the Z3816A firmware leaves channel B idle apart from a
loopback self-test; a 59551A would need a second port for PORT 2.

- [ ] TODO: trace TXB/RXB from the DUART to any header.

- Follow TXB/RXB from the DUART.  Note any unpopulated header or
  spare connector they reach; no firmware talks on it, so this is
  documentation only.

## 5. The outer oven's enable and readback

*Open items:* that PORTGP bit 5 (PGP5) reaches P2/8, that the ADC is
an ADC0838, and how the Z3801A's "Oven" and "Secondary oven voltage"
channels map to volts at P2/9 are owners' reports, not traced here.

- [ ] TODO: trace PGP5 to P2/8, identify the ADC, and calibrate the P2/9
  channel against a meter.

- Trace PGP5 (MC68331 pin, see the manual's pinout) to P2/8 and
  confirm the level changes when the firmware's state passes
  `external oven warmup` (the `Oven Pwr` field on the status screen).
- Identify the ADC by its markings and confirm which of its inputs
  P2/9 feeds.
- Record the ADC count for P2/9 against a meter at two or three
  heater voltages, to give the "Secondary oven voltage" channel its
  unit.  The firmware's alarm limit is 6.8 in that unit.

*Changes:* the ovens section's owner-report caveats become
measurements; the health-monitor table gets a unit for the oven
channels.  The 58503A image (revision 3633) has two oven channels
with a default reading of 4.0 and a message naming one `Primary oven
voltage`, so the unit to establish is volts at P2/9 against that
reading.

## 6. The oscillator-current channel's unit

*Open item:* channel 6 (`Oscillator current`) has a nominal 250 and a
limit 650 after 4.489 per ADC count, unit unknown.

- [ ] TODO: measure across the oven supply's sense resistor against
  channel 6.

- Find the sense resistor in the oven supply; measure the voltage
  across it and the supply current at warm-up and at steady state, and
  compare with the channel's value from `:DIAGnostic:...` or the status
  screen.

*Changes:* the c·s term of the loop gets a physical scale; the
`Oscillator current` health channel gets a unit.

## 7. The warm-restart path

*Open item:* the firmware keeps its loop state across a reset when
the reset-status register shows neither EXT nor POW and the RAM
checksum holds.  Read, not exercised.

- [ ] TODO: on a spare unit, apply a brief external reset and watch
  whether tau and the aging fit survive.

- Both software restarts in the image discard that state before
  resetting (`docs/firmware/restart.md`, "Restarting"): `:SYSTem:PON` zeroes
  the region and `:SYSTem:PRESet` clears its flag.  Neither exercises
  the warm path, and this project sends neither.  `*TST?` does not
  reset the processor: it resets the GPS engine and sends the loop
  back through `powerup` with the loop's RAM intact, so it does not
  test this path either.  On a spare unit it is the one way to watch
  the receiver re-acquire from the daemon's log without touching the
  power.
- A brief external reset, if the board has a reset input, shows
  whether EXT alone (without POW) counts as warm.  No command triggers
  a RESET-instruction restart with the flag intact.
- Watch whether τ (loop time constant) and the aging fit survive: the
  EFC should not jump and the drift term should not go to zero.

*Changes:* confirms which reset sources keep the loop's state, and
whether `*TST?` is one of them.

## 8. The one-second reading versus the ten-second mean -- done

Measured 25 September 2026 on the bench 58503A: one hour of
`:DIAGnostic:PTIMe:TINTerval?` beside `:SYNChronization:TINTerval?`,
one pass a second through the daemon with raw commands enabled.  The
ten-second value is the mean of the ten one-second readings to
0.15 ns rms; MDEV of the means equals MDEV of the readings to 1 to
3 % at every tau; ADEV of the means is √10 low at every tau to 500 s.
The numbers are in `PLAN.md`, "Allan deviation is computed over
segments".  Plain `:PTIMe:TINTerval?` is the ten-second mean, by the
image and by 187 polls.

## 9. The console, on a spare unit only -- done

*Done:* the console has been used on the bench units to read their
ROM and EEPROM, and `halt` returns the port to SCPI (item 14).

*Open item:* what the pForth console does once started, and whether
anything short of a power cycle returns the port to SCPI.  Entering it
needs `:SYSTem:LANGuage "PFORTH"`, which only `smartclock-cli
read-memory` sends.

- On a unit that is not the bench reference: send the language
  command from a terminal, note the banner (`pForth $Revision:
  1.2 $`), try `words`, and try whether any pSOS word (`spawn` of the
  `sci` task's entry, `0x39706`) brings SCPI back.  Power-cycle to
  recover.
- While there, `302000 w@ .` prints the port word from item 1.

*Changes:* closes the console items, and item 1 without a probe.

## 10. A 58503A image -- done for revision 3633

`third_party/58503a-3633.bin`, assembled from willhb's flash dumps
(`NOTICE`), answered the first questions (`firmware/console.md`, "The 58503A
image"): the loop has the c·s term, `TCOefficient` is that c and its
setter writes it, the oscillator current is channel 3 with the same
exponential average, and `EFControl:ABSolute?` reports the DAC word
with the term in it.

The bench receiver's own flash, revision 3704-C, was read through its
pForth console (`firmware/console.md`, "Reading memory through it"): its loop
has the same term, on the same current renumbered as channel 6.

*Still open:* why the bench receiver's record shows no response to
the oven current when its firmware applies the term.

## 11. The two Z380x units that track nothing

*Open item:* why the Z3801A (3542A01548) has tracked no satellite
since it came to the bench, and the Z3805A (3625A01487) none since
2026-09-23, when it held six.  Both have the same engine, a Motorola
B1121P1114 with software 8.4 (`firmware/gps.md`, "The engines on the
bench").

The Z3801A's own log dates it: it cycled between GPS lock and
holdover from 2016-08-05 to its last lock on 2016-09-20, and every
power-on since is stamped 2016-09-24, the date its engine still holds.
It has tracked nothing since 2016, before the 2019 week rollover.

Checked on 2026-09-27:

- *The feed.*  Four receivers off the bench, on the same HP-designed
  distribution amplifier, track.  Both units are on DC-block ports,
  whose load resistor is what their antenna-current reading sees.
- *The Z3805A's internal RF cable.*  Checked by the operator.
- *The rails,* read in the pForth console with `adc_5v`, `adc_p15v`,
  `adc_m15v` and `adc_ant_curr`, and `hardware_bits` 0 in every
  snapshot of the six hours before:

  | | +5 V | +15 V | −15 V | Antenna current |
  | - | ---- | ----- | ----- | --------------- |
  | Z3801A | 4.94 | 14.95 | −15.21 | 30.39 |
  | Z3805A | 4.94 | 14.88 | −15.08 | 26.47 |

- *The engines' state,* from `print_stat` in the same sessions: date
  1/01/2007, no satellite visible or tracked, every assigned channel
  in mode 0, Code Search.  RX STATUS was 08 on the Z3801A and 09 on
  the Z3805A, whose bit 0 is Bad Almanac in the VP Oncore reference.
  The day before, the Z3801A's engine had read 02/10/2007, 2026-09-26
  less 1024 weeks.
- *An initial date and time.*  Both accepted
  `:GPS:INITial:DATE 2007,2,11` and `:GPS:INITial:TIME` at the same
  offset, 1024 weeks behind UTC, with no error (097-59551-02, 5-7 and
  5-8; volatile, and valid only before the first satellite is
  tracked).  Neither tracked a satellite in the 30 minutes after.
- *The feed, again.*  The HP 58517A eight-way amplifier then failed
  on and off for every receiver on it, whichever one powered it.  On
  an HP 58516A four-way with the Taoglas, a known-good receiver held
  a surveyed fix throughout while neither unit tracked a satellite in
  the 29 minutes after a power cycle.
- *`:SYSTem:PRESet`* on both, then an hour on the same four-way beside
  the same locked receiver: neither tracked a satellite.  Both came up
  surveying (`:GPS:POS:SURV:STAT?` `ONCE`) with a 10 degree mask and
  no PRNs ignored; the power-up survey setting read 1 on the Z3805A
  and 0 on the Z3801A.  `:SYSTem:PON` was refused by both.
- *What each engine expects to see.*  With the bench position held
  and the date and time given (1024 weeks behind UTC), on the same
  four-way as a known-good receiver, 2026-09-27 at about 21:25 UTC:
  the Z3805A predicted PRNs 3, 4, 6, 7, 9, 16, 26 and 27 within a
  degree or two of where the known-good receiver was tracking them, 42
  to 46 dB-Hz for the higher ones, and was attempting 3, 4, 7, 9, 16
  and 26, tracking none.  The Z3801A predicted 1, 8, 10, 11, 14, 18,
  22, 31 and 32, with the same elevations and azimuths its console's
  `print_vis` gave on 2026-09-26: its 2016 almanac, not the sky.  The
  Z3805A searches the right satellites in the right places and does
  not acquire them; the Z3801A searches the wrong ones.
- *`master_reset`, then 20 dB more gain* (a Raven LA-21-1575-100-T,
  under 3 dB noise figure) on the Z3805A: its engine held PRNs 9, 14
  and 22, which the known-good receiver had at 39 to 44 dB-Hz, for
  nearly five minutes without acquiring any.
- *Overnight, 2026-09-28, both after `master_reset`, a power cycle and
  the same 20 dB ahead of the four-way:* the Z3805A held lock about 98%
  of nine hours on 2 to 4 satellites; the Z3801A locked for part of an
  hour, then tracked nothing from 10:00 UTC on.

On 2026-10-03, over 24 hours on one splitter beside a 58503A and a
u-blox NEO-M8T, the Z3805A held no satellite the NEO-M8T read below
39 dB-Hz, with a 20 dB LNA ahead of it that the others lacked; the
58503A held 88% of those at 36 to 39 dB-Hz ([`bench-sky-2026-10-03.html`](https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/bench-sky-2026-10-03.html)).

*Next:* one of these engines in the 58503A (3710A01056), whose
firmware takes a six-channel engine (`firmware/gps.md`, "Six or eight
channels").  If it tracks there, the fault is in the Z380x unit; if
not, in the engine.  Waiting on a supply for the 58503A.

- [ ] TODO: fit a Z380x engine into the 58503A once its supply arrives.

## 12. Field upgrades -- Z3801A reinstall verified

The flasher (now `smartclock-cli flash`) reinstalled the Z3801A's own
dump on 2026-09-28; PRIMARY boot and the recorded settings were
verified.  See [the flasher](firmware/restart.md#the-flasher) for the
procedure, checks and limits.  Other models and revision changes have
simulator coverage, not hardware validation.  Which flash part holds
which byte lane is unknown.

An interrupted load that leaves invalid primary checksums enters the
protected installer at power-up.  Recovery after Ctrl-C mid-record is
untested, and no test is planned.  Recovery with valid checksums but
an unusable primary is item 13.

## 13. S1 and recovery from a checksum-valid unusable primary

*Open item:* what each of the eight positions on S1 controls, and
whether any forces INSTALL without a working primary interpreter.
The Z3801A 3543 and Z3805A 3543B reset-to-primary paths contain no
switch test, and a running primary enters the installer only through
the SCPI task (`firmware/restart.md`, "Forced installer entry with an unusable
primary").  The pForth console's `execute` reaches that same exit,
verified on the Z3801A and Z3805A on 2026-09-28.  This does not exclude
a hardware effect on booting.  The hypotheses below model flash read
faults that preserve the installer while failing primary checks.

- [ ] TODO: with the unit open, read the byte at `0x302000` from the
  pForth console (`3153920 c@ .`) before and after changing each S1
  position, one at a time, powered down.  The Oman installer reads that
  byte for its host-port settings (`firmware/restart.md`, "The switch byte at
  `0x302000`"); whether S1 drives it is unknown.  See hypothesis 4
  below.
- [ ] TODO: on an unpowered board, map S1's connections to buffers,
  CPU pins, flash or programmable logic.  Record the assembly
  revision, physical switch numbering and which contacts close in the
  marked ON position.
- Check whether any contact reaches the CPU's reset/debug signals or
  the flash's address/control signals.  Keep those possibilities
  separate from an ordinary input byte read by software.
- Locate BKPT/DSCLK, IFETCH/DSI and IPIPE/DSO.  MC68331UM section
  5.10.2 documents debug access independent of a running primary.
  Protected installer entry `0xa58` is a candidate destination after
  reset has initialized the memory interfaces; see `firmware/restart.md`,
  "Forced installer entry with an unusable primary".  Neither the
  board connection nor that recovery method has been tested.

*Changes:* a verified switch map and, if the board supports it, a
recovery procedure for a primary whose checksums pass but which cannot
accept `:SYSTem:LANGuage "INSTALL"`.

### Recovery hypotheses

*Speculative, by request: hypotheses to guide a bench investigation,
not findings.  Facts they rest on are in `firmware/README.md`.*

Research question: can a jumper or the eight-position S1 force recovery
when PRIMARY has valid checksums but cannot accept the command to enter
INSTALL?  The boards of interest are the Z3801A and Z3805A, with a
58503A available for comparison.  No S1 position has been identified;
the one candidate input is the byte at `0x302000` (hypothesis 4).

These are hypotheses, not instructions for changing switches or wiring.
The investigation used the firmware dumps and the MCU manual; no
receiver was accessed.  Results recorded on 2026-09-28.

### Candidate mechanisms

| Hypothesis | Mechanism | Current evidence |
| ---------- | --------- | ---------------- |
| 1. Alter primary flash reads | Address aliasing or select gating makes primary checksums fail while protected boot code remains readable. | Fits the reset code; modeled against both dumps below.  S1 wiring is unknown. |
| 2. Enable background debugging | A contact enables BDM through BKPT at reset; a debugger redirects execution to the protected installer. | MCU support and installer entry are established; board access and recovery are untested.  A switch alone does not select INSTALL. |
| 3. Select alternate boot storage | Bank selection changes the reset vectors or startup code presented to the CPU. | No alternate recovery image or selection circuit identified. |
| 4. S1 is the byte at `0x302000` | An 8-bit input in CS5's window that software reads. | Only the Oman installer reads it, to override host-port settings; no firmware on the bench units reads it.  Nothing ties it to S1. |

The related 55300A manual assigns S1 B1 to "Preset All Serial Ports at
Powerup" and B2 to "Password Required" (097-55300-01, figures 3-14 and
3-15A; `third_party/097-55300-01-iss-1.pdf`).  Those assignments are
not established for these boards.  The reset path and installer entry
the hypotheses rely on are in
[`firmware/restart.md`](firmware/restart.md#forced-installer-entry-with-an-unusable-primary).

### 1. Alter primary flash reads

#### What the firmware establishes

The Z3801A 3543 and Z3805A 3543B dumps have identical reset code from
`0x550` through `0x745`; all 502 bytes were disassembled.  The path has
no switch-byte test: four lane checksum comparisons decide whether to enter installer startup at `0x746` or jump to PRIMARY at
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
How those selects connect to the flash parts is not yet traced.

The low bank contains both the protected installer and part of PRIMARY:

- `0x00000`--`0x0ffff`: protected boot region.
- `0x10000`--`0x3ffff`: first part of PRIMARY, including its vectors.
- `0x40000`--`0x7ffff`: remainder of PRIMARY.

The installer copy records begin at `0xbbc` and end with the terminator
at `0xb328` in both dumps.  Every record's source bytes are below
`0x10000`.  The checksum-failure startup, copy routine and installer
source therefore remain readable if a fault leaves the whole protected
region unchanged.  That is a necessary condition, not successful
execution on faulty hardware.

#### Read-fault modeling

For each altered read view, the model sums even and odd bytes
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
every protected byte.  For interleaved eight-bit chips with
conventional sequential wiring, CPU address bit 16 is chip address bit
15, and CPU bit 17 is chip bit 16; this needs a board trace.  The model
changes the address flash sees; it does not model grounding a driven
CPU address line.

For the all-`0xff` case, each upper lane sums 131070 data bytes to
`0xfe02`, while its stored sum reads `0xffff`: under that read value
the check always fails.  All-zero data and stored sums compare equal.  A floating bus is not guaranteed to read either value.

The MCU supplies the read acknowledgment internally for these settings.
So gating the flash output externally, with the MCU's chip-select
configuration intact, could complete bus cycles with wrong data rather
than produce a bus error.  That is an inference from the manual; board
loading and decode logic are unmeasured.

#### Implications for S1

The strongest specific versions of hypothesis 1 are:

1. A contact gates the upper bank's read enable, leaving the low bank
   alone.  It must produce checksum-breaking read values; deselection
   alone is not enough, because zero-filled reads pass.
2. A contact causes flash address bit 15 or 16 to read low through
   suitable isolation or selection logic.  The corresponding CPU
   address alias preserves the installer while failing the checks on
   both existing images, even when it affects only one byte lane.

Neither needs the CPU to read S1 as a byte; the firmware sees the
result only through its checksum comparisons.

Disabling the whole low bank would also hide the reset code and is
not this mechanism.  A write-protect switch alone would not change
read checksums or prevent the jump to PRIMARY.  A global data-bit fault
would also affect boot instructions; the model does not support it as
a clean recovery method.

Even if a read override gets INSTALL running in RAM, normal flash
access must be restored before programming and verifying PRIMARY.
Whether a switch can be restored while running, whether its state is
latched at reset, and whether it affects write addressing are
unresolved.  No live-switching procedure is established.

#### What would confirm or reject it

On an unpowered board, trace S1 to the flash address pins, read-enable
and chip-enable logic, and the CPU's CSBOOT/CS1/CS6/CS7 signals.
Record the assembly revision and switch numbering.  A connection to a
flash-side address selector or upper-bank read gate supports this
hypothesis; one only to unrelated GPIO or debug signals redirects the
investigation.

The model verifies checksum consequences for known images, not the
electrical circuit, an S1 assignment, an installer session, or recovery
of a real unit.  Those remain open.

### 2. Enable background debugging

The MC68331 has an independent way to redirect execution: background
debug mode (BDM).  MC68331UM sections 5.10.2.1 and 5.10.2.2 describe
enabling it through BKPT at reset and entering it through a hardware
breakpoint.  Sections 5.10.2.5.2 and 5.10.2.6 describe changing the
return program counter (RPC) and resuming with GO.  After the reset
code has configured the memory interfaces, redirecting execution to the
protected installer entry at `0xa58` is a candidate recovery for a
checksum-valid primary that cannot accept commands.  This is an
inference from the CPU manual and firmware, not a tested procedure.
Access to BKPT/DSCLK, IFETCH/DSI and IPIPE/DSO on these boards has not
been mapped.

### 4. S1 is the byte at `0x302000`

The facts are in `firmware/restart.md`, "The switch byte at `0x302000`": the
Oman installer (58503A 3633) reads that byte at every start and, when
bit 0 is clear, takes the host port's baud, framing and pacing from
bits 1 to 4.  That matches the 55300A's S1 B1, "Preset All Serial
Ports at Powerup", in kind.  If S1 drives this byte, it sets the
installer's line settings; it does not force INSTALL, and on the
bench units' Peru and USA installers it does nothing at all.

It can be tested without firmware that uses it.  From the pForth
console, `3153920 c@ .` reads the byte (`0x302000` is 3153920).  Read
it, change one S1 position with the unit powered down, and read it
again; a bit that follows the switch maps that position.  If no bit
changes for any position, the hypothesis is rejected for that board.
This needs `:SYSTem:LANGuage "PFORTH"` and `halt` to return to SCPI
(item 14), and has not been done.  What else a read of `0x302000` does on
the Z3801A and Z3805A boards is unknown.

## 14. Leaving the console with `halt`

*Open item:* whether `halt` returns the port from the pForth console to
SCPI as the images say, and what each visit costs (`firmware/console.md`,
"Leaving it").

- With the daemon stopped, at the unit's own line settings, capture a
  transcript of: `*IDN?`, `:SYSTem:LANGuage?` and `:SYSTem:ERRor?`
  until empty; `:SYSTem:LANGuage "PFORTH"`; `mem_rep`; `halt`; then
  `*IDN?`, `:SYSTem:LANGuage?` (expect `"PRIMARY"`), `:SYSTem:ERRor?`
  and a few ordinary queries.
- Repeat once and compare the two `mem_rep` reports for the leak per
  visit.  No more than two visits per unit between power cycles.
- If the port stays silent or at the console prompt, leave it through
  the installer as `read-memory` does, or power cycle.

Done on 2026-10-04, two visits on each of the Z3805A and the 58503A:
`halt` returned to SCPI in PRIMARY each time with an empty error queue,
and each unit's second `mem_rep` matched its first, 18,790 and 16,248
bytes free (`firmware/console.md`, "Leaving it").

*Changes:* `read-memory` and its kin leave the console with `halt`
first on every image, keeping the installer route as the fallback, and
firmware with no known installer exit is no longer refused.

## 15. A receiver swapped under a running daemon

*Open item:* whether the slow tier's `*IDN?` check catches a cable
moved between two units on the same line settings (PLAN.md, "Rows
belong to a receiver, not to a file").

Done on 2026-10-05 with the installed daemons stopped and a throwaway
`smartclockd --device /dev/ttyUSB0 --baud 19200 --slow 10` logging to a
scratch file:

- *Z3805A to 58503A, slowly.*  Reads failed long enough to call the
  link dead; the daemon reopened and attached as the 58503A.  The
  Z3805A's rows end at 21:21:31 UTC under its serial, the 58503A's
  begin at 21:21:48 under its own.  The check took no part.
- *58503A to Z3805A, in a few seconds.*  No run of failures; the check
  logged `the receiver was swapped`, dropped the one queued command and
  reattached as the Z3805A.  Five rows from 21:22:51.5 to 21:22:54.5,
  with the Z3805A's EFC of 45.476 %, are filed under the 58503A: the
  window up to one slow pass that PLAN.md states.
