//! The built `crook` binary, run with a command line it has to refuse, or
//! with an answer nobody is left to read.
//!
//! What the unit tests cannot reach is `run` itself, which is where the
//! process's own arguments are read and where a panic becomes an exit status:
//! a test of the helper that turns them into text stays green when `run` goes
//! back to reading them some other way. Every case here is answered or
//! refused while the arguments are read, before anything opens a window, so
//! it runs on a machine with no display.

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

/// `crook pane blocks 7 | head -5` on a long build log: `head` takes its five
/// lines and goes, and the rest of the answer is written into a pipe nobody
/// reads. Rust leaves SIGPIPE ignored, so that write fails with EPIPE, and
/// `println!` made the failure a panic and exit status 101 — the lines all
/// there, and a pipeline under `pipefail` saying Crook had crashed. A reader
/// that has gone is the end of the answer, as it is for any command piped
/// to `head`.
///
/// Both kinds of answer: one a window gave — a stand-in that answers the one
/// request with a value `--json` prints as it came — and one the binary has
/// without asking anybody. The pipe's reader is gone before the command
/// starts, so the first write is the one that fails, however short.
#[test]
#[cfg(unix)]
fn an_answer_nobody_is_left_to_read_ends_the_command_and_is_not_a_panic() {
    use std::fs::Permissions;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    let directory = std::env::temp_dir().join(format!("crook-cl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let socket = directory.join("w.sock");
    let listener = UnixListener::bind(&socket).expect("a socket to answer on");
    // Locked as a window locks its own, or the command line will not ask it.
    std::fs::set_permissions(&directory, Permissions::from_mode(0o700)).expect("closed");
    std::fs::set_permissions(&socket, Permissions::from_mode(0o600)).expect("locked");
    let window = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the command line asks");
        let mut asked = String::new();
        let _ = BufReader::new(&stream).read_line(&mut asked);
        let answer = r#"{"v":1,"ok":true,"result":{"pane_id":7,"blocks":[]}}"#;
        let _ = writeln!(stream, "{answer}");
        asked
    });

    for words in [&["pane", "blocks", "7", "--json"][..], &["--help"][..]] {
        let (reader, writer) = std::io::pipe().expect("a pipe");
        drop(reader);
        let output = crook::process::command(env!("CARGO_BIN_EXE_crook"))
            .args(words)
            .env("CROOK_SOCKET", &socket)
            .env_remove("CROOK_TOKEN")
            .stdout(writer)
            .output()
            .expect("the crook binary runs");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked"), "{words:?}: {stderr}");
        assert_eq!(Some(0), output.status.code(), "{words:?}: {stderr}");
    }
    // A command line that never asked would leave the stand-in waiting for
    // ever; one connection of the test's own ends the wait, and the request
    // it read is then an empty one.
    let _ = UnixStream::connect(&socket);
    let asked = window.join().expect("the stand-in window answered");
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        asked.contains("pane.blocks"),
        "the window was not asked for the blocks: {asked:?}"
    );
}
