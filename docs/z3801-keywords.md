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
