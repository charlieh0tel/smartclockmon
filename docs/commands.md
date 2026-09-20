# Command matrix

Generated from `crates/smartclock/commands.toml`.  Run `make docs`
to regenerate; a test fails if this file and the table disagree.

## What is in the table

| Tree | Commands | Hardware | Firmware | Manual |
| ---- | -------- | -------- | -------- | ------ |
| 58503A/B, 59551A | 113 | 81 | 0 | 32 |
| Z3801A, Z3816A | 56 | 0 | 55 | 1 |

**H** means the receiver answered it.  **F** means every keyword
appears in the firmware's own keyword table, so the spelling is
right though no such receiver has been on the line.  **M** means
it was transcribed from a manual and nothing more.

| Class | Commands | Gate |
| ----- | -------- | ---- |
| Query | 81 | none |
| Control | 24 | `--allow-control` |
| Dangerous | 8 | `--allow-dangerous` |

## Undocumented

11 commands appear in none of the manuals here.  They were found
by building candidate paths from the firmware's keyword table and
sending them to a receiver: an unknown header returns -113 and
changes nothing, so a sweep is safe and settles the question.

| Command | Operation | Found on |
| ------- | --------- | -------- |
| `:DIAGnostic:TEMPerature?` | temperature | 58503A 3710A01056 |
| `:DIAGnostic:ROSCillator:CURRent?` | oven current | 58503A 3710A01056 |
| `:DIAGnostic:ROSCillator:EFControl:ABSolute?` | efc absolute | 58503A 3710A01056 |
| `:DIAGnostic:ROSCillator:TCOefficient?` | oven tempco | 58503A 3710A01056 |
| `:DIAGnostic:IDENtification:GPSystem?` | gps engine identity | 58503A 3710A01056 |
| `:DIAGnostic:IDENtification:DEFault?` | model identity | 58503A 3710A01056 |
| `:DIAGnostic:GPSystem:TIME?` | gps engine time | 58503A 3710A01056 |
| `:DIAGnostic:GPSystem:UTC?` | gps engine utc | 58503A 3710A01056 |
| `:DIAGnostic:TOFFset?` | time offset | 58503A 3710A01056 |
| `:DIAGnostic:ROSCillator:EFControl:DATA?` | efc data | 58503A 3710A01056 |
| `:DIAGnostic:SLOG?` | log oldest | 58503A 3710A01056 |

## Every command

One row per logical operation.  A blank cell means that tree has
no spelling for it, and the library returns `Unsupported` without
anything reaching the receiver.

| Operation | Class | 58503A/B, 59551A | Z3801A, Z3816A |
| --------- | ----- | ---------------- | -------------- |
| idn | Query | `*IDN?` H | `*IDN?` F |
| cls | Control | `*CLS` M | `*CLS` F |
| selftest | Control | `*TST?` M | `*TST?` F |
| ese set | Control | `*ESE` M |  |
| ese | Query | `*ESE?` H | `*ESE?` F |
| esr | Query | `*ESR?` H | `*ESR?` F |
| sre set | Control | `*SRE` M |  |
| sre | Query | `*SRE?` H | `*SRE?` F |
| stb | Query | `*STB?` H | `*STB?` F |
| position avg | Query | `:GPS:POSition?` H | `:PTIMe:GPSystem:POSition?` F |
| position actual | Query | `:GPS:POSition:ACTual?` H |  |
| position set | Control | `:GPS:POSition` M |  |
| position hold last | Query | `:GPS:POSition:HOLD:LAST?` H | `:PTIMe:GPSystem:POSition:HOLD:LAST?` F |
| position hold state | Query | `:GPS:POSition:HOLD:STATe?` H |  |
| survey progress | Query | `:GPS:POSition:SURVey:PROGress?` H | `:PTIMe:GPSystem:POSition:SURVey:PROGress?` F |
| survey state | Query | `:GPS:POSition:SURVey:STATe?` H | `:PTIMe:GPSystem:POSition:SURVey:STATe?` F |
| survey once | Control | `:GPS:POSition:SURVey:STATe ONCE` M |  |
| survey powerup | Query | `:GPS:POSition:SURVey:STATe:POWerup?` H |  |
| survey powerup set | Control | `:GPS:POSition:SURVey:STATe:POWerup` M |  |
| elevation mask | Query | `:GPS:SATellite:TRACking:EMANgle?` H | `:PTIMe:GPSystem:EMANgle?` F |
| elevation mask set | Control | `:GPS:SATellite:TRACking:EMANgle` M |  |
| sat ignore | Query | `:GPS:SATellite:TRACking:IGNore?` H | `:PTIMe:GPSystem:SATellite:TRACking:IGNore?` F |
| sat ignore set | Control | `:GPS:SATellite:TRACking:IGNore` M |  |
| sat ignore count | Query | `:GPS:SATellite:TRACking:IGNore:COUNt?` H |  |
| sat include | Query | `:GPS:SATellite:TRACking:INCLude?` H | `:PTIMe:GPSystem:SATellite:TRACking:INCLude?` F |
| sat include set | Control | `:GPS:SATellite:TRACking:INCLude` M |  |
| antenna delay | Query | `:GPS:REFerence:ADELay?` H | `:PTIMe:GPSystem:ADELay?` F |
| antenna delay set | Control | `:GPS:REFerence:ADELay` M |  |
| time valid | Query | `:GPS:REFerence:VALid?` H |  |
| sat tracking | Query | `:GPS:SATellite:TRACking?` H | `:PTIMe:GPSystem:SATellite:TRACking?` F |
| sat tracking count | Query | `:GPS:SATellite:TRACking:COUNt?` H | `:PTIMe:GPSystem:SATellite:TRACking:COUNt?` F |
| sat visible | Query | `:GPS:SATellite:VISible:PREDicted?` H | `:PTIMe:GPSystem:SATellite:VISible:PREDicted?` F |
| sat visible count | Query | `:GPS:SATellite:VISible:PREDicted:COUNt?` H | `:PTIMe:GPSystem:SATellite:VISible:PREDicted:COUNt?` F |
| initial date set | Control | `:GPS:INITial:DATE` M |  |
| initial time set | Control | `:GPS:INITial:TIME` M |  |
| initial pos set | Control | `:GPS:INITial:POSition` M |  |
| sync state | Query | `:SYNChronization:STATe?` H | `:ROSCillator:STATe?` F |
| efc | Query | `:DIAGnostic:ROSCillator:EFControl:RELative?` H | `:DIAGnostic:ROSCillator:EFControl:RELative?` F |
| led gpslock | Query | `:LED:GPSLock?` H | `:LED:GPSLock?` F |
| led holdover | Query | `:LED:HOLDover?` H | `:LED:HOLDover?` F |
| ffom | Query | `:SYNChronization:FFOMerit?` H | `:PTIMe:FFOMerit?` F |
| tfom | Query | `:SYNChronization:TFOMerit?` H |  |
| tinterval | Query | `:SYNChronization:TINTerval?` H | `:PTIMe:TINTerval?` F |
| holdover unc pred | Query | `:SYNChronization:HOLDover:TUNCertainty:PREDicted?` H | `:ROSCillator:HOLDover:TUNCertainty:PREDicted?` F |
| holdover unc now | Query | `:SYNChronization:HOLDover:TUNCertainty:PRESent?` H | `:ROSCillator:HOLDover:TUNCertainty:PRESent?` F |
| holdover duration | Query | `:SYNChronization:HOLDover:DURation?` H | `:ROSCillator:HOLDover:DURation?` F |
| holdover thresh | Query | `:SYNChronization:HOLDover:DURation:THReshold?` H | `:ROSCillator:HOLDover:DURation:THReshold?` F |
| holdover thresh set | Control | `:SYNChronization:HOLDover:DURation:THReshold` M |  |
| holdover exceeded | Query | `:SYNChronization:HOLDover:DURation:THReshold:EXCeeded?` H | `:ROSCillator:HOLDover:DURation:THReshold:EXCeeded?` F |
| holdover waiting | Query | `:SYNChronization:HOLDover:WAITing?` H | `:ROSCillator:HOLDover:WAITing?` F |
| holdover initiate | Control | `:SYNChronization:HOLDover:INITiate` M | `:ROSCillator:HOLDover:INITiate` F |
| holdover recover | Control | `:SYNChronization:HOLDover:RECovery:INITiate` M | `:ROSCillator:HOLDover:RECovery:INITiate` F |
| holdover limit ignore | Control | `:SYNChronization:HOLDover:RECovery:LIMit:IGNore` M | `:ROSCillator:HOLDover:RECovery:LIMit:IGNore` F |
| sync immediate | Control | `:SYNChronization:IMMediate` M | `:PTIMe:SYNChronization:IMMediate` F |
| status screen | Query | `:SYSTem:STATus?` H | `:SYSTem:STATus?` F |
| status screen lines | Query | `:SYSTem:STATus:LENGth?` H |  |
| error | Query | `:SYSTem:ERRor?` H | `:SYSTem:ERRor?` F |
| log read all | Query | `:DIAGnostic:LOG:READ:ALL?` H |  |
| log count | Query | `:DIAGnostic:LOG:COUNt?` H | `:DIAGnostic:LOG:COUNt?` F |
| log read | Query | `:DIAGnostic:LOG:READ?` H | `:DIAGnostic:LOG:READ?` F |
| log clear | Control | `:DIAGnostic:LOG:CLEar` M | `:DIAGnostic:LOG:CLEar` F |
| led alarm | Query | `:LED:ALARm?` H | `:LED:ALARm?` F |
| status preset alarm | Control | `:STATus:PRESet:ALARm` M | `:STATus:PRESet:ALARm` F |
| oper condition | Query | `:STATus:OPERation:CONDition?` H | `:STATus:OPERation:CONDition?` F |
| oper event | Query | `:STATus:OPERation:EVENt?` H |  |
| hardware condition | Query | `:STATus:OPERation:HARDware:CONDition?` H | `:STATus:OPERation:HARDware:CONDition?` F |
| hardware event | Query | `:STATus:OPERation:HARDware:EVENt?` H |  |
| holdover condition | Query | `:STATus:OPERation:HOLDover:CONDition?` H | `:STATus:OPERation:HOLDover:CONDition?` F |
| holdover event | Query | `:STATus:OPERation:HOLDover:EVENt?` H |  |
| powerup condition | Query | `:STATus:OPERation:POWerup:CONDition?` H | `:STATus:OPERation:POWerup:CONDition?` F |
| quest condition | Query | `:STATus:QUEStionable:CONDition?` H | `:STATus:QUEStionable:CONDition?` F |
| quest event | Query | `:STATus:QUEStionable:EVENt?` H |  |
| lifetime count | Query | `:DIAGnostic:LIFetime:COUNt?` H | `:DIAGnostic:LIFetime:COUNt?` F |
| diag test | Control | `:DIAGnostic:TEST?` M |  |
| diag test result | Query | `:DIAGnostic:TEST:RESult?` H |  |
| query response | Query | `:DIAGnostic:QUERy:RESPonse?` H | `:DIAGnostic:QUERy:RESPonse?` F |
| timecode | Query | `:PTIMe:TCODe?` H | `:PTIMe:TCODe?` F |
| timecode format | Query | `:PTIMe:TCODe:FORMat?` H | `:PTIMe:TCODe:FORMat?` F |
| timecode format set | Control | `:PTIMe:TCODe:FORMat` M |  |
| date | Query | `:PTIMe:DATE?` H |  |
| time | Query | `:PTIMe:TIME?` H |  |
| time string | Query | `:PTIMe:TIME:STRing?` H |  |
| tzone | Query | `:PTIMe:TZONe?` H |  |
| tzone set | Control | `:PTIMe:TZONe` M |  |
| leap accumulated | Query | `:PTIMe:LEAPsecond:ACCumulated?` H | `:PTIMe:LEAPsecond:ACCumulated?` F |
| leap date | Query | `:PTIMe:LEAPsecond:DATE?` H |  |
| leap duration | Query | `:PTIMe:LEAPsecond:DURation?` H |  |
| leap state | Query | `:PTIMe:LEAPsecond:STATe?` H |  |
| comm settings | Query | `:SYSTem:COMMunicate?` H |  |
| comm baud | Query | `:SYSTem:COMMunicate:SERial1:BAUD?` H |  |
| comm baud set | Dangerous | `:SYSTem:COMMunicate:SERial1:BAUD` M |  |
| comm parity | Query | `:SYSTem:COMMunicate:SERial1:PARity?` H |  |
| comm parity set | Dangerous | `:SYSTem:COMMunicate:SERial1:PARity` M |  |
| comm pace | Query | `:SYSTem:COMMunicate:SERial1:PACE?` H |  |
| comm pace set | Dangerous | `:SYSTem:COMMunicate:SERial1:PACE` M |  |
| comm fduplex | Query | `:SYSTem:COMMunicate:SERial1:FDUPlex?` H |  |
| comm fduplex set | Dangerous | `:SYSTem:COMMunicate:SERial1:FDUPlex` M |  |
| comm preset | Dangerous | `:SYSTem:COMMunicate:SERial1:PRESet` M |  |
| system preset | Dangerous | `:SYSTem:PRESet` M | `:SYSTem:PRESet` F |
| language | Query | `:SYSTem:LANGuage?` H | `:SYSTem:LANGuage?` F |
| language set | Dangerous | `:SYSTem:LANGuage` M | `:SYSTem:LANGuage` F |
| flash erase | Dangerous | `:DIAGnostic:ERASe` M | `:DIAGnostic:ERASe` M |
| temperature | Query | `:DIAGnostic:TEMPerature?` H (58503A) |  |
| oven current | Query | `:DIAGnostic:ROSCillator:CURRent?` H (58503A) |  |
| efc absolute | Query | `:DIAGnostic:ROSCillator:EFControl:ABSolute?` H (58503A) |  |
| oven tempco | Query | `:DIAGnostic:ROSCillator:TCOefficient?` H (58503A) |  |
| gps engine identity | Query | `:DIAGnostic:IDENtification:GPSystem?` H (58503A) |  |
| model identity | Query | `:DIAGnostic:IDENtification:DEFault?` H (58503A) |  |
| gps engine time | Query | `:DIAGnostic:GPSystem:TIME?` H (58503A) |  |
| gps engine utc | Query | `:DIAGnostic:GPSystem:UTC?` H (58503A) |  |
| time offset | Query | `:DIAGnostic:TOFFset?` H (58503A) |  |
| efc data | Query | `:DIAGnostic:ROSCillator:EFControl:DATA?` H (58503A) |  |
| log oldest | Query | `:DIAGnostic:SLOG?` H (58503A) |  |
