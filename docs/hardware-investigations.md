# Hardware investigations

Things the firmware images cannot settle and a bench can.  Each item
names the open question in `docs/firmware.md` it would close, what to
measure, and what the answer changes.  Receiver access requires
authorization under `AGENTS.md`; firmware installation has a specific
exception for `smartclock-cli flash`.  Investigations that need the console
are marked as wanting a spare unit.

Stop the daemon before any direct-mode work; it holds the port.

## 1. The word that chooses G

*Open item:* what drives bit 8 of the read-only port at `0x302000`
(chip select 7), which picks −1.25 × 10⁻¹² or −2.125 × 10⁻¹² per EFC
unit on the Z3816A.

- Find the device CS7 selects: follow the MC68331's CS7 pin
  (CSPAR1 field 1; pin table in MC68331UM appendix D) to whatever it
  strobes -- a buffer, a latch, or a switch pack.  Note what feeds its
  D8 line.
- If it is a link or switch, record its position and the unit's
  oscillator option.  If it is a signal, trace it to the oscillator or
  DAC board.
- Cross-check against the actual EFC sensitivity (item 2).

*Changes:* the G row of the loop table stops saying "by a hardware
bit", and the two Z3816A gains get names.

## 2. The actual EFC sensitivity and its sign

*Open item:* G is the sensitivity the firmware *assumes*; the sign is
negative on the Z3816A and positive on the Z3801A, and neither is
measured.

- With the receiver in holdover or with the loop open (a `phase_off`
  setpoint is console-only; do not use it -- instead compare EFC
  against a frequency counter over a long locked run), log
  `:DIAGnostic:ROSCillator:EFControl:RELative?` and the 10 MHz
  frequency against an external reference.
- Fit frequency against EFC.  The slope in fractional frequency per
  percent, scaled by whatever the EFC word is per percent, is the real
  G.

*Measured on the bench 58503A, 3704-C, 2026-09-26* (`efc.md`, "The pull,
measured"): **+3.94 × 10⁻¹³ per count**, count up raising the frequency,
from a retrim that left the crystal on 10 MHz at 0 V and the receiver
holding count 0, 4.568 V, with the output 2.840 × 10⁻⁷ low.  The
58503A image, revision 3633, assumes G = +6.25 × 10⁻¹³: the sign
agrees and the real gain is 0.63 of the assumed.  Still open for the
Z3816A, whose image picks one of two negative values, and the Z3801A.

*Changes:* whether the loop's poles sit where the doc's derivation
puts them (−1/(2τ)) or are scaled by the ratio of real to assumed G.

## 3. Which connector the SCI reaches

*Open item:* the SCPI port is the processor's own SCI in the Z3816A
image; that it is the RS-422 port on J3 is inferred, not traced.

- Follow the SCI's TXD/RXD pins (PQS7/PQS6 on the MC68331) to the
  level converter and connector.
- On the Z3801A, follow the 68681 DUART's channel B (TXB/RXB) the
  same way, since that image drives its host port there.

*Changes:* closes "That the SCI is the port wired to J3".

## 4. Whether DUART channel B reaches a header

*Open item:* the Z3816A firmware leaves channel B idle apart from a
loopback self-test; a 59551A would need a second port for PORT 2.

- Follow TXB/RXB from the DUART.  If they end at an unpopulated header
  or a spare connector, note it; the firmware has nothing that would
  talk on it, so this is documentation only.

## 5. The outer oven's enable and readback

*Open items:* that PORTGP bit 5 (PGP5) reaches P2/8, that the ADC is an
ADC0838, and how the Z3801A's "Oven" and "Secondary oven voltage"
channels map to volts at P2/9, are owners' reports, not traced here.

- Trace PGP5 (MC68331 pin, see the manual's pinout) to P2/8 and
  confirm the level changes when the firmware's state passes
  `external oven warmup` (the `Oven Pwr` field on the status screen).
- Identify the ADC by its markings and confirm which of its inputs
  P2/9 feeds.
- Record the ADC count for P2/9 against a meter reading at two or
  three heater voltages, to give the "Secondary oven voltage" channel
  its unit.  The firmware's alarm limit is 6.8 in the channel's own
  unit.

*Changes:* the ovens section's owner-report caveats become
measurements; the health-monitor table gets a unit for the oven
channels.  The 58503A image (revision 3633) shows two oven channels
with a default reading of 4.0 and a message naming one `Primary oven
voltage`, so the unit to establish is volts at P2/9 against that
reading.

## 6. The oscillator-current channel's unit

*Open item:* channel 6 (`Oscillator current`) has a nominal 250 and a
limit 650 after 4.489 per ADC count, unit unknown.

- Find the sense resistor in the oven supply and measure the voltage
  across it and the supply current at warm-up and at steady state;
  compare with the channel's value from `:DIAGnostic:...` or the
  status screen.

*Changes:* the c·s term of the loop gets a physical scale; the
`Oscillator current` health channel gets a unit.

## 7. The warm-restart path

*Open item:* the firmware keeps its loop state across a reset when
the reset-status register shows neither EXT nor POW and the RAM
checksum holds; this was read, not exercised.

- The two software restarts the image has both discard that state
  before resetting (`docs/firmware.md`, "Restarting"): `:SYSTem:PON`
  zeroes the region and `:SYSTem:PRESet` clears its flag, so neither
  exercises the warm path, and neither is a command this project
  sends.  `*TST?` does not reset the processor at all: it resets the
  GPS engine and sends the loop back through `powerup`, with the
  loop's RAM intact, so it is not a test of this path either -- though
  on a spare unit it is the one way to watch the receiver re-acquire
  from the daemon's log without touching the power.
- A brief external reset, if the board has a reset input, answers
  whether EXT alone (without POW) is treated as warm; a RESET-
  instruction restart with the flag intact has no command that
  triggers it.
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

## 9. The console, on a spare unit only

*Open item:* what the pForth console does once started, and whether
anything short of a power cycle returns the port to SCPI.  Entering
it needs `:SYSTem:LANGuage "PFORTH"`, which only
`smartclock-cli read-memory` sends.

- On a unit that is not the bench reference: send the language
  command from a terminal, observe the banner (`pForth $Revision:
  1.2 $`), try `words`, and try whether any pSOS word (`spawn` of the
  `sci` task's entry, `0x39706`) brings SCPI back.  Power-cycle to
  recover.
- While there, `302000 w@ .` would print the port word from item 1
  directly.

*Changes:* closes the console items, and item 1 without a probe.

## 10. A 58503A image -- done for revision 3633

`third_party/58503a-3633.bin`, assembled from willhb's flash dumps
(`NOTICE`), answered the first questions (`firmware.md`, "The 58503A
image"): the loop has the c·s term, `TCOefficient` is that c and its
setter writes it, the oscillator current is channel 3 with the same
exponential average, and `EFControl:ABSolute?` reports the DAC word
with the term in it.

The bench receiver's own flash, revision 3704-C, was read through its
pForth console (`firmware.md`, "Reading memory through it"): its loop
has the same term, on the same current renumbered as channel 6.

*Still open:* why the bench receiver's record shows no response to the
oven current when its firmware applies the term.

## 11. The two Z380x units that track nothing

*Open item:* why the Z3801A (3542A01548) has tracked no satellite
since it came to the bench, and the Z3805A (3625A01487) none since
2026-09-23, when it held six.  Both have the same engine, a Motorola
B1121P1114 with software 8.4 (`firmware.md`, "The engines on the
bench").

The Z3801A's own log dates it: it cycled between GPS lock and holdover
from 2016-08-05 to its last lock on 2016-09-20, and every power-on
since is stamped 2016-09-24, the date its engine still holds.  So it
has tracked nothing since 2016, before the 2019 week rollover.

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
- *An initial date and time.*  `:GPS:INITial:DATE 2007,2,11` and
  `:GPS:INITial:TIME` at the same offset, 1024 weeks behind UTC, were
  accepted by both with no error (097-59551-02, 5-7 and 5-8; volatile,
  and valid only before the first satellite is tracked).  Neither
  tracked a satellite in the 30 minutes after.
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
  the Z3805A predicted PRNs 3, 4, 6, 7, 9, 16, 26 and 27 within a degree
  or two of where the known-good receiver was tracking them, 42 to
  46 dB-Hz for the higher ones, and was attempting 3, 4, 7, 9, 16 and
  26, tracking none.  The Z3801A predicted 1, 8, 10, 11, 14, 18, 22, 31
  and 32, with the same elevations and azimuths its console's
  `print_vis` gave on 2026-09-26: its 2016 almanac, not the sky.  So the
  Z3805A searches the right satellites in the right places and does
  not acquire them, and the Z3801A searches the wrong ones.
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
58503A held 88% of those at 36 to 39 dB-Hz (`sky-comparison.html`).

*Next:* one of these engines in the 58503A (3710A01056), whose
firmware takes a six-channel engine (`firmware.md`, "Six or eight
channels").  Tracking there puts the fault in the Z380x unit; not
tracking, in the engine.  Waiting on a supply for the 58503A.

## 12. Field upgrades -- Z3801A reinstall verified

The flasher (now `smartclock-cli flash`) reinstalled the Z3801A's own dump on 2026-09-28;
PRIMARY boot and the recorded settings were verified.  See
[the flasher](firmware.md#the-flasher) for the procedure, checks and
limits.  Other models and revision changes have simulator coverage,
not hardware validation.  Which flash part holds which byte lane is
still unknown.

An interrupted load that leaves invalid primary checksums enters the
protected installer at power-up.  Recovery after Ctrl-C mid-record
remains untested; no test is planned.  Recovery with valid checksums
but an unusable primary is the separate open item below.

## 13. S1 and recovery from a checksum-valid unusable primary

*Open item:* what each of the eight positions on S1 controls, and
whether any forces INSTALL without a working primary interpreter.
The Z3801A 3543 and Z3805A 3543B reset-to-primary paths contain no
switch test, and a running primary enters the installer only through
the SCPI task (`firmware.md`, "Forced installer entry with an unusable
primary").  The pForth console's `execute` reaches that same exit and
was verified on the Z3801A and Z3805A on 2026-09-28.  This does not exclude a
hardware effect on booting.
See [recovery-hypothesis.md](recovery-hypothesis.md) for modeled flash
read faults that preserve the installer while failing primary checks.

- [ ] TODO: with the unit open, read the byte at `0x302000` from the
  pForth console (`3153920 c@ .`) before and after changing each S1
  position, one at a time, powered down.  The Oman installer reads that
  byte for its host-port settings (`firmware.md`, "The switch byte at
  `0x302000`"); whether S1 drives it is unknown.  See
  [recovery-hypothesis.md](recovery-hypothesis.md), hypothesis 4.
- [ ] TODO: on an unpowered board, map S1's connections to buffers, CPU pins,
  flash or programmable logic.  Record the assembly revision, physical
  switch numbering and which contacts close in the marked ON position.
- Check whether any contact reaches the CPU's reset/debug signals or
  the flash's address/control signals.  Keep those possibilities
  separate from an ordinary input byte read by software.
- Locate BKPT/DSCLK, IFETCH/DSI and IPIPE/DSO.  MC68331UM section
  5.10.2 documents debug access independent of a running primary.
  Protected installer entry `0xa58` is a candidate destination after
  reset has initialized the memory interfaces; see `firmware.md`,
  "Forced installer entry with an unusable primary".  Neither the
  board connection nor that recovery method has been tested.

*Changes:* a verified switch map and, if supported by the board, a
recovery procedure for a primary whose checksums pass but which cannot
accept `:SYSTem:LANGuage "INSTALL"`.

## 14. Leaving the console with `halt`

*Open item:* whether `halt` returns the port from the pForth console to
SCPI as the images say, and what each visit costs (`firmware.md`,
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

Done on the Z3805A on 2026-10-04, two visits: `halt` returned to SCPI
in PRIMARY each time with an empty error queue, and the second visit's
`mem_rep` matched the first, 18,790 bytes free (`firmware.md`,
"Leaving it").  Not yet tried on a 58503A.

*Changes:* `read-memory` and its kin leave the console with `halt`
first on every image, keeping the installer route as the fallback, and
firmware with no known installer exit is no longer refused.
