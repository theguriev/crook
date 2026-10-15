//! Executable specification for [`write_program`]: why a program a test
//! writes and runs at once failed now and then, made to fail every time, and
//! why one written the helper's way cannot.
//!
//! What made it fail is a child forked by another thread while the file was
//! open for writing. That child holds the same open file as the writer — a
//! copy of its descriptor, the very thing `try_clone` makes — so a second
//! handle kept open here stands in for it exactly, and needs no luck with
//! the scheduler to land where it has to.

use std::fs::{File, OpenOptions, Permissions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "crook-process-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What `program` printed, or why it could not be started.
fn ran(program: &Path) -> std::io::Result<String> {
    let output = command(program.to_str().expect("the scratch path is text")).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A program that says it ran.
const PROGRAM: &str = "#!/bin/sh\necho ran\n";

#[cfg(target_os = "linux")]
#[test]
fn a_program_open_for_writing_anywhere_is_refused_under_any_name() {
    // The CI failure, on purpose: written, closed, made runnable — and still
    // open for writing in the child a fork made in between.
    let scratch = Scratch::new("busy");
    let program = scratch.0.join("gh");
    let mut written = File::create(&program).expect("the scratch directory is writable");
    written
        .write_all(PROGRAM.as_bytes())
        .expect("the program is written");
    let forked = written.try_clone().expect("the descriptor can be copied");
    drop(written);
    std::fs::set_permissions(&program, Permissions::from_mode(0o755))
        .expect("the program can be made runnable");

    let refused = ran(&program).expect_err("a program open for writing was run");
    assert_eq!(ErrorKind::ExecutableFileBusy, refused.kind(), "{refused}");

    // Writing it elsewhere and renaming it into place is the same file under
    // another name, held open all the same.
    let renamed = scratch.0.join("renamed");
    std::fs::rename(&program, &renamed).expect("the program can be renamed");
    assert_eq!(
        Some(ErrorKind::ExecutableFileBusy),
        ran(&renamed).err().map(|refused| refused.kind())
    );
    drop(forked);
}

#[test]
fn a_written_program_runs_while_a_fork_still_holds_what_it_was_written_from() {
    // The one file `write_program` opens for writing in this process is the
    // staging file, so the worst a fork can do is hold that one open, which
    // this does for the whole test.
    let scratch = Scratch::new("written");
    let staged = scratch.0.join("staged");
    std::fs::write(&staged, PROGRAM).expect("the scratch directory is writable");
    let forked = OpenOptions::new()
        .append(true)
        .open(&staged)
        .expect("the staging file can be held open");
    if cfg!(target_os = "linux") {
        // Proof that the hold is real: the staging file itself would not run.
        std::fs::set_permissions(&staged, Permissions::from_mode(0o755))
            .expect("the staging file can be made runnable");
        assert_eq!(
            Some(ErrorKind::ExecutableFileBusy),
            ran(&staged).err().map(|refused| refused.kind())
        );
    }

    let program = scratch.0.join("gh");
    copy_in(&staged, &program);
    assert_eq!("ran\n", ran(&program).expect("the copy runs"));
    drop(forked);

    // And the whole helper, end to end.
    let whole = scratch.0.join("hook");
    write_program(&whole, PROGRAM);
    assert_eq!("ran\n", ran(&whole).expect("the written program runs"));
    let mode = std::fs::metadata(&whole)
        .expect("the program is there")
        .permissions()
        .mode();
    assert_eq!(0o755, mode & 0o777, "written as {mode:o}");
}
