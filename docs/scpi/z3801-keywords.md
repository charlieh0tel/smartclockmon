# Z3801A SCPI keyword table

The receiver's own vocabulary, read from `third_party/firmware/z3801a-3543.bin`;
it settles spelling questions the manual leaves open.

## Encoding

Each keyword is stored as two consecutive NUL-terminated strings: the
mandatory short form, then the optional tail.  `ROSC\0ILLATOR\0` is
`ROSCillator`; a keyword with no optional part has an empty second
string, so `SERIAL2\0\0` is just `SERIAL2`.  This is the SCPI
convention of uppercase-required and lowercase-optional, stored as a
split rather than as case.

So plain `strings` shows fragments -- `ROSC`, `TINT`, `PTIM`, `ESHOLD`
-- and the long forms as separate second halves.

Regenerate by locating `ROSC\0ILLATOR\0`, walking back over
NUL-terminated uppercase words to the start of the run, then reading
pairs forward.  Pairing from the wrong offset yields plausible
nonsense like `ESHOLDstat`.

## Against the command table

All but one of the 82 Z3801 entries in `commands.toml` have every
keyword present here.  The 63 marked `evidence = "firmware"` rest on
this table alone.  The 18 marked `evidence = "hardware"` were found on
a Z3805A (3625A01487, firmware 3543B-A) and cite it; their keywords --
`TFOMerit`, `TEMPerature`, `TCOefficient`, `CURRent`, `ABSolute`,
`SLOG`, `LEAPsecond`, `TZONe`, `STRing`, `LENGth`, `GPSystem` -- are
all here too.

The exception is `:DIAGnostic:ERASe`, which belongs to the INSTALL
language used for firmware download, not the PRIMARY language this
table serves.  It stays `evidence = "manual"`.

## The tree

The image also holds the tree, readable without the parser code, in
two structures.  Pointers are absolute and the image is not relocated,
so a stored pointer is a file offset as it stands.

A **node** lives in `0x57000`..`0x5e000`:

    +0   u32   pointer to the keyword pair
    +4   u32   pointer to the child list, zero for a leaf

A **child list** lives in `0x52000`..`0x53100`:

    +0   u16   an id
    +2   u16   how many children
    +4   u16   flags; 0xffff is common
    +6   u32 x count   pointers to child nodes

Walking `:SYSTem:` gives `COMMunicate` -- itself the parent of `SER`,
`SER1`, `SER2`, `SERIAL`, `SERIAL1` and `SERIAL2` -- then `DATE`,
`ERRor`, `LANGuage`, `PRESet`, `PRINt:LENGth`, `STATus:LENGth` and
`TIME`.

Five of those, put to a Z3805A, exist: `:SYSTem:PRINt:LENGth?`
answered `+23`, `:SYSTem:LANGuage?` answered `"PRIMARY"`,
`:SYSTem:DATE?` and `:SYSTem:TIME?` were recognized and declined with
-230 for want of a fix, and `:SYSTem:COMMunicate:SERIAL2:BAUD?`
answered `+9600` -- a second serial port, at a different rate from
the first, which neither manual mentions.

The whole tree, 595 paths, is `z3801a-3543.txt`, as
`smartclock-cli dump-scpi` reads it (`README.md`).  Three points
of method: only a child list that some node's `+4` points at is real,
since searching the region for any window containing a target finds
overlapping sub-arrays; a walk must keep nodes that are both a command
and a parent, such as `:PTIMe:GPSystem:POSition`, not only leaves; and
it must follow every parent of a shared list, such as `SER`, `SER1`
and `SERIAL`, not only the first.

`:SOURce` is an optional header: `PTIMe`, `PULSe`, `ROSCillator` and
`SYNChronization` all hang beneath it.  Both forms answer --
`:PTIMe:FFOMerit?` and `:SOURce:PTIMe:FFOMerit?` each returned `+3`
from a Z3805A.

## Of note

`RAIM`, `TCOefficient`, `HYSTeresis`, `GCORrection`, `TMHValid` and
`TOFFset` appear in neither manual, nor do `GARY`, `DOUGlas`,
`KENneth`, `ROBin` or the four-letter entries (`R1PO`, `RACD`, `RAST`,
`WTZO` and the rest).

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
