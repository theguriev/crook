//! `script/glibc-floor`, which holds the Linux archive to the glibc it promises.
//!
//! The archive this module installs on Linux, and `script/install` with it, is
//! only worth installing if it starts, and v0.1.13's did not on Ubuntu 22.04 or
//! Debian 12. It was linked on a runner with glibc 2.39, and two weak references
//! in Rust std left a version need the dynamic loader refuses to start without.
//! `release.yml` now builds that archive at the floor and runs the script over
//! the binary before it is published; these run it over `readelf` listings
//! instead, one of them the listing of the release that broke, because `cargo
//! test` runs on every change and a release build does not.
//!
//! Unix only: the script is `sh`, and so is what runs it.

use std::io::Write as _;
use std::process::Stdio;

/// `readelf -W --dyn-syms -V` of the v0.1.13 Linux binary, cut to the rows
/// that matter; the version needs are whole.
const V0_1_13: &str = include_str!("../../../script/fixtures/readelf-v0.1.13.txt");

/// What the script said about one listing.
struct Verdict {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `script/glibc-floor <floor> -` with `listing` on its standard input.
fn checked(floor: &str, listing: &str) -> Verdict {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .join("script/glibc-floor");
    let mut child = crate::process::command("sh")
        .arg(&script)
        .arg(floor)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sh should start");
    // A floor the script refuses ends it before it reads a byte, and the
    // write can then find the pipe already closed. That is the script
    // answering, not the test failing; its exit status says which.
    let written = child
        .stdin
        .take()
        .expect("standard input is piped")
        .write_all(listing.as_bytes());
    if let Err(error) = written {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "{error}");
    }
    let output = child.wait_with_output().expect("the script should finish");
    Verdict {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A version needs section for `libc.so.6` holding `versions`, laid out the
/// way `readelf -V` prints one.
fn needs(versions: &[&str]) -> String {
    let mut listing = format!(
        "Version needs section '.gnu.version_r' contains 1 entry:\n \
         Addr: 0x0000000000000000  Offset: 0x00000000  Link: 7 (.dynstr)\n  \
         000000: Version: 1  File: libc.so.6  Cnt: {}\n",
        versions.len()
    );
    for (index, version) in versions.iter().enumerate() {
        listing.push_str(&format!(
            "  0x{:04x}:   Name: GLIBC_{version}  Flags: none  Version: {}\n",
            (index + 1) * 16,
            index + 2
        ));
    }
    listing
}

#[test]
fn the_release_that_would_not_start_on_ubuntu_22_04_is_refused() {
    let verdict = checked("2.31", V0_1_13);

    assert_eq!(verdict.code, Some(1), "{}", verdict.stderr);
    for need in [
        "GLIBC_2.32",
        "GLIBC_2.33",
        "GLIBC_2.34",
        "GLIBC_2.35",
        "GLIBC_2.39",
    ] {
        assert!(
            verdict.stderr.contains(need),
            "{need} is not named in:\n{}",
            verdict.stderr
        );
    }
    // The symbols behind the needs, named, because "GLIBC_2.39 not found"
    // says nothing about where in a Rust binary a need came from.
    for symbol in ["pidfd_spawnp", "pidfd_getpid", "hypot"] {
        assert!(
            verdict.stderr.contains(symbol),
            "{symbol} is not named in:\n{}",
            verdict.stderr
        );
    }
    // And nothing the floor already covers is blamed, 2.9 included, which is
    // above 2.31 when the two are compared as text.
    for fine in [
        "GLIBC_2.30",
        "GLIBC_2.9",
        "GLIBC_2.2.5",
        "malloc",
        "GCC_3.0",
    ] {
        assert!(
            !verdict.stderr.contains(fine),
            "{fine} was blamed in:\n{}",
            verdict.stderr
        );
    }
}

#[test]
fn a_floor_is_met_by_a_need_equal_to_it() {
    // The floor is the oldest glibc the binary runs on, so a need for exactly
    // that version is inside it, and the next one down is not.
    let at = checked("2.39", V0_1_13);
    assert_eq!(at.code, Some(0), "{}", at.stderr);
    assert!(at.stdout.contains("GLIBC_2.39"), "{}", at.stdout);

    let below = checked("2.38", V0_1_13);
    assert_eq!(below.code, Some(1), "{}", below.stderr);
    assert!(below.stderr.contains("GLIBC_2.39"), "{}", below.stderr);
    assert!(!below.stderr.contains("GLIBC_2.35"), "{}", below.stderr);
}

#[test]
fn a_version_is_compared_by_its_numbers_and_not_by_its_text() {
    // Text gets the first two wrong: 2.10 sorts before 2.9, and the needs of
    // a real binary run from 2.2.5 to 2.39. The third is a version one number
    // longer than the floor, which is above it too.
    let ten = checked("2.9", &needs(&["2.2.5", "2.9", "2.10"]));
    assert_eq!(ten.code, Some(1), "2.10 is above 2.9: {}", ten.stderr);
    assert!(ten.stderr.contains("GLIBC_2.10"), "{}", ten.stderr);

    let nine = checked("2.10", &needs(&["2.2.5", "2.9"]));
    assert_eq!(nine.code, Some(0), "2.9 is under 2.10: {}", nine.stderr);

    let longer = checked("2.3", &needs(&["2.2.5", "2.3", "2.3.4"]));
    assert_eq!(
        longer.code,
        Some(1),
        "2.3.4 is above 2.3: {}",
        longer.stderr
    );
    assert!(longer.stderr.contains("GLIBC_2.3.4"), "{}", longer.stderr);
}

#[test]
fn a_listing_with_no_glibc_needs_is_an_error_and_not_a_pass() {
    // A check that finds nothing and passes is what a wrong path, a static
    // binary or a readelf that changed its layout would all look like, and
    // each of those would wave through whatever was built.
    let only_gcc = "Version needs section '.gnu.version_r' contains 1 entry:\n  \
                    000000: Version: 1  File: libgcc_s.so.1  Cnt: 1\n  \
                    0x0010:   Name: GCC_3.0  Flags: none  Version: 2\n";
    for listing in ["", "readelf: Error: Not an ELF file\n", only_gcc] {
        let verdict = checked("2.31", listing);
        assert_eq!(
            verdict.code,
            Some(2),
            "{listing:?} passed: {}",
            verdict.stdout
        );
    }
}

#[test]
fn a_floor_that_is_not_a_version_is_refused() {
    // An empty floor is what an unset variable in a workflow hands over, and
    // it must not read as "no floor at all".
    for floor in ["", "latest", "2.", ".31", "2..31", "2.31a"] {
        let verdict = checked(floor, V0_1_13);
        assert_eq!(verdict.code, Some(2), "{floor:?} was taken as a floor");
    }
}
