# Undocumented commands

What is known of the commands no manual here has: what their handlers
do in the firmware, and what a receiver answered.  A path is
undocumented where `models.md` shows no manual for it; that file also
lists, generated from the images, the undocumented paths whose node
has the handlers of a documented one, and the stubs, whose every
handler only refuses.  This file is written by hand; a test checks that
every path heading here names a path some image's tree holds.

Candidate paths built from the keyword table and sent to a receiver
show which it implements: an unknown header returns -113 and changes
nothing.  A read can still change state: a status group read with no
leaf, or anything ending `:EVENt?`, clears the event register it
returns (`z3801a.md`).

Handler addresses are in `58503a-3704.bin` unless another image is
named.

## Other names for documented commands

Each of these runs, query and setter, the handlers of a documented
branch (`models.md`, "Same handlers as a documented path"), in every
image that has it.

| Word | Same handlers as |
| ---- | ---------------- |
| `DOUGlas` | `:GPSystem:POSition:SURVey` |
| `GARY` | `:DIAGnostic:LOG` |
| `KENneth` | `:SYNChronization:HOLDover:TUNCertainty` |
| `MATThew` | `:ALARm` |
| `ROBin` | `:SENSe:DATA` |

### `:DOUGlas`

The position survey.  On 2026-10-09 the 58503A, the Z3805A and the
Z3801A each answered `:DOUGlas:DURation?` with `+7.20000E+003`,
`:DOUGlas:STATe?` with `0`, and `:DOUGlas:PROGress?` with -221,
"Settings conflict", which the handler raises when no survey is
running.

### `:GARY`

The diagnostic log.  `:GARY:COUNt?` answered `+24` on the 58503A,
`+64` on the Z3805A and `+41` on the Z3801A on 2026-10-09.  In the
58503B and Z3815A images `GARY` keeps only its own query, the handler
of `:DIAGnostic:LOG:READ:ALL`.

### `:KENneth`

The holdover time uncertainty: `:PREDicted?` is "an estimate of the
time interval error that can be expected for a one day holdover" and
`:PRESent?` "the current time interval error during holdover"
(097-59551-02, `:SYNChronization:HOLDover:TUNCertainty`).  The query
handlers read a state byte (`0x102476`): `:PRESent?` answers only in
states 3 and 4, and `:PREDicted?` in 3 to 6, its second field 1 in 3
and 4.  Answers of 2026-10-09:

| Receiver | `:PREDicted?` | `:PRESent?` |
| -------- | ------------- | ----------- |
| 58503A, locked | `+1.61E-005,0` | -221 |
| Z3805A | `+3.2E-006,1` | -221 |
| Z3801A, 464 s into holdover | `+1.01E-005,1` | `+1.0E-006` |

### `:ROBin`

The measurement arrays.  `:ROBin:POINts?` answered `+4,+5,+6` on the
58503A, the Z3805A and the Z3801A on 2026-10-09; `:SENSe:DATA:POINts?`
on a Z3805A had given three arrays.

## Found by sweeping a 58503A

1,530 candidates over the `:DIAGnostic` subtree found these.

| Command | Reading on the 58503A |
| ------- | --------------------- |
| `:DIAGnostic:TEMPerature?` | `+3.68550E+001`, degrees Celsius |
| `:DIAGnostic:ROSCillator:CURRent?` | `+1.05882E+002`, oven current |
| `:DIAGnostic:ROSCillator:TCOefficient?` | `-3.36500E+001`, oven-current constant (`firmware/loop.md`) |
| `:DIAGnostic:ROSCillator:EFControl:ABSolute?` | `+713392`, DAC code |
| `:DIAGnostic:ROSCillator:EFControl:DATA?` | `+0` |
| `:DIAGnostic:ROSCillator:EFControl?` | same as `:RELative?` |
| `:DIAGnostic:IDENtification:GPSystem?` | the GPS engine, also as `:GPS?` |
| `:DIAGnostic:IDENtification:DEFault?` | `"58503A","CQ"` |
| `:DIAGnostic:GPSystem:TIME?` | `+21,+41,+53,+2007,+2,+4`, time then date |
| `:DIAGnostic:GPSystem:UTC?` | `1` |
| `:DIAGnostic:TOFFset?` | `+0.00000E+000` |
| `:DIAGnostic:SLOG?` | oldest log entry, against `:LOG?` for the newest |

See `efc.md` for what the EFC commands mean in volts and in frequency.
`:DIAGnostic:TEMPerature?` is described in `firmware/console.md`; in
the Z3816A, Z3815A and 58503B images it is a stub (`models.md`).

`:DIAGnostic:IDENtification:GPSystem?` names the GPS engine: a Motorola
with `SOFTWARE DATE 06 Aug 1996`, before the 1024-week rollovers of
1999 and 2019, and so the source of the receiver's 1024-week date
error (`firmware/gps.md`, "The engines on the bench").

The firmware strings include `Double oven`.  The 58503A images carry it
too (`firmware/console.md`, "The 58503A image"); the bench 58503A's
oscillator is a 10811-60159 (`ocxo.md`).

## What answered on a Z3805A

`:DIAGnostic:OS:MEMory?`, `:PROCess?` and `:STACk?` dump the state of
the real-time system, pSOS, and name its tasks:

    scpi   the command parser        pllp   the disciplining loop
    gpsm   GPS manager               curv   curve fitting
    hmon   health monitor            klok   clock
    spoo   spooler                   ROOT, IDLE

with message exchanges `1pps`, `pllc`, `plla`, `gpsx`, `logr`, `eepx`,
`sciR`/`sciW` and `drtR`/`drtW`, and about 13 percent of the CPU idle.
`pllp` runs the disciplining loop and `curv` the aging fit
(`firmware/loop.md`, "The disciplining loop").

Other answers:

    :DIAGnostic:PTIMe:TINTerval?        time interval, unlocked reading
    :DIAGnostic:ROSCillator:EFControl?  the bare node answers as RELative
    :DIAGnostic:SER*:RESTricted?        a restricted mode, on both ports
    :DIAGnostic:SYSTem:PSTartup?        :DIAGnostic:SYSTem:DOUTput?
    :DIAGnostic:SLOG:COUNt?             the second log is real
    :SENSe:DATA:POINts?                 three measurement arrays
    :SYSTem:PRINt?                      the whole status screen again
    :LED:ACTive? :ALARm:MAJor? :ENABled? :TMHValid?

88 of the 118 that answered were `:SYSTem:COMMunicate` -- the serial
settings, which are per port and per direction.

## Read on a 58503A, 2026-10-09

Each sent to the bench 58503A (3704-C) through its daemon after its
query handler was read; none has a manual entry.

### `:DIAGnostic:GPSystem:ACURrent`

`+2.08431E+002`.  The handler returns a word from a table of eight
channels (`0x222a6`), the value read where one has been and a default
from the image where not.  Its unit is not established.

### `:DIAGnostic:GPSystem:TRACking:LOG`

`0`: a flag.

### `:DIAGnostic:INPut:DATA`

`+0`.  The handler returns zero whatever the state.

### `:DIAGnostic:REFerence:GPSystem:QUEStionable:HYSTeresis`

`+60`: a stored value, converted for the reply.  Its sibling under
`:EXTernal` is a stub, and answered -113.

### `:DIAGnostic:ROSCillator:EFControl:MODE`

`NORM`.  The handler maps a mode byte (`0x10217a`) to one of three
values.  New in 3704.

### `:DIAGnostic:TCODe`

`:ERRor:AMASk?` and `:STATus:AMASk?` answered `+255`, `:ERRor:OMASk?`
and `:STATus:OMASk?` `+0`: each a mask byte.  New in 3704.

### `:DIAGnostic:TMODe`

`:DATA?` answered `+0` and `:STATe?` `0`, each a byte.

### `:SOURce:PTIMe:UTC`

`1`: a flag (`0x102264`).  New in 3704.

## Stubs answered

`:OUTPut:PIN1:DELay?` and `:OUTPut:PIN1:FREQuency?` answered -113,
"Undefined header", on the 58503A, the Z3805A and the Z3801A, and
`:DIAGnostic:REFerence:EXTernal:QUEStionable:HYSTeresis?` on the
58503A: the refusal a stub's handler makes (`models.md`, "Stubs").
