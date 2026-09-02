//! Spawning a process without flashing a console window at the user.
//!
//! On Windows every process a GUI application starts opens a console window of
//! its own unless it is created with `CREATE_NO_WINDOW`. A terminal starts a
//! lot of processes, so forgetting the flag once is a visible bug, and
//! remembering it at every call site is not a plan. The workspace's
//! `.clippy.toml` therefore bans `std::process::Command::new` outright and
//! names this function as the replacement — which is why this is the one place
//! in the workspace that is allowed to call it.

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
