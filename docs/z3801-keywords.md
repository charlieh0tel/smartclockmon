# Z3801A SCPI keyword table

Extracted from `the keyword table`.  This is the receiver's own
vocabulary, so it settles spelling questions the manual leaves open.

## Encoding

Each keyword is stored as two consecutive NUL-terminated strings: the
mandatory short form, then the optional tail.  `ROSC\0ILLATOR\0` is
`ROSCillator`; a keyword with no optional part has an empty second
string, so `SERIAL2\0\0` is just `SERIAL2`.  That is exactly the SCPI
convention of uppercase-required and lowercase-optional, stored as a
split rather than as case.

This is why plain `strings` only ever shows fragments -- `ROSC`, `TINT`,
`PTIM`, `ESHOLD` -- and why the long forms appear to be missing.  They
are not: they are the second half of each pair.

Regenerate by locating `ROSC\0ILLATOR\0`, walking back over
NUL-terminated uppercase words to the start of the run, then reading
pairs forward.  The parity matters: pairing from the wrong offset yields
plausible nonsense like `ESHOLDstat`.

## What it settled

All but one of the 56 Z3801A entries in `commands.toml` have every
keyword present here, so their spelling is confirmed even though no such
receiver has been on the line.  Those are marked `evidence = "firmware"`.

The exception is `:DIAGnostic:ERASe`, which belongs to the INSTALL
language used for firmware download, not the PRIMARY language this image
serves.  It stays `evidence = "manual"`.

The table gives the vocabulary, not the tree: which keyword nests under
which would need the parser code, not its strings.

## Of note

`RAIM`, `TCOefficient`, `HYSTeresis`, `GCORrection`, `TMHValid` and
`TOFFset` appear in neither manual.  `GARY`, `DOUGlas`, `KENneth` and
`ROBin` are presumably developer commands.  The four-letter entries
(`R1PO`, `RACD`, `RAST`, `WTZO` and friends) look like an internal or
factory command set.

## Keywords

    ABSolute            ACCumulated         ACTion              ACTive
    ACTual              ADDRess             ADELay              ALAR1
    ALAR3               ALARM1              ALARM3              ALARm
    ALIGnment           ALL                 ASCii               AUTO
    BARR                BARR1               BARR2               BARRAY
    BARRAY1             BARRAY2             BAUD                BITS
    BOOLean             CALA                CALCulate           CEQU
    CLEar               CLS                 COMMunicate         CONDition
    CONTinuous          COUNt               CURRent             DANA
    DARR                DARR1               DARR2               DARR3
    DARRAY              DARRAY1             DARRAY2             DARRAY3
    DATA                DATE                DEFault             DEGRees
    DELay               DIAGnostic          DISPlay             DOUBle
    DOUGlas             DOUTput             DURation            E
    EDGE                EEPRom              EFControl           EGResponse
    EMANgle             ENABle              ENABled             ERRor
    ESE                 ESR                 EVEN                EVENt
    EXCeeded            F1                  F2                  FALLing
    FARR                FARR1               FARR2               FARR3
    FARRAY              FARRAY1             FARRAY2             FARRAY3
    FDUPlex             FFOMerit            FIRSt               FLOat
    FMHO                FORMat              FPGA                FREQuency
    GARY                GCORrection         GPS                 GPSLock
    GPSTime             GPSystem            HAPPening           HARDware
    HEIGht              HOLD                HOLDing             HOLDover
    HPOSition           HYSTeresis          IARR                IARR1
    IARR2               IARR3               IARR4               IARRAY
    IARRAY1             IARRAY2             IARRAY3             IARRAY4
    IDENtification      IDN                 IGNore              IMMediate
    IMRC                IMULtiple           INCLude             INFinity
    INITial             INITiate            INTeger             INTerpolator
    IPSU                IREFerence          KENneth             LANGuage
    LAST                LATitude            LEAP                LEAPsecond
    LED                 LENGth              LIFetime            LIMit
    LOCKed              LOG                 LONGitude           LONGitutde
    MAJor               MANGle              MAXimum             ME
    MEASurement         MEMory              MINimum             MINor
    MODel               MSEConds            N                   NEGative
    NONE                NTRansition         ODD                 ONCE
    ONE                 OPC                 OPERation           OS
    OTHer               OUTPut              OVERflow            PACE
    PARity              PERiod              PIN1                PIN2
    PIN3                PIN6                PIN7                PIN8
    PINS                POINts              POSition            POSitive
    POWer               POWerup             PPS                 PREDicted
    PRESent             PRESet              PRINt               PROCess
    PROCessor           PROGress            PROMpt              PSTartup
    PTIMe               PTRansition         PULSe               QMRC
    QSPI                QUERy               QUEStionable        R1PO
    RACD                RAIM                RAM                 RAST
    RAT1                RAT2                RAT3                READ
    REAL                RECeive             RECovering          RECovery
    REFerence           RELative            REQU                RESPonse
    RESTricted          RESult              RFOU                RISing
    RLOC                RMHO                ROBin               ROSCillator
    RPHS                RSPR                RSST                RSSU
    RST                 RTAD                RTCM                RTSA
    RTZO                RVST                RWHO                S
    SAMPle              SATellite           SAVE                SBITs
    SENSe               SER                 SER1                SER2
    SERIAL              SERIAL1             SERIAL2             SERial
    SET                 SLOG                SOURce              SRE
    STACk               STARt               STATe               STATus
    STB                 STRing              SURVey              SYNChronization
    SYSTem              T0                  T1                  TARR
    TARR1               TARR2               TARR3               TARRAY
    TARRAY1             TARRAY2             TARRAY3             TCODe
    TCOefficient        TEMPerature         TEST                TFOMerit
    THReshold           TIME                TINTerval           TKN
    TMHValid            TOFFset             TOGGle              TRACking
    TRANsmit            TST                 TST1                TST2
    TST3                TST4                TSTAMP              TSTAMP1
    TSTAMP2             TSTAMP3             TSTAMP4             TSTamp
    TUNCertainty        TYPE                TZONe               UART
    USER                UTC                 VALid               VISible
    W                   W1PO                WACD                WAITing
    WAT1                WAT2                WAT3                WEAL
    WFOU                WLOC                WRITe               WTZO
    XON

## Commands found by sweeping a 58503A

The keyword table gives the vocabulary but not the tree.  Building
candidate paths from it and sending them to the receiver settles the
rest: an unknown header returns -113 and changes nothing, so a sweep is
safe and definitive.  1,530 candidates over the `:DIAGnostic` subtree
found fifteen commands, none of which appear in any manual here.

| Command | Reading on 3710A01056 |
| ------- | --------------------- |
| `:DIAGnostic:TEMPerature?` | `+3.68550E+001`, degrees Celsius |
| `:DIAGnostic:ROSCillator:CURRent?` | `+1.05882E+002`, oven current |
| `:DIAGnostic:ROSCillator:TCOefficient?` | `-3.36500E+001`, learned tempco |
| `:DIAGnostic:ROSCillator:EFControl:ABSolute?` | `+713392`, DAC code |
| `:DIAGnostic:ROSCillator:EFControl:DATA?` | `+0` |
| `:DIAGnostic:ROSCillator:EFControl?` | same as `:RELative?` |
| `:DIAGnostic:IDENtification:GPSystem?` | the GPS engine, also as `:GPS?` |
| `:DIAGnostic:IDENtification:DEFault?` | `"58503A","CQ"` |
| `:DIAGnostic:GPSystem:TIME?` | `+21,+41,+53,+2007,+2,+4`, time then date |
| `:DIAGnostic:GPSystem:UTC?` | `1` |
| `:DIAGnostic:TOFFset?` | `+0.00000E+000` |
| `:DIAGnostic:SLOG?` | oldest log entry, against `:LOG?` for the newest |

### The EFC value is twenty bits

`ABSolute` and `RELative` are the same quantity in different units:

    relative percent = code / 2^20 * 200 - 100

Code 713392 gives 36.0687, which is exactly what `:RELative?` returned
at the same moment, so the value spans twenty bits and `RELative` runs
-100 to +100 across it.

Tom Van Baak reached the same conclusion for the Z3801A by other means,
and went further into the hardware:
<http://www.leapsecond.com/pages/z3801a-efc/>.  The value is a 20-bit
integer, but the converter is a 16-bit AD569 updated at 102.4 Hz whose
low-order bits are dithered to interpolate the remaining four, with a
second-order low-pass filter smoothing the result.  So "20-bit DAC"
would be wrong: 20-bit value, 16-bit DAC, 4 bits of dither.

### How much frequency one EFC unit buys is unknown here

That page measures 5.2e-13 of output frequency per EFC unit on a
Z3801A, putting its full span near 5.5e-7.

Whether the 58503A matches is open.  Both use an HP 10811: the Z3801A
manual names it, and the development unit carries a 10811-60159.  The
same oscillator argues for a similar span, but nothing here measures it.

The one datum on this unit is an external reading of about 52 mV on the
control voltage while `RELative` read 36.06 percent.  Taken as a
deviation from centre that puts full scale near plus or minus 144 mV,
which would need an EFC sensitivity around 2e-6 per volt to cover the
Z3801A's span.  Whether that is the right sensitivity for a 10811, and
whether 52 mV was measured from centre or to ground, are both unsettled,
so nothing should be concluded from it yet.

Settling it needs the same experiment Van Baak ran, on this unit: log
`EFC:ABSolute?` against a counter while the receiver corrects itself out
of a long holdover.  Until then the library reports the raw value and
the percentage the receiver itself gives, and claims nothing about
frequency.

### Why the date is wrong

`:DIAGnostic:IDENtification:GPSystem?` names the GPS engine: a Motorola
with `SOFTWARE DATE 06 Aug 1996`.  That firmware predates the 1024-week
rollovers of 1999 and 2019, which is the source of the receiver's
1024-week date error rather than anything in the 58503A itself.

### Not on the 58503A

The firmware strings include `Double oven`, but that is a Z3801A
feature.  The 58503A has a single-oven OCXO, so any double-oven field
belongs to the other model.
