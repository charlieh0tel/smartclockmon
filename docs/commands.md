# Command matrix

Generated from `crates/smartclock/commands.toml`.  Run `make docs`
to regenerate; a test fails if this file and the table disagree.

## What is in the table

| Tree | Commands | Hardware | Firmware | Manual |
| ---- | -------- | -------- | -------- | ------ |
| 58503A/B, 59551A | 130 | 92 | 0 | 38 |
| Z3801A, Z3816A | 83 | 19 | 63 | 1 |

**H** means the receiver answered it.  **F** means every keyword
appears in the firmware's own keyword table, so the spelling is
right though no such receiver has been on the line.  **M** means
it was transcribed from a manual and nothing more.

| Class | Commands | Gate |
| ----- | -------- | ---- |
| Query | 91 | none |
| Control | 31 | `--allow-control` |
| Dangerous | 8 | `--allow-dangerous` |

## Undocumented

12 commands appear in none of the manuals here.  They were found
by building candidate paths from the firmware's keyword table and
sending them to a receiver: an unknown header returns -113 and
changes nothing, so a sweep is safe and settles the question.

| Command | Operation | Found on |
| ------- | --------- | -------- |
| `:DIAGnostic:PTIMe:TINTerval?` | tinterval reading | a 58503A |
| `:DIAGnostic:TEMPerature?` | temperature | a 58503A |
| `:DIAGnostic:ROSCillator:CURRent?` | oven current | a 58503A |
| `:DIAGnostic:ROSCillator:EFControl:ABSolute?` | efc absolute | a 58503A |
| `:DIAGnostic:ROSCillator:TCOefficient?` | oven tempco | a 58503A |
| `:DIAGnostic:IDENtification:GPSystem?` | gps engine identity | a 58503A |
| `:DIAGnostic:IDENtification:DEFault?` | model identity | a 58503A |
| `:DIAGnostic:GPSystem:TIME?` | gps engine time | a 58503A |
| `:DIAGnostic:GPSystem:UTC?` | gps engine utc | a 58503A |
| `:DIAGnostic:TOFFset?` | time offset | a 58503A |
| `:DIAGnostic:ROSCillator:EFControl:DATA?` | efc data | a 58503A |
| `:DIAGnostic:SLOG?` | log oldest | a 58503A |

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
| esr | Control | `*ESR?` H | `*ESR?` F |
| sre set | Control | `*SRE` M |  |
| sre | Query | `*SRE?` H | `*SRE?` F |
| stb | Query | `*STB?` H | `*STB?` F |
| position avg | Query | `:GPS:POSition?` H | `:PTIMe:GPSystem:POSition?` F |
| position actual | Query | `:GPS:POSition:ACTual?` H | `:PTIMe:GPSystem:POSition:ACTual?` F |
| position set | Control | `:GPS:POSition` M | `:PTIMe:GPSystem:POSition` F |
| position hold last | Query | `:GPS:POSition:HOLD:LAST?` H | `:PTIMe:GPSystem:POSition:HOLD:LAST?` F |
| position hold state | Query | `:GPS:POSition:HOLD:STATe?` H | `:PTIMe:GPSystem:POSition:HOLD:STATe?` H |
| survey progress | Query | `:GPS:POSition:SURVey:PROGress?` H | `:PTIMe:GPSystem:POSition:SURVey:PROGress?` F |
| survey state | Query | `:GPS:POSition:SURVey:STATe?` H | `:PTIMe:GPSystem:POSition:SURVey:STATe?` F |
| survey once | Control | `:GPS:POSition:SURVey:STATe ONCE` M | `:PTIMe:GPSystem:POSition:SURVey:STATe ONCE` F |
| survey powerup | Query | `:GPS:POSition:SURVey:STATe:POWerup?` H | `:PTIMe:GPSystem:POSition:SURVey:STATe:POWerup?` H |
| survey powerup set | Control | `:GPS:POSition:SURVey:STATe:POWerup` M | `:PTIMe:GPSystem:POSition:SURVey:STATe:POWerup` F |
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
| initial date set | Control | `:GPS:INITial:DATE` M | `:PTIMe:GPSystem:INITial:DATE` F |
| initial time set | Control | `:GPS:INITial:TIME` M | `:PTIMe:GPSystem:INITial:TIME` F |
| initial pos set | Control | `:GPS:INITial:POSition` M | `:PTIMe:GPSystem:INITial:POSition` F |
| sync state | Query | `:SYNChronization:STATe?` H | `:ROSCillator:STATe?` F |
| efc | Query | `:DIAGnostic:ROSCillator:EFControl:RELative?` H | `:DIAGnostic:ROSCillator:EFControl:RELative?` F |
| led gpslock | Query | `:LED:GPSLock?` H | `:LED:GPSLock?` F |
| led holdover | Query | `:LED:HOLDover?` H | `:LED:HOLDover?` F |
| ffom | Query | `:SYNChronization:FFOMerit?` H | `:PTIMe:FFOMerit?` F |
| tfom | Query | `:SYNChronization:TFOMerit?` H | `:PTIMe:TFOMerit?` H |
| tinterval | Query | `:SYNChronization:TINTerval?` H | `:PTIMe:TINTerval?` F |
| tinterval reading | Query | `:DIAGnostic:PTIMe:TINTerval?` H (58503A) | `:DIAGnostic:PTIMe:TINTerval?` H |
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
| status screen lines | Query | `:SYSTem:STATus:LENGth?` H | `:SYSTem:STATus:LENGth?` H |
| error | Control | `:SYSTem:ERRor?` H | `:SYSTem:ERRor?` F |
| log read all | Query | `:DIAGnostic:LOG:READ:ALL?` H | `:DIAGnostic:LOG:READ:ALL?` H |
| log count | Query | `:DIAGnostic:LOG:COUNt?` H | `:DIAGnostic:LOG:COUNt?` F |
| log read | Query | `:DIAGnostic:LOG:READ?` H | `:DIAGnostic:LOG:READ?` F |
| log clear | Control | `:DIAGnostic:LOG:CLEar` M | `:DIAGnostic:LOG:CLEar` F |
| led alarm | Query | `:LED:ALARm?` H | `:LED:ALARm?` F |
| status preset alarm | Control | `:STATus:PRESet:ALARm` M | `:STATus:PRESet:ALARm` F |
| oper condition | Query | `:STATus:OPERation:CONDition?` H | `:STATus:OPERation:CONDition?` F |
| oper event | Control | `:STATus:OPERation:EVENt?` H |  |
| hardware condition | Query | `:STATus:OPERation:HARDware:CONDition?` H | `:STATus:OPERation:HARDware:CONDition?` F |
| hardware event | Control | `:STATus:OPERation:HARDware:EVENt?` H |  |
| holdover condition | Query | `:STATus:OPERation:HOLDover:CONDition?` H | `:STATus:OPERation:HOLDover:CONDition?` F |
| holdover event | Control | `:STATus:OPERation:HOLDover:EVENt?` H |  |
| powerup condition | Query | `:STATus:OPERation:POWerup:CONDition?` H | `:STATus:OPERation:POWerup:CONDition?` F |
| quest condition | Query | `:STATus:QUEStionable:CONDition?` H | `:STATus:QUEStionable:CONDition?` F |
| quest event | Control | `:STATus:QUEStionable:EVENt?` H |  |
| powerup event | Control | `:STATus:OPERation:POWerup:EVENt?` M |  |
| oper positive transition | Query | `:STATus:OPERation:PTRansition?` H |  |
| oper negative transition | Query | `:STATus:OPERation:NTRansition?` H |  |
| quest positive transition | Query | `:STATus:QUEStionable:PTRansition?` H |  |
| quest negative transition | Query | `:STATus:QUEStionable:NTRansition?` H |  |
| hardware positive transition | Query | `:STATus:OPERation:HARDware:PTRansition?` H |  |
| hardware negative transition | Query | `:STATus:OPERation:HARDware:NTRansition?` H |  |
| holdover positive transition | Query | `:STATus:OPERation:HOLDover:PTRansition?` H |  |
| holdover negative transition | Query | `:STATus:OPERation:HOLDover:NTRansition?` H |  |
| powerup positive transition | Query | `:STATus:OPERation:POWerup:PTRansition?` H |  |
| powerup negative transition | Query | `:STATus:OPERation:POWerup:NTRansition?` H |  |
| oper enable | Query | `:STATus:OPERation:ENABle?` M |  |
| hardware enable | Query | `:STATus:OPERation:HARDware:ENABle?` M |  |
| holdover enable | Query | `:STATus:OPERation:HOLDover:ENABle?` M |  |
| powerup enable | Query | `:STATus:OPERation:POWerup:ENABle?` M |  |
| quest enable | Query | `:STATus:QUEStionable:ENABle?` M |  |
| lifetime count | Query | `:DIAGnostic:LIFetime:COUNt?` H | `:DIAGnostic:LIFetime:COUNt?` F |
| diag test | Control | `:DIAGnostic:TEST?` M |  |
| diag test result | Query | `:DIAGnostic:TEST:RESult?` H |  |
| query response | Query | `:DIAGnostic:QUERy:RESPonse?` H | `:DIAGnostic:QUERy:RESPonse?` F |
| timecode | Query | `:PTIMe:TCODe?` H | `:PTIMe:TCODe?` F |
| timecode format | Query | `:PTIMe:TCODe:FORMat?` H | `:PTIMe:TCODe:FORMat?` F |
| timecode format set | Control | `:PTIMe:TCODe:FORMat` M |  |
| date | Query | `:PTIMe:DATE?` H | `:PTIMe:DATE?` H |
| time | Query | `:PTIMe:TIME?` H | `:PTIMe:TIME?` H |
| time string | Query | `:PTIMe:TIME:STRing?` H | `:PTIMe:TIME:STRing?` H |
| tzone | Query | `:PTIMe:TZONe?` H | `:PTIMe:TZONe?` H |
| tzone set | Control | `:PTIMe:TZONe` M |  |
| leap accumulated | Query | `:PTIMe:LEAPsecond:ACCumulated?` H | `:PTIMe:LEAPsecond:ACCumulated?` F |
| leap date | Query | `:PTIMe:LEAPsecond:DATE?` H | `:PTIMe:LEAPsecond:DATE?` H |
| leap duration | Query | `:PTIMe:LEAPsecond:DURation?` H | `:PTIMe:LEAPsecond:DURation?` H |
| leap state | Query | `:PTIMe:LEAPsecond:STATe?` H | `:PTIMe:LEAPsecond:STATe?` H |
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
| temperature | Query | `:DIAGnostic:TEMPerature?` H (58503A) | `:DIAGnostic:TEMPerature?` H |
| oven current | Query | `:DIAGnostic:ROSCillator:CURRent?` H (58503A) | `:DIAGnostic:ROSCillator:CURRent?` H |
| efc absolute | Query | `:DIAGnostic:ROSCillator:EFControl:ABSolute?` H (58503A) | `:DIAGnostic:ROSCillator:EFControl:ABSolute?` H |
| oven tempco | Query | `:DIAGnostic:ROSCillator:TCOefficient?` H (58503A) | `:DIAGnostic:ROSCillator:TCOefficient?` H |
| gps engine identity | Query | `:DIAGnostic:IDENtification:GPSystem?` H (58503A) | `:DIAGnostic:IDENtification:GPSystem?` H |
| model identity | Query | `:DIAGnostic:IDENtification:DEFault?` H (58503A) |  |
| gps engine time | Query | `:DIAGnostic:GPSystem:TIME?` H (58503A) |  |
| gps engine utc | Query | `:DIAGnostic:GPSystem:UTC?` H (58503A) |  |
| time offset | Query | `:DIAGnostic:TOFFset?` H (58503A) |  |
| efc data | Query | `:DIAGnostic:ROSCillator:EFControl:DATA?` H (58503A) |  |
| log oldest | Query | `:DIAGnostic:SLOG?` H (58503A) | `:DIAGnostic:SLOG?` H |
| system pon | Dangerous |  | `:SYSTem:PON` F (Z3816A) |
