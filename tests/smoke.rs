//! Smoke tests for the binary's non-interactive paths (AC-01 parts).

use std::process::Command;

fn kyte() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kyte"))
}

#[test]
fn ac01_version_prints_and_exits_0() {
    let out = kyte().arg("--version").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "kyte 0.6.0");
}

#[test]
fn ac01_help_lists_options_and_exits_0() {
    let out = kyte().arg("--help").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = String::from_utf8_lossy(&out.stdout);
    for o in [
        "--help",
        "--version",
        "--esc-ms",
        "--no-kitty",
        "--print-swap",
    ] {
        assert!(s.contains(o), "help misses {o}");
    }
}

#[test]
fn ac01_unknown_option_exits_2() {
    let out = kyte().arg("--definitely-not").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option"));
}

#[test]
fn ac01_missing_value_exits_2() {
    let out = kyte().arg("--esc-ms").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// A build without the updater says so, and has nothing to reach the network with.
#[cfg(not(feature = "update"))]
#[test]
fn a_build_without_the_updater_says_so() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kyte"))
        .arg("--update")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("no updater"), "{said}");
}
