# Hardware investigations

Things the firmware images cannot settle and a bench can.  Each item
names the open question in `docs/firmware.md` it would close, what to
measure, and what the answer changes.  None of them involves sending
the receiver anything this project never sends (`:SYSTem:PRESet`,
`:SYSTem:COMMunicate:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage`);
the ones that need the console are marked as wanting a spare unit.

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
it needs `:SYSTem:LANGuage "PFORTH"`, which this project never sends.

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

*Still open:* the bench receiver is revision 3704-C and its record
shows no response to the oven current where 3633's code would.  A
dump of a 3704 unit -- the bench receiver's own flash, with the same
programmer -- is what would settle it.
