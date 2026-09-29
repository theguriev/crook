//! The built `crook` binary, run with a command line it has to refuse.
//!
//! What the unit tests cannot reach is `run` itself, which is where the
//! process's own arguments are read: a test of the helper that turns them
//! into text stays green when `run` goes back to reading them some other
//! way. Every case here is refused while the arguments are read, before
//! anything opens a window, so it runs on a machine with no display.

/// A folder name on Linux is bytes, and a file manager's "Open terminal here"
/// hands it over as it is. `std::env::args` panics on one that is not UTF-8,
/// which is a crash with exit status 101 and no window; Crook says which word
/// it could not read and exits 1, as it does for any argument it cannot use.
#[test]
#[cfg(unix)]
fn a_word_that_is_not_text_is_a_line_on_stderr_and_not_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let folder = OsString::from_vec(b"/tmp/caf\xe9".to_vec());
    let output = crook::process::command(env!("CARGO_BIN_EXE_crook"))
        .arg("--working-directory")
        .arg(folder)
        .output()
        .expect("the crook binary runs");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(Some(1), output.status.code(), "{stderr}");
    assert!(
        stderr.contains(r#"crook: "/tmp/caf\xE9" is not UTF-8"#),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}
