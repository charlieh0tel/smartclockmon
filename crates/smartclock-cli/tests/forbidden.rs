//! The commands no direct-mode tool sends are refused before the port
//! is opened.

use std::process::Command;

#[test]
fn a_forbidden_command_is_refused_before_anything_is_opened() {
    // A device that does not exist: reaching it would fail on opening,
    // so being told about the refusal instead shows it came first.
    let output = Command::new(env!("CARGO_BIN_EXE_smartclock-cli"))
        .args([
            "--device",
            "/nonexistent/smartclock",
            "query",
            "*IDN?",
            ":syst:comm:ser:baud 9600",
        ])
        .output()
        .expect("run the tool");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("refusing to send"), "{stderr}");
    assert!(!stderr.contains("/nonexistent"), "{stderr}");
}
