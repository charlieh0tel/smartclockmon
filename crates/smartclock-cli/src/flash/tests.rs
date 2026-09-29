use std::collections::VecDeque;
use std::net::TcpListener;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::ensure;
use smartclock::error::Error as SessionError;
use smartclock::session::Config;
use smartclock::session::Session;
use smartclock::transport::serial::Settings;
use smartclock_sim::installer::FlashLayout;
use smartclock_sim::installer::Installer;
use smartclock_sim::net::serve;
use smartclock_sim::receiver::Receiver;
use smartclock_sim::transport::SimTransport;

use super::Link;
use super::Mode;
use super::firmware::Firmware;
use super::firmware::ImageError;
use super::firmware::Layout;
use super::firmware::RECORD_SIZE;
use super::firmware::srecord;
use super::flash;
use super::open;

const PRIMARY_START: usize = Layout::AmdLanes.primary_start();

/// The receiver parses records independently of the flasher and exposes
/// its flash so the final comparison includes the protected boot bytes.
fn simulated(firmware: &Firmware, bytes: Vec<u8>, layout: FlashLayout) -> SimTransport {
    let mut receiver = Receiver::default();
    receiver.identity = format!(
        "HEWLETT-PACKARD,{},3542A01548,{}-A",
        firmware.profile.model, firmware.profile.revision
    );
    receiver.installer =
        Some(Installer::new(bytes, layout, firmware.profile.installer.into()).unwrap());
    SimTransport::new(receiver)
}

fn session(transport: SimTransport) -> Session<SimTransport> {
    let mut session = Session::new(
        transport,
        Config {
            idle: Duration::from_millis(1),
            ..Config::default()
        },
    );
    session.sync().unwrap();
    session
}

#[test]
fn simulator_reflashes_each_model_through_real_echoes_and_prompts() {
    for (name, layout) in [
        ("z3801a-3543.bin", FlashLayout::AmdLanes),
        ("z3805a-3543b.bin", FlashLayout::AmdLanes),
        ("58503a-3633.bin", FlashLayout::AmdLanes),
        ("58503a-3704.bin", FlashLayout::AmdLanes),
        ("z3816a-4001.bin", FlashLayout::IntelWords),
    ] {
        let bytes = std::fs::read(format!(
            "{}/../../third_party/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let firmware = Firmware::validate(bytes.clone()).unwrap();
        let transport = simulated(&firmware, bytes.clone(), layout);
        flash(
            &mut session(transport.clone()),
            &firmware,
            true,
            Some("3542A01548"),
        )
        .unwrap();
        let receiver = transport.receiver().lock().unwrap();
        let installer = receiver.installer.as_ref().unwrap();
        assert_eq!(installer.flash, bytes, "{name}");
        assert!(!installer.active, "{name} did not boot");
    }
}

#[test]
fn simulator_recovery_after_partial_programming_preserves_boot_flash() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let transport = simulated(&firmware, DUMP.to_vec(), FlashLayout::AmdLanes);
    transport
        .receiver()
        .lock()
        .unwrap()
        .installer
        .as_mut()
        .unwrap()
        .fail_at = Some(PRIMARY_START + RECORD_SIZE);
    let mut session = session(transport.clone());
    assert!(flash(&mut session, &firmware, true, Some("3542A01548")).is_err());
    {
        let mut receiver = transport.receiver().lock().unwrap();
        let installer = receiver.installer.as_mut().unwrap();
        assert!(installer.active);
        assert_eq!(&installer.flash[..PRIMARY_START], &DUMP[..PRIMARY_START]);
        assert_ne!(installer.flash, DUMP);
        installer.fail_at = None;
    }
    flash(&mut session, &firmware, true, Some("3542A01548")).unwrap();
    assert_eq!(
        transport
            .receiver()
            .lock()
            .unwrap()
            .installer
            .as_ref()
            .unwrap()
            .flash,
        DUMP
    );
}

#[test]
fn simulator_rejects_bad_records_and_protected_addresses() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let transport = simulated(&firmware, DUMP.to_vec(), FlashLayout::AmdLanes);
    let mut session = session(transport.clone());
    session.query(":SYSTem:LANGuage \"INSTALL\"").unwrap();
    assert!(
        session
            .query(":DIAGnostic:DOWNload S2060100001234B2")
            .is_err()
    );
    for record in ["S2060000001234B3", "S206010000123400", "S206FFFFFF1234B6"] {
        assert!(
            session
                .query(&format!(":DIAGnostic:DOWNload \"{record}\""))
                .is_err()
        );
    }
    assert_eq!(
        transport
            .receiver()
            .lock()
            .unwrap()
            .installer
            .as_ref()
            .unwrap()
            .flash,
        DUMP
    );
}

const DUMP: &[u8] = include_bytes!("../../../../third_party/z3801a-3543.bin");

/// Strict exchange script for early-exit and identity-change tests.
#[derive(Debug, Default)]
struct Script {
    replies: VecDeque<(String, String)>,
    sent: Vec<String>,
}

impl Script {
    fn add(&mut self, command: &str, reply: &str) {
        self.replies.push_back((command.into(), reply.into()));
    }
    fn start(model: &str, revision: &str, mode: &str) -> Self {
        let mut s = Self::default();
        s.add(
            "*IDN?",
            &format!("HEWLETT-PACKARD,{model},3542A01548,{revision}"),
        );
        s.add(":SYSTem:LANGuage?", mode);
        s.add(":SYSTem:ERRor?", "+0,\"No error\"");
        s
    }
}

impl Link for Script {
    fn command(&mut self, command: &str) -> Result<String> {
        self.sent.push(command.into());
        let (expected, reply) = self.replies.pop_front().context("unexpected command")?;
        ensure!(expected == command, "expected {expected}, got {command}");
        ensure!(reply != "FAIL", "simulated lost connection");
        Ok(reply)
    }
    fn settle(&mut self) -> Result<()> {
        Ok(())
    }
}

#[test]
fn validates_audited_images_and_rejects_corruption_and_chip_files() {
    Firmware::validate(DUMP.to_vec()).unwrap();
    let mut changed = DUMP.to_vec();
    changed[PRIMARY_START] ^= 1;
    assert!(matches!(
        Firmware::validate(changed),
        Err(ImageError::Unknown(_))
    ));
    assert!(matches!(
        Firmware::validate(DUMP[..PRIMARY_START].to_vec()),
        Err(ImageError::Size(_))
    ));
    for name in [
        "z3805a-3543b.bin",
        "58503a-3633.bin",
        "58503a-3704.bin",
        "z3816a-4001.bin",
    ] {
        let bytes = std::fs::read(format!(
            "{}/../../third_party/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        Firmware::validate(bytes).unwrap();
    }
}

#[test]
fn records_cover_only_primary_and_have_valid_counts_and_checksums() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let mut reconstructed = Vec::new();
    for (address, record) in firmware.records() {
        let bytes: Vec<u8> = (2..record.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&record[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(usize::from(bytes[0]), bytes.len() - 1);
        assert_eq!(bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b)), 255);
        assert_eq!(
            (usize::from(bytes[1]) << 16) | (usize::from(bytes[2]) << 8) | usize::from(bytes[3]),
            address
        );
        reconstructed.extend_from_slice(&bytes[4..bytes.len() - 1]);
    }
    assert_eq!(reconstructed, DUMP[PRIMARY_START..]);
    assert_eq!(
        srecord(Layout::AmdLanes, PRIMARY_START, &[0x12, 0x34]),
        "S2060100001234B2"
    );
}

#[test]
fn check_only_never_changes_language_or_flash() {
    let mut s = Script::start("Z3801A", "3543-A", "PRIMARY");
    flash(
        &mut s,
        &Firmware::validate(DUMP.to_vec()).unwrap(),
        false,
        None,
    )
    .unwrap();
    assert!(s.replies.is_empty());
    assert!(s.sent.iter().all(|c| c.ends_with('?')));
}

#[test]
fn mismatches_stop_before_installer_or_erase() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    for (model, rev, serial) in [
        ("Z3805A", "3543B-A", "3542A01548"),
        ("Z3801A", "unknown-A", "3542A01548"),
        ("Z3801A", "3543-A", "wrong"),
    ] {
        let mut s = Script::start(model, rev, "PRIMARY");
        assert!(flash(&mut s, &firmware, true, Some(serial)).is_err());
        assert_eq!(s.sent, ["*IDN?"]);
    }
}

fn installer_from(mode: Mode) -> Script {
    let mut s = match mode {
        Mode::Primary => Script::start("Z3801A", "3543-A", "PRIMARY"),
        Mode::Installer => Script::start("Z3801A", "Peru-A", "INSTALL"),
    };
    if mode == Mode::Primary {
        s.add(":SYSTem:LANGuage \"INSTALL\"", "");
    }
    s.add("*IDN?", "HEWLETT-PACKARD,Z3801A,3542A01548,Peru-A");
    s.add(":SYSTem:LANGuage?", "INSTALL");
    s.add(":SYSTem:ERRor?", "+0,\"No error\"");
    s.add(":DIAGnostic:ERASe", "");
    s
}

#[test]
fn erase_failure_never_downloads_or_reboots() {
    let mut s = installer_from(Mode::Installer);
    s.add(":DIAGnostic:ERASe?", "+0");
    assert!(
        flash(
            &mut s,
            &Firmware::validate(DUMP.to_vec()).unwrap(),
            true,
            Some("3542A01548")
        )
        .is_err()
    );
    assert!(s.replies.is_empty());
    assert_eq!(s.sent.last().unwrap(), ":DIAGnostic:ERASe?");
}

#[test]
fn download_failure_never_retries_or_reboots() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let mut s = installer_from(Mode::Installer);
    s.add(":DIAGnostic:ERASe?", "+1");
    s.add(":SYSTem:ERRor?", "+0,\"No error\"");
    s.add(
        &format!(
            ":DIAGnostic:DOWNload \"{}\"",
            firmware.records().next().unwrap().1
        ),
        "FAIL",
    );
    assert!(flash(&mut s, &firmware, true, Some("3542A01548")).is_err());
    assert!(s.replies.is_empty());
    assert!(s.sent.last().unwrap().starts_with(":DIAGnostic:DOWNload "));
}

fn full_transfer(firmware: &Firmware, start: Mode, finish: Mode) -> Script {
    let mut s = installer_from(start);
    s.add(":DIAGnostic:ERASe?", "+1");
    s.add(":SYSTem:ERRor?", "+0,\"No error\"");
    for (_, record) in firmware.records() {
        s.add(&format!(":DIAGnostic:DOWNload \"{record}\""), "");
        s.add(":SYSTem:ERRor?", "+0,\"No error\"");
    }
    s.add(":SYSTem:ERRor?", "+0,\"No error\"");
    s.add(":SYSTem:LANGuage \"PRIMARY\"", "");
    let final_queries = match finish {
        Mode::Primary => Script::start("Z3801A", "3543-A", "PRIMARY"),
        Mode::Installer => Script::start("Z3801A", "Peru-A", "INSTALL"),
    };
    s.replies.extend(final_queries.replies.into_iter().take(2));
    s
}

#[test]
fn primary_and_recovery_program_every_record() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    for mode in [Mode::Primary, Mode::Installer] {
        let mut s = full_transfer(&firmware, mode, Mode::Primary);
        flash(&mut s, &firmware, true, Some("3542A01548")).unwrap();
        assert!(s.replies.is_empty());
    }
}

#[test]
fn failed_boot_is_not_reported_as_success() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let mut s = full_transfer(&firmware, Mode::Primary, Mode::Installer);
    assert!(flash(&mut s, &firmware, true, Some("3542A01548")).is_err());
    assert!(s.replies.is_empty());
}

#[test]
fn simulator_upgrades_and_downgrades_preserving_the_original_installer() {
    let older = include_bytes!("../../../../third_party/58503a-3633.bin");
    let newer = include_bytes!("../../../../third_party/58503a-3704.bin");
    for (original, candidate) in [(older, newer), (newer, older)] {
        let original_firmware = Firmware::validate(original.to_vec()).unwrap();
        let firmware = Firmware::validate(candidate.to_vec()).unwrap();
        let transport = simulated(&original_firmware, original.to_vec(), FlashLayout::AmdLanes);
        let mut session = session(transport.clone());
        flash(&mut session, &firmware, true, Some("3542A01548")).unwrap();
        assert_eq!(
            session
                .query("*IDN?")
                .unwrap()
                .one_line("identity")
                .unwrap(),
            format!(
                "HEWLETT-PACKARD,58503A,3542A01548,{}-A",
                firmware.profile.revision
            )
        );
        let receiver = transport.receiver().lock().unwrap();
        let installer = receiver.installer.as_ref().unwrap();
        assert!(!installer.active);
        assert_eq!(installer.revision, original_firmware.profile.installer);
        let mut expected = original.to_vec();
        expected[PRIMARY_START..].copy_from_slice(&candidate[PRIMARY_START..]);
        assert_eq!(installer.flash, expected);
    }
}

#[test]
fn a_changed_suffix_does_not_hide_a_successful_upgrade_or_wrong_revision() {
    let firmware =
        Firmware::validate(include_bytes!("../../../../third_party/58503a-3704.bin").to_vec())
            .unwrap();
    for final_revision in ["3704-D", "3633-D"] {
        let mut script = full_transfer(&firmware, Mode::Primary, Mode::Primary);
        for ((_, reply), revision) in script
            .replies
            .iter_mut()
            .filter(|(command, _)| command == "*IDN?")
            .zip(["3633-C", "Oman-C", final_revision])
        {
            *reply = format!("HEWLETT-PACKARD,58503A,3542A01548,{revision}");
        }
        let result = flash(&mut script, &firmware, true, Some("3542A01548"));
        assert_eq!(result.is_ok(), final_revision == "3704-D", "{result:?}");
        assert!(script.replies.is_empty());
    }
}

#[test]
fn installer_transition_rejects_unknown_revisions_and_changed_suffixes() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    for revision in ["Unknown-A", "USA-A", "Peru-B"] {
        let mut script = Script::start("Z3801A", "3543-A", "PRIMARY");
        script.add(":SYSTem:LANGuage \"INSTALL\"", "");
        script.add(
            "*IDN?",
            &format!("HEWLETT-PACKARD,Z3801A,3542A01548,{revision}"),
        );
        if revision == "Peru-B" {
            script.add(":SYSTem:LANGuage?", "INSTALL");
        }
        assert!(flash(&mut script, &firmware, true, Some("3542A01548")).is_err());
        assert!(script.replies.is_empty());
        assert!(
            !script
                .sent
                .iter()
                .any(|command| command == ":DIAGnostic:ERASe")
        );
    }
}

#[test]
fn simulator_distinguishes_installer_headers_from_bad_data() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let mut session = session(simulated(&firmware, DUMP.to_vec(), FlashLayout::AmdLanes));
    session.query(":SYSTem:LANGuage \"INSTALL\"").unwrap();
    for (command, expected_code) in [
        (":SYNChronization:TINTerval?", -113),
        (":DIAGnostic:DOWNload S2060100001234B2", -112),
        (":DIAGnostic:DOWNload invalid", -222),
        (":DIAGnostic:DOWNload \"S2060100001234B2", -222),
        (":DIAGnostic:DOWNload \"S206010000123400\"", -222),
    ] {
        let error = session.query(command).unwrap_err();
        assert!(
            matches!(error, SessionError::Device { code, .. } if code == expected_code),
            "{error}"
        );
    }
    assert!(
        session
            .query("*IDN?")
            .unwrap()
            .one_line("identity")
            .unwrap()
            .ends_with("Peru-A")
    );
}

#[test]
fn stale_errors_explain_recovery_without_clearing_the_remaining_queue() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    for write in [false, true] {
        let transport = simulated(&firmware, DUMP.to_vec(), FlashLayout::AmdLanes);
        {
            let mut receiver = transport.receiver().lock().unwrap();
            receiver.queue_error(-113, "Undefined header");
            receiver.queue_error(-230, "Data corrupt or stale");
            receiver.queue_error(-222, "Data out of range");
        }
        let mut session = session(transport.clone());
        let error = flash(&mut session, &firmware, write, Some("3542A01548")).unwrap_err();
        let message = format!("{error:#}");
        for expected in [
            "preflight",
            ":SYSTem:ERRor?",
            "*CLS",
            "before retrying",
            "-113",
            "Undefined header",
        ] {
            assert!(message.contains(expected), "{message}");
        }
        assert!(
            session
                .query(":SYSTem:ERRor?")
                .unwrap()
                .one_line("queued error")
                .unwrap()
                .starts_with("-230,")
        );
        session.query("*CLS").unwrap();
        assert_eq!(
            session
                .query(":SYSTem:ERRor?")
                .unwrap()
                .one_line("empty queue")
                .unwrap(),
            "+0,\"No error\""
        );
        flash(&mut session, &firmware, false, Some("3542A01548")).unwrap();
        let receiver = transport.receiver().lock().unwrap();
        let installer = receiver.installer.as_ref().unwrap();
        assert!(!installer.active);
        assert_eq!(installer.flash, DUMP);
    }
}

#[test]
fn a_receiver_on_the_network_is_opened_checked_and_recorded() {
    let firmware = Firmware::validate(DUMP.to_vec()).unwrap();
    let transport = simulated(&firmware, DUMP.to_vec(), FlashLayout::AmdLanes);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = transport.clone();
    // One connection to find the settings, one for the session.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            serve(stream.unwrap(), served.clone()).unwrap();
        }
    });
    let settings = Settings {
        path: format!("tcp://{address}"),
        ..Settings::default()
    };
    let config = Config {
        idle: Duration::from_millis(1),
        ..Config::default()
    };
    let transcript = std::env::temp_dir().join(format!(
        "smartclock-flash-test-{}.jsonl",
        std::process::id()
    ));
    let file = std::fs::File::create(&transcript).unwrap();
    let mut session = Session::new(open(&settings, &config, Some(file)).unwrap(), config);
    session.sync().unwrap();
    flash(&mut session, &firmware, false, Some("3542A01548")).unwrap();
    drop(session);
    let recorded = std::fs::read_to_string(&transcript).unwrap();
    std::fs::remove_file(&transcript).unwrap();
    assert!(recorded.contains("IDN"), "{recorded}");
    let receiver = transport.receiver().lock().unwrap();
    let installer = receiver.installer.as_ref().unwrap();
    assert!(!installer.active);
    assert_eq!(installer.flash, DUMP);
}
