//! Opening Crook when the person logs in.
//!
//! Each platform has one place a program is listed to be started at login,
//! and that place is the person's own, not the system's: a LaunchAgent in
//! `~/Library/LaunchAgents` on macOS, an XDG autostart entry in
//! `~/.config/autostart` on Linux, and the `Run` key under `HKEY_CURRENT_USER`
//! on Windows. None needs a privilege, and each is undone by taking the one
//! entry Crook wrote back out — which is all [`apply`] ever does, so nothing
//! of anybody else's is read or touched.
//!
//! On, from the General settings' **Open at login**. The entry names the
//! binary that is running at the moment it is switched on, so a Crook that
//! is moved afterwards is switched off and on again to follow it — the same
//! as every application whose login item is a path.
//!
//! What each entry *says* is worked out by a function that runs everywhere,
//! so that the macOS plist and the Linux desktop entry are both tested on
//! every platform; only the writing is per platform.

use std::path::{Path, PathBuf};

/// The LaunchAgent's label, which is also its file's name.
pub const LAUNCH_AGENT: &str = "com.theguriev.crook.login";

/// The Linux autostart entry's file name.
pub const AUTOSTART_ENTRY: &str = "crook.desktop";

/// The name of the value Crook writes under Windows' `Run` key.
pub const RUN_VALUE: &str = "Crook";

/// Puts Crook's login entry in place, or takes it out.
///
/// Taking out an entry that is not there is not an error, so switching the
/// setting off on a machine where it was never on does nothing quietly.
pub fn apply(enabled: bool) -> Result<(), String> {
    let program = std::env::current_exe()
        .map_err(|why| format!("could not tell where Crook is running from: {why}"))?;
    platform::apply(enabled, &program)
}

/// The LaunchAgent that opens `program` at login.
///
/// An application bundle is opened with `open`, as the Dock opens it, so it
/// starts as an application — its own icon, its own menu — and not as a
/// process launchd happens to be holding; a bare binary is started as it is.
pub fn launch_agent_plist(program: &Path) -> String {
    let arguments = match bundle_of(program) {
        Some(bundle) => vec!["/usr/bin/open".to_owned(), bundle.display().to_string()],
        None => vec![program.display().to_string()],
    };
    let arguments: String = arguments
        .iter()
        .map(|argument| format!("\t\t<string>{}</string>\n", escape_xml(argument)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LAUNCH_AGENT}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {arguments}\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The XDG autostart entry that opens `program` at login.
///
/// The path quoted, as the Desktop Entry spec's `Exec` key quotes an argument
/// with a space in it, and with `"`, `` ` ``, `$` and `\` escaped inside the
/// quotes as it says to.
pub fn autostart_entry(program: &Path) -> String {
    let mut quoted = String::from("\"");
    for character in program.display().to_string().chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Crook\n\
         Comment=Opened at login, from Crook's settings\n\
         Exec={quoted}\n\
         Icon=crook\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// The `.app` a binary is inside, when it is inside one.
fn bundle_of(program: &Path) -> Option<PathBuf> {
    program
        .ancestors()
        .find(|ancestor| {
            ancestor
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .map(Path::to_path_buf)
}

/// `text` with the five characters XML gives a meaning written as entities.
fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Writes `contents` to `path`, making its directory, or removes it.
#[cfg(unix)]
fn place(path: &Path, contents: Option<String>) -> Result<(), String> {
    match contents {
        Some(contents) => {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)
                    .map_err(|why| format!("could not make {}: {why}", directory.display()))?;
            }
            std::fs::write(path, contents)
                .map_err(|why| format!("could not write {}: {why}", path.display()))
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(why) => Err(format!("could not remove {}: {why}", path.display())),
        },
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub fn apply(enabled: bool, program: &Path) -> Result<(), String> {
        let home = std::env::home_dir().ok_or("this account has no home directory")?;
        let path = home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCH_AGENT}.plist"));
        place(&path, enabled.then(|| launch_agent_plist(program)))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::*;

    pub fn apply(enabled: bool, program: &Path) -> Result<(), String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| Some(std::env::home_dir()?.join(".config")))
            .ok_or("this account has no home directory")?;
        let path = config.join("autostart").join(AUTOSTART_ENTRY);
        place(&path, enabled.then(|| autostart_entry(program)))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    /// Through `reg`, which every Windows has, rather than a registry crate
    /// for one value.
    pub fn apply(enabled: bool, program: &Path) -> Result<(), String> {
        const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let quoted = format!("\"{}\"", program.display());
        let mut command = crate::process::command("reg");
        if enabled {
            command.args([
                "add", RUN, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &quoted, "/f",
            ]);
        } else {
            command.args(["delete", RUN, "/v", RUN_VALUE, "/f"]);
        }
        let output = command
            .output()
            .map_err(|why| format!("could not run reg: {why}"))?;
        // Deleting a value that is not there fails, and is what "off" already
        // means.
        if output.status.success() || !enabled {
            Ok(())
        } else {
            Err(format!(
                "reg could not add the login entry: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_bundle_is_opened_the_way_the_dock_opens_it() {
        let plist = launch_agent_plist(Path::new("/Applications/Crook.app/Contents/MacOS/crook"));
        assert!(plist.contains("<string>com.theguriev.crook.login</string>"));
        assert!(plist.contains("<string>/usr/bin/open</string>"));
        assert!(plist.contains("<string>/Applications/Crook.app</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }

    #[test]
    fn a_bare_binary_is_started_as_it_is_and_its_path_escaped() {
        let plist = launch_agent_plist(Path::new("/opt/R&D/crook"));
        assert!(plist.contains("<string>/opt/R&amp;D/crook</string>"));
        assert!(!plist.contains("/usr/bin/open"));
    }

    #[test]
    fn the_autostart_entry_quotes_its_path_the_way_the_spec_says() {
        let entry = autostart_entry(Path::new("/home/someone/My Apps/crook$1"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Exec=\"/home/someone/My Apps/crook\\$1\"\n"));
        assert!(entry.contains("Type=Application\n"));
    }
}
