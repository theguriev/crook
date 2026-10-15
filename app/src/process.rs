//! Spawning a process without flashing a console window at the user.
//!
//! On Windows every process a GUI application starts opens a console window of
//! its own unless it is created with `CREATE_NO_WINDOW`. A terminal starts a
//! lot of processes, so forgetting the flag once is a visible bug, and
//! remembering it at every call site is not a plan. The workspace's
//! `.clippy.toml` therefore bans `std::process::Command::new` outright and
//! names this function as the replacement — which is why this is the one place
//! in the workspace that is allowed to call it.
//!
//! And the other half of starting a process nobody waits on the answer from:
//! [`reap`], so that it does not stay a zombie for as long as Crook runs.

/// Builds a [`std::process::Command`] that runs `program` invisibly.
///
/// Identical to `Command::new` everywhere except Windows, where it also sets
/// `CREATE_NO_WINDOW`.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "this function is the replacement those lints point at"
)]
pub fn command(program: &str) -> std::process::Command {
    let command = std::process::Command::new(program);

    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt as _;

        // winbase.h CREATE_NO_WINDOW. Spelled out rather than pulled from a
        // crate, because one hexadecimal constant is not worth a dependency
        // that only builds on one of the three target platforms.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };

    command
}

/// Waits on `child` on a thread of its own, so it is reaped when it exits
/// rather than left a zombie for as long as Crook runs.
///
/// For a program Crook starts and does not want an answer from — a link's
/// opener, an editor, a desktop notification. Dropping a [`std::process::Child`]
/// does not wait on it, and on Unix a child nobody waits on keeps its entry in
/// the process table until its parent exits. A thread rather than the
/// background pool, because what is waited on can live for hours — an editor
/// window, an `xdg-open` that stays until the browser it started closes — and
/// a pool worker held that long is one every git read is waiting for.
///
/// `name` is what the program is called in the log: a non-zero exit is a
/// debug line and nothing more, since nobody is waiting on the outcome.
pub fn reap(child: std::process::Child, name: &'static str) {
    reap_at(child, name, log::Level::Debug);
}

/// [`reap`], with a non-zero exit logged at `level` rather than as a debug
/// line.
///
/// For a child whose failure is worth finding in the log even though nobody
/// waits on it: a window New window opened, which otherwise just never
/// appears. See [`crate::new_window`].
pub fn reap_at(mut child: std::process::Child, name: &'static str, level: log::Level) {
    let reaper = std::thread::Builder::new()
        .name(format!("crook-reap-{name}"))
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => log::log!(level, "{name} exited {status}"),
            Ok(_) => {}
            Err(why) => log::debug!("{name} could not be waited on: {why}"),
        });
    if let Err(why) = reaper {
        log::debug!("nothing could wait on {name}: {why}");
    }
}

/// Writes `contents` to `path` as a program, for a test that runs it at once.
///
/// Not `fs::write` and a `chmod`, because of the threads beside the test. A
/// thread that forks while the file is still open for writing hands its child
/// a copy of that descriptor, which the child holds until it execs, and until
/// then Linux refuses to run the file: `ETXTBSY`, "Text file busy". The suite
/// starts processes on every thread all the time, so a fake `gh` or hook
/// written here and run a moment later failed now and then for no reason in
/// the code under test. Renaming a finished file into place does not help,
/// since it is the same file under a new name. So the bytes go to a staging
/// file nothing runs, and a `cp` of their own copies them to `path`: the
/// program is only ever open for writing in that process, which forks
/// nothing, and is closed for good when it exits.
#[cfg(all(test, unix))]
pub(crate) fn write_program(path: &std::path::Path, contents: &str) {
    use std::sync::atomic::{AtomicU64, Ordering};

    static STAGED: AtomicU64 = AtomicU64::new(0);
    let staged = std::env::temp_dir().join(format!(
        "crook-program-{}-{}",
        std::process::id(),
        STAGED.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&staged, contents).expect("the temporary directory is writable");
    copy_in(&staged, path);
    let _ = std::fs::remove_file(&staged);
}

/// Copies `staged` to `path` in a process of its own and makes the copy
/// runnable: the half of [`write_program`] that keeps the program out of
/// every descriptor this process holds.
#[cfg(all(test, unix))]
fn copy_in(staged: &std::path::Path, path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;

    // By its path: a test that empties this process's PATH for a moment would
    // otherwise take `cp` away from every test writing a program meanwhile.
    let copied = command("/bin/cp")
        .arg(staged)
        .arg(path)
        .status()
        .expect("cp can be started");
    assert!(copied.success(), "cp could not write {}", path.display());
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("the program can be made runnable");
}

#[cfg(all(test, unix))]
#[path = "process_tests.rs"]
mod tests;
