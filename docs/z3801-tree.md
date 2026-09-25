# Z3801A command tree

Every command path in the firmware image, read out of the parser's own
tables.  `z3801-keywords.md` has the vocabulary and the structures this
was extracted from; this is the tree they describe.

Extracted from `third_party/z3801a-3543.bin`.  `z3816a-4001.bin` holds the same
tables at the same offsets.

## What it is worth

62 of the 65 non-common Z3801 commands in `commands.toml` appear here,
which is the check that the extraction is right.  The three that do not:

  - `:DIAGnostic:ERASe` belongs to the INSTALL language, not PRIMARY.
  - `:ROSCillator:HOLDover:DURation:THReshold` and its `:EXCeeded`
    appear as `:DURation:MEASurement:THReshold`.  The short form answers
    on hardware, so `MEASurement` is optional here.

`:SOURce` is an optional header, as SCPI allows.  Everything under it
answers with or without it: `:PTIMe:FFOMerit?` and
`:SOURce:PTIMe:FFOMerit?` both returned `+3` from a Z3805A.

A path being here means the parser knows it.  It does not mean the
receiver implements it, and the difference is large: 282 of these were
put to a Z3805A as queries and **124 answered or declined, 158 came
back undefined**.  The tree is the vocabulary of the parser table, not
the command set of any one model, so treat a path as a candidate until
a receiver has answered it.

Some of these are writes.  The four-letter `R...`/`W...` pairs are a
factory interface of unknown effect and half of them write; `GARY`,
`DOUGlas`, `KENneth` and `ROBin` are developer commands.  None of those
has been sent.

**A question mark does not make a command safe.**  A status group read
with no leaf -- `:STATus:OPERation?`, `:STATus:QUEStionable?`,
`:STATus:OPERation:POWerup?` and the rest -- returns the *event*
register, and an event register clears when it is read.  Those five
were swept before this was understood, and they cleared the Z3805A's
latched events.  On a monitored receiver that would have thrown away
exactly the history the daemon exists to keep, and taken the operator's
front-panel alarm with it.  Any sweep list must exclude the bare group
forms and anything ending `:EVENt?`.

## Which commands are writable

Each node carries two handler slots: a setter at +8 from the keyword
pointer and a query at +18.  A setter of zero means the command is
read-only, and that is a fact about the firmware rather than about any
manual.  Of the 513 paths, 277 are writable and 236 are not.

Checked against the 71 Z3801-dialect entries in `commands.toml`, 70
agree.  The ten that look like disagreements are not: the table gives a
query and its setter separate ids where the firmware has one node with
both slots.

The one real gap is `:DIAGnostic:ROSCillator:TCOefficient`.  Its setter
slot holds `FUN_0002a93c`, so **the oscillator temperature coefficient
can be overwritten** -- and the command appears in no manual in either
direction.  Kusters describes the coefficient as measured against GPS
while locked and kept in EPROM, which reads as something the receiver
owns; the interface says otherwise, and the Z3816A image settles it:
the setter is the only writer, the loop never adjusts the value, and a
console word `xcal` measures it for an operator to enter
(`firmware.md`, "s, the oscillator current").

It is not in the command table.  The table requires every Z3801 entry
to exist on the 58503 tree too, and the evidence here is a Z3801A
image; there is no 58503A firmware to say the same of that model.
Recording it here, where the provenance is the image, is honest.  It
has not been sent, and writing a learned calibration constant into a
working reference's EPROM is not something to try casually: the
argument's range and units are undocumented beyond the parts in 10^12
per degree C derived in `efc.md`.

## What answered on a Z3805A

`:DIAGnostic:OS:MEMory?`, `:PROCess?` and `:STACk?` dump the whole
state of the real-time system, which is pSOS, and name its tasks:

    scpi   the command parser        pllp   the disciplining loop
    gpsm   GPS manager               curv   curve fitting
    hmon   health monitor            klok   clock
    spoo   spooler                   ROOT, IDLE

with message exchanges `1pps`, `pllc`, `plla`, `gpsx`, `logr`, `eepx`,
`sciR`/`sciW` and `drtR`/`drtW`, and about 13 percent of the CPU idle.
`curv` and `pllp` are the learning and the loop that Kusters' design
paper describes from the outside.

Others worth knowing:

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

## Paths

    :ACTion
    :BARR
    :BARR1
    :BARR2
    :BARRAY
    :BARRAY1
    :BARRAY2
    :BOOLean
    :DIAGnostic
    :DIAGnostic:GPSystem
    :DIAGnostic:GPSystem:POSition
    :DIAGnostic:GPSystem:POSition:DEGRees
    :DIAGnostic:GPSystem:POSition:HOLD
    :DIAGnostic:GPSystem:POSition:HOLD:RAIM
    :DIAGnostic:GPSystem:POSition:HOLD:RAIM:STATe
    :DIAGnostic:GPSystem:POSition:HOLD:RAIM:THReshold
    :DIAGnostic:GPSystem:POSition:MSEConds
    :DIAGnostic:GPSystem:POSition:SURVey
    :DIAGnostic:GPSystem:POSition:SURVey:RAIM
    :DIAGnostic:GPSystem:POSition:SURVey:RAIM:STATe
    :DIAGnostic:GPSystem:POSition:SURVey:RAIM:THReshold
    :DIAGnostic:GPSystem:TIME
    :DIAGnostic:GPSystem:UTC
    :DIAGnostic:IDENtification
    :DIAGnostic:IDENtification:DEFault
    :DIAGnostic:IDENtification:GPSystem
    :DIAGnostic:IDENtification:HARDware
    :DIAGnostic:IDENtification:MODel
    :DIAGnostic:IDENtification:SERial
    :DIAGnostic:LIFetime
    :DIAGnostic:LIFetime:COUNt
    :DIAGnostic:LOG
    :DIAGnostic:LOG:CLEar
    :DIAGnostic:LOG:COUNt
    :DIAGnostic:LOG:READ
    :DIAGnostic:LOG:READ:ALL
    :DIAGnostic:LOG:WRITe
    :DIAGnostic:ME
    :DIAGnostic:OS
    :DIAGnostic:OS:MEMory
    :DIAGnostic:OS:PROCess
    :DIAGnostic:OS:STACk
    :DIAGnostic:PTIMe
    :DIAGnostic:PTIMe:TINTerval
    :DIAGnostic:QUERy
    :DIAGnostic:QUERy:RESPonse
    :DIAGnostic:ROSCillator
    :DIAGnostic:ROSCillator:CURRent
    :DIAGnostic:ROSCillator:EFControl
    :DIAGnostic:ROSCillator:EFControl:ABSolute
    :DIAGnostic:ROSCillator:EFControl:RELative
    :DIAGnostic:ROSCillator:TCOefficient
    :DIAGnostic:SER
    :DIAGnostic:SER1
    :DIAGnostic:SER1:EGResponse
    :DIAGnostic:SER1:RESTricted
    :DIAGnostic:SER2
    :DIAGnostic:SER2:EGResponse
    :DIAGnostic:SER2:RESTricted
    :DIAGnostic:SER:EGResponse
    :DIAGnostic:SER:RESTricted
    :DIAGnostic:SERIAL
    :DIAGnostic:SERIAL1
    :DIAGnostic:SERIAL1:EGResponse
    :DIAGnostic:SERIAL1:RESTricted
    :DIAGnostic:SERIAL2
    :DIAGnostic:SERIAL2:EGResponse
    :DIAGnostic:SERIAL2:RESTricted
    :DIAGnostic:SERIAL:EGResponse
    :DIAGnostic:SERIAL:RESTricted
    :DIAGnostic:SLOG
    :DIAGnostic:SLOG:CLEar
    :DIAGnostic:SLOG:COUNt
    :DIAGnostic:SLOG:READ
    :DIAGnostic:SLOG:READ:ALL
    :DIAGnostic:STATus
    :DIAGnostic:STATus:ERRor
    :DIAGnostic:STATus:HAPPening
    :DIAGnostic:SYSTem
    :DIAGnostic:SYSTem:DOUTput
    :DIAGnostic:SYSTem:PSTartup
    :DIAGnostic:TEMPerature
    :DIAGnostic:TEST
    :DIAGnostic:TEST:RESult
    :DIAGnostic:TOFFset
    :FARR
    :FARR1
    :FARR2
    :FARRAY
    :FARRAY1
    :FLOat
    :FORMat
    :FORMat:DATA
    :IARR
    :IARR1
    :IARR2
    :IARR3
    :IARR4
    :IARRAY
    :IARRAY1
    :IARRAY2
    :IARRAY3
    :IARRAY4
    :IMULtiple
    :INTeger
    :LED
    :LED:ACTive
    :LED:ALARm
    :LED:ALARm:MAJor
    :LED:ALARm:MINor
    :LED:ALARm:USER
    :LED:ENABled
    :LED:GPSLock
    :LED:HOLDover
    :LED:TMHValid
    :OUTPut
    :OUTPut:PIN1
    :OUTPut:PIN1:DELay
    :OUTPut:PIN1:DELay:ALIGnment
    :OUTPut:PIN1:FREQuency
    :OUTPut:PIN2
    :OUTPut:PIN2:DELay
    :OUTPut:PIN2:DELay:ALIGnment
    :OUTPut:PIN2:FREQuency
    :OUTPut:PIN3
    :OUTPut:PIN3:DELay
    :OUTPut:PIN3:DELay:ALIGnment
    :OUTPut:PIN3:FREQuency
    :OUTPut:PIN6
    :OUTPut:PIN6:DELay
    :OUTPut:PIN6:DELay:ALIGnment
    :OUTPut:PIN6:FREQuency
    :OUTPut:PIN7
    :OUTPut:PIN7:DELay
    :OUTPut:PIN7:DELay:ALIGnment
    :OUTPut:PIN7:FREQuency
    :OUTPut:PIN8
    :OUTPut:PIN8:DELay
    :OUTPut:PIN8:DELay:ALIGnment
    :OUTPut:PIN8:FREQuency
    :OUTPut:PINS
    :OUTPut:PINS:DELay
    :OUTPut:PINS:DELay:ALIGnment
    :OUTPut:PINS:FREQuency
    :OUTPut:STATe
    :SAMPle
    :SENSe
    :SENSe:DATA
    :SENSe:DATA:CLEar
    :SENSe:DATA:MEMory
    :SENSe:DATA:MEMory:OVERflow
    :SENSe:DATA:MEMory:OVERflow:COUNt
    :SENSe:DATA:MEMory:SAVE
    :SENSe:DATA:POINts
    :SENSe:DATA:TSTamp
    :SENSe:TST
    :SENSe:TST1
    :SENSe:TST1:EDGE
    :SENSe:TST2
    :SENSe:TST2:EDGE
    :SENSe:TST3
    :SENSe:TST3:EDGE
    :SENSe:TST4
    :SENSe:TST4:EDGE
    :SENSe:TST:EDGE
    :SENSe:TSTAMP
    :SENSe:TSTAMP1
    :SENSe:TSTAMP1:EDGE
    :SENSe:TSTAMP2
    :SENSe:TSTAMP2:EDGE
    :SENSe:TSTAMP3
    :SENSe:TSTAMP3:EDGE
    :SENSe:TSTAMP4
    :SENSe:TSTAMP4:EDGE
    :SENSe:TSTAMP:EDGE
    :SOURce
    :SOURce:GPSystem
    :SOURce:GPSystem:ADELay
    :SOURce:GPSystem:EMANgle
    :SOURce:GPSystem:INITial
    :SOURce:GPSystem:INITial:DATE
    :SOURce:GPSystem:INITial:POSition
    :SOURce:GPSystem:INITial:TIME
    :SOURce:GPSystem:POSition
    :SOURce:GPSystem:POSition:ACTual
    :SOURce:GPSystem:POSition:HOLD
    :SOURce:GPSystem:POSition:HOLD:LAST
    :SOURce:GPSystem:POSition:HOLD:STATe
    :SOURce:GPSystem:POSition:SURVey
    :SOURce:GPSystem:POSition:SURVey:DURation
    :SOURce:GPSystem:POSition:SURVey:PROGress
    :SOURce:GPSystem:POSition:SURVey:STATe
    :SOURce:GPSystem:POSition:SURVey:STATe:POWerup
    :SOURce:GPSystem:REFerence
    :SOURce:GPSystem:REFerence:ADELay
    :SOURce:GPSystem:REFerence:VALid
    :SOURce:GPSystem:SATellite
    :SOURce:GPSystem:SATellite:TRACking
    :SOURce:GPSystem:SATellite:TRACking:COUNt
    :SOURce:GPSystem:SATellite:TRACking:EMANgle
    :SOURce:GPSystem:SATellite:TRACking:IGNore
    :SOURce:GPSystem:SATellite:TRACking:IGNore:ALL
    :SOURce:GPSystem:SATellite:TRACking:IGNore:COUNt
    :SOURce:GPSystem:SATellite:TRACking:IGNore:NONE
    :SOURce:GPSystem:SATellite:TRACking:IGNore:STATe
    :SOURce:GPSystem:SATellite:TRACking:INCLude
    :SOURce:GPSystem:SATellite:TRACking:INCLude:ALL
    :SOURce:GPSystem:SATellite:TRACking:INCLude:COUNt
    :SOURce:GPSystem:SATellite:TRACking:INCLude:NONE
    :SOURce:GPSystem:SATellite:TRACking:INCLude:STATe
    :SOURce:GPSystem:SATellite:VISible
    :SOURce:GPSystem:SATellite:VISible:PREDicted
    :SOURce:GPSystem:SATellite:VISible:PREDicted:COUNt
    :SOURce:PTIMe
    :SOURce:PTIMe:DATE
    :SOURce:PTIMe:FFOMerit
    :SOURce:PTIMe:GPSystem
    :SOURce:PTIMe:GPSystem:ADELay
    :SOURce:PTIMe:GPSystem:EMANgle
    :SOURce:PTIMe:GPSystem:INITial
    :SOURce:PTIMe:GPSystem:INITial:DATE
    :SOURce:PTIMe:GPSystem:INITial:POSition
    :SOURce:PTIMe:GPSystem:INITial:TIME
    :SOURce:PTIMe:GPSystem:POSition
    :SOURce:PTIMe:GPSystem:POSition:ACTual
    :SOURce:PTIMe:GPSystem:POSition:HOLD
    :SOURce:PTIMe:GPSystem:POSition:HOLD:LAST
    :SOURce:PTIMe:GPSystem:POSition:HOLD:STATe
    :SOURce:PTIMe:GPSystem:POSition:SURVey
    :SOURce:PTIMe:GPSystem:POSition:SURVey:DURation
    :SOURce:PTIMe:GPSystem:POSition:SURVey:PROGress
    :SOURce:PTIMe:GPSystem:POSition:SURVey:STATe
    :SOURce:PTIMe:GPSystem:POSition:SURVey:STATe:POWerup
    :SOURce:PTIMe:GPSystem:REFerence
    :SOURce:PTIMe:GPSystem:REFerence:ADELay
    :SOURce:PTIMe:GPSystem:REFerence:VALid
    :SOURce:PTIMe:GPSystem:SATellite
    :SOURce:PTIMe:GPSystem:SATellite:TRACking
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:COUNt
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:EMANgle
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:IGNore
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:IGNore:ALL
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:IGNore:COUNt
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:IGNore:NONE
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:IGNore:STATe
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:INCLude
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:INCLude:ALL
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:INCLude:COUNt
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:INCLude:NONE
    :SOURce:PTIMe:GPSystem:SATellite:TRACking:INCLude:STATe
    :SOURce:PTIMe:GPSystem:SATellite:VISible
    :SOURce:PTIMe:GPSystem:SATellite:VISible:PREDicted
    :SOURce:PTIMe:GPSystem:SATellite:VISible:PREDicted:COUNt
    :SOURce:PTIMe:LEAPsecond
    :SOURce:PTIMe:LEAPsecond:ACCumulated
    :SOURce:PTIMe:LEAPsecond:ACCumulated:CALCulate
    :SOURce:PTIMe:LEAPsecond:DATE
    :SOURce:PTIMe:LEAPsecond:DURation
    :SOURce:PTIMe:LEAPsecond:GPSTime
    :SOURce:PTIMe:LEAPsecond:STATe
    :SOURce:PTIMe:PPS
    :SOURce:PTIMe:PPS:EDGE
    :SOURce:PTIMe:SYNChronization
    :SOURce:PTIMe:SYNChronization:IMMediate
    :SOURce:PTIMe:TCODe
    :SOURce:PTIMe:TCODe:FORMat
    :SOURce:PTIMe:TFOMerit
    :SOURce:PTIMe:TIME
    :SOURce:PTIMe:TIME:STRing
    :SOURce:PTIMe:TINTerval
    :SOURce:PTIMe:TZONe
    :SOURce:PULSe
    :SOURce:PULSe:CONTinuous
    :SOURce:PULSe:CONTinuous:PERiod
    :SOURce:PULSe:CONTinuous:STATe
    :SOURce:PULSe:REFerence
    :SOURce:PULSe:REFerence:EDGE
    :SOURce:PULSe:STARt
    :SOURce:PULSe:STARt:DATE
    :SOURce:PULSe:STARt:TIME
    :SOURce:ROSCillator
    :SOURce:ROSCillator:HOLDover
    :SOURce:ROSCillator:HOLDover:DURation
    :SOURce:ROSCillator:HOLDover:DURation:MEASurement
    :SOURce:ROSCillator:HOLDover:DURation:MEASurement:THReshold
    :SOURce:ROSCillator:HOLDover:DURation:STATus
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold:ALAR1
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold:ALAR3
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold:ALARM1
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold:ALARM3
    :SOURce:ROSCillator:HOLDover:DURation:STATus:THReshold:EXCeeded
    :SOURce:ROSCillator:HOLDover:HYSTeresis
    :SOURce:ROSCillator:HOLDover:INITiate
    :SOURce:ROSCillator:HOLDover:LIMit
    :SOURce:ROSCillator:HOLDover:LIMit:STATe
    :SOURce:ROSCillator:HOLDover:LIMit:THReshold
    :SOURce:ROSCillator:HOLDover:RECovery
    :SOURce:ROSCillator:HOLDover:RECovery:AUTO
    :SOURce:ROSCillator:HOLDover:RECovery:INITiate
    :SOURce:ROSCillator:HOLDover:RECovery:LIMit
    :SOURce:ROSCillator:HOLDover:RECovery:LIMit:IGNore
    :SOURce:ROSCillator:HOLDover:TUNCertainty
    :SOURce:ROSCillator:HOLDover:TUNCertainty:PREDicted
    :SOURce:ROSCillator:HOLDover:TUNCertainty:PREDicted:DURation
    :SOURce:ROSCillator:HOLDover:TUNCertainty:PRESent
    :SOURce:ROSCillator:HOLDover:WAITing
    :SOURce:ROSCillator:LIMit
    :SOURce:ROSCillator:LIMit:THReshold
    :SOURce:ROSCillator:STATe
    :SOURce:SYNChronization
    :SOURce:SYNChronization:FFOMerit
    :SOURce:SYNChronization:HOLDover
    :SOURce:SYNChronization:HOLDover:DURation
    :SOURce:SYNChronization:HOLDover:DURation:MEASurement
    :SOURce:SYNChronization:HOLDover:DURation:MEASurement:THReshold
    :SOURce:SYNChronization:HOLDover:DURation:STATus
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold:ALAR1
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold:ALAR3
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold:ALARM1
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold:ALARM3
    :SOURce:SYNChronization:HOLDover:DURation:STATus:THReshold:EXCeeded
    :SOURce:SYNChronization:HOLDover:HYSTeresis
    :SOURce:SYNChronization:HOLDover:INITiate
    :SOURce:SYNChronization:HOLDover:LIMit
    :SOURce:SYNChronization:HOLDover:LIMit:STATe
    :SOURce:SYNChronization:HOLDover:LIMit:THReshold
    :SOURce:SYNChronization:HOLDover:RECovery
    :SOURce:SYNChronization:HOLDover:RECovery:AUTO
    :SOURce:SYNChronization:HOLDover:RECovery:INITiate
    :SOURce:SYNChronization:HOLDover:RECovery:LIMit
    :SOURce:SYNChronization:HOLDover:RECovery:LIMit:IGNore
    :SOURce:SYNChronization:HOLDover:TUNCertainty
    :SOURce:SYNChronization:HOLDover:TUNCertainty:PREDicted
    :SOURce:SYNChronization:HOLDover:TUNCertainty:PREDicted:DURation
    :SOURce:SYNChronization:HOLDover:TUNCertainty:PRESent
    :SOURce:SYNChronization:HOLDover:WAITing
    :SOURce:SYNChronization:IMMediate
    :SOURce:SYNChronization:LIMit
    :SOURce:SYNChronization:LIMit:THReshold
    :SOURce:SYNChronization:STATe
    :SOURce:SYNChronization:TFOMerit
    :SOURce:SYNChronization:TINTerval
    :STATus
    :STATus:OPERation
    :STATus:OPERation:CONDition
    :STATus:OPERation:ENABle
    :STATus:OPERation:EVENt
    :STATus:OPERation:HARDware
    :STATus:OPERation:HARDware:CONDition
    :STATus:OPERation:HARDware:ENABle
    :STATus:OPERation:HARDware:EVENt
    :STATus:OPERation:HARDware:NTRansition
    :STATus:OPERation:HARDware:PTRansition
    :STATus:OPERation:HOLDover
    :STATus:OPERation:HOLDover:CONDition
    :STATus:OPERation:HOLDover:ENABle
    :STATus:OPERation:HOLDover:EVENt
    :STATus:OPERation:HOLDover:NTRansition
    :STATus:OPERation:HOLDover:PTRansition
    :STATus:OPERation:NTRansition
    :STATus:OPERation:POWerup
    :STATus:OPERation:POWerup:CONDition
    :STATus:OPERation:POWerup:ENABle
    :STATus:OPERation:POWerup:EVENt
    :STATus:OPERation:POWerup:NTRansition
    :STATus:OPERation:POWerup:PTRansition
    :STATus:OPERation:PTRansition
    :STATus:PRESet
    :STATus:PRESet:ALARm
    :STATus:QUEStionable
    :STATus:QUEStionable:CONDition
    :STATus:QUEStionable:CONDition:USER
    :STATus:QUEStionable:ENABle
    :STATus:QUEStionable:EVENt
    :STATus:QUEStionable:EVENt:USER
    :STATus:QUEStionable:NTRansition
    :STATus:QUEStionable:PTRansition
    :SYSTem
    :SYSTem:COMMunicate
    :SYSTem:COMMunicate:SER
    :SYSTem:COMMunicate:SER1
    :SYSTem:COMMunicate:SER1:ADDRess
    :SYSTem:COMMunicate:SER1:FDUPlex
    :SYSTem:COMMunicate:SER1:PRESet
    :SYSTem:COMMunicate:SER1:PROMpt
    :SYSTem:COMMunicate:SER1:RECeive
    :SYSTem:COMMunicate:SER1:RECeive:BAUD
    :SYSTem:COMMunicate:SER1:RECeive:BITS
    :SYSTem:COMMunicate:SER1:RECeive:PACE
    :SYSTem:COMMunicate:SER1:RECeive:PARity
    :SYSTem:COMMunicate:SER1:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SER1:RECeive:SBITs
    :SYSTem:COMMunicate:SER1:TRANsmit
    :SYSTem:COMMunicate:SER1:TRANsmit:BAUD
    :SYSTem:COMMunicate:SER1:TRANsmit:BITS
    :SYSTem:COMMunicate:SER1:TRANsmit:PACE
    :SYSTem:COMMunicate:SER1:TRANsmit:PARity
    :SYSTem:COMMunicate:SER1:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SER1:TRANsmit:SBITs
    :SYSTem:COMMunicate:SER2
    :SYSTem:COMMunicate:SER2:ADDRess
    :SYSTem:COMMunicate:SER2:FDUPlex
    :SYSTem:COMMunicate:SER2:PRESet
    :SYSTem:COMMunicate:SER2:PROMpt
    :SYSTem:COMMunicate:SER2:RECeive
    :SYSTem:COMMunicate:SER2:RECeive:BAUD
    :SYSTem:COMMunicate:SER2:RECeive:BITS
    :SYSTem:COMMunicate:SER2:RECeive:PACE
    :SYSTem:COMMunicate:SER2:RECeive:PARity
    :SYSTem:COMMunicate:SER2:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SER2:RECeive:SBITs
    :SYSTem:COMMunicate:SER2:TRANsmit
    :SYSTem:COMMunicate:SER2:TRANsmit:BAUD
    :SYSTem:COMMunicate:SER2:TRANsmit:BITS
    :SYSTem:COMMunicate:SER2:TRANsmit:PACE
    :SYSTem:COMMunicate:SER2:TRANsmit:PARity
    :SYSTem:COMMunicate:SER2:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SER2:TRANsmit:SBITs
    :SYSTem:COMMunicate:SER:ADDRess
    :SYSTem:COMMunicate:SER:FDUPlex
    :SYSTem:COMMunicate:SER:PRESet
    :SYSTem:COMMunicate:SER:PROMpt
    :SYSTem:COMMunicate:SER:RECeive
    :SYSTem:COMMunicate:SER:RECeive:BAUD
    :SYSTem:COMMunicate:SER:RECeive:BITS
    :SYSTem:COMMunicate:SER:RECeive:PACE
    :SYSTem:COMMunicate:SER:RECeive:PARity
    :SYSTem:COMMunicate:SER:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SER:RECeive:SBITs
    :SYSTem:COMMunicate:SER:TRANsmit
    :SYSTem:COMMunicate:SER:TRANsmit:BAUD
    :SYSTem:COMMunicate:SER:TRANsmit:BITS
    :SYSTem:COMMunicate:SER:TRANsmit:PACE
    :SYSTem:COMMunicate:SER:TRANsmit:PARity
    :SYSTem:COMMunicate:SER:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SER:TRANsmit:SBITs
    :SYSTem:COMMunicate:SERIAL
    :SYSTem:COMMunicate:SERIAL1
    :SYSTem:COMMunicate:SERIAL1:ADDRess
    :SYSTem:COMMunicate:SERIAL1:FDUPlex
    :SYSTem:COMMunicate:SERIAL1:PRESet
    :SYSTem:COMMunicate:SERIAL1:PROMpt
    :SYSTem:COMMunicate:SERIAL1:RECeive
    :SYSTem:COMMunicate:SERIAL1:RECeive:BAUD
    :SYSTem:COMMunicate:SERIAL1:RECeive:BITS
    :SYSTem:COMMunicate:SERIAL1:RECeive:PACE
    :SYSTem:COMMunicate:SERIAL1:RECeive:PARity
    :SYSTem:COMMunicate:SERIAL1:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL1:RECeive:SBITs
    :SYSTem:COMMunicate:SERIAL1:TRANsmit
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:BAUD
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:BITS
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:PACE
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:PARity
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL1:TRANsmit:SBITs
    :SYSTem:COMMunicate:SERIAL2
    :SYSTem:COMMunicate:SERIAL2:ADDRess
    :SYSTem:COMMunicate:SERIAL2:FDUPlex
    :SYSTem:COMMunicate:SERIAL2:PRESet
    :SYSTem:COMMunicate:SERIAL2:PROMpt
    :SYSTem:COMMunicate:SERIAL2:RECeive
    :SYSTem:COMMunicate:SERIAL2:RECeive:BAUD
    :SYSTem:COMMunicate:SERIAL2:RECeive:BITS
    :SYSTem:COMMunicate:SERIAL2:RECeive:PACE
    :SYSTem:COMMunicate:SERIAL2:RECeive:PARity
    :SYSTem:COMMunicate:SERIAL2:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL2:RECeive:SBITs
    :SYSTem:COMMunicate:SERIAL2:TRANsmit
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:BAUD
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:BITS
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:PACE
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:PARity
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL2:TRANsmit:SBITs
    :SYSTem:COMMunicate:SERIAL:ADDRess
    :SYSTem:COMMunicate:SERIAL:FDUPlex
    :SYSTem:COMMunicate:SERIAL:PRESet
    :SYSTem:COMMunicate:SERIAL:PROMpt
    :SYSTem:COMMunicate:SERIAL:RECeive
    :SYSTem:COMMunicate:SERIAL:RECeive:BAUD
    :SYSTem:COMMunicate:SERIAL:RECeive:BITS
    :SYSTem:COMMunicate:SERIAL:RECeive:PACE
    :SYSTem:COMMunicate:SERIAL:RECeive:PARity
    :SYSTem:COMMunicate:SERIAL:RECeive:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL:RECeive:SBITs
    :SYSTem:COMMunicate:SERIAL:TRANsmit
    :SYSTem:COMMunicate:SERIAL:TRANsmit:BAUD
    :SYSTem:COMMunicate:SERIAL:TRANsmit:BITS
    :SYSTem:COMMunicate:SERIAL:TRANsmit:PACE
    :SYSTem:COMMunicate:SERIAL:TRANsmit:PARity
    :SYSTem:COMMunicate:SERIAL:TRANsmit:PARity:TYPE
    :SYSTem:COMMunicate:SERIAL:TRANsmit:SBITs
    :SYSTem:DATE
    :SYSTem:ERRor
    :SYSTem:LANGuage
    :SYSTem:PRESet
    :SYSTem:PRINt
    :SYSTem:PRINt:LENGth
    :SYSTem:STATus
    :SYSTem:STATus:LENGth
    :SYSTem:TIME
    :TARR
    :TARR1
    :TARR2
    :TARR3
    :TARRAY
    :TARRAY1
    :TARRAY2
    :TARRAY3
    :TOGGle
