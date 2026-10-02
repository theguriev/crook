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
pub fn reap(mut child: std::process::Child, name: &'static str) {
    let reaper = std::thread::Builder::new()
        .name(format!("crook-reap-{name}"))
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => log::debug!("{name} exited {status}"),
            Ok(_) => {}
            Err(why) => log::debug!("{name} could not be waited on: {why}"),
        });
    if let Err(why) = reaper {
        log::debug!("nothing could wait on {name}: {why}");
    }
}
