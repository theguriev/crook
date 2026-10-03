//! Making Crook the terminal the system opens.
//!
//! Each desktop keeps the answer in one place of the person's own: on macOS,
//! Launch Services' handler for the shell role of `public.unix-executable` —
//! what Finder opens a `.command` or a script with — and on Linux the list
//! `xdg-terminal-exec` reads, `xdg-terminals.list`, whose first entry is the
//! terminal a launcher's *Open in terminal* and a `Terminal=true` entry get.
//! Windows has no such setting a program may change for itself, so there is
//! nothing to offer there.
//!
//! Asked from the General settings, which show **Make default** while Crook is
//! not and say so once it is. Never done unasked: the installer prints the
//! command and leaves the list alone, and this is the button that runs it.

/// What the Linux list calls Crook: the desktop entry the installer writes.
pub const DESKTOP_ENTRY: &str = "crook.desktop";

/// Whether Crook is the terminal the system opens, or `None` where that cannot
/// be asked or made so — Windows, a Mac binary outside its bundle, a Linux
/// without Crook's desktop entry installed.
pub fn is_default() -> Option<bool> {
    platform::is_default()
}

/// Makes Crook the terminal the system opens.
pub fn make_default() -> Result<(), String> {
    platform::make_default()
}

/// The first terminal `list` names: a line that is neither blank nor a
/// comment, trimmed, as `xdg-terminal-exec` reads it.
pub fn first_entry(list: &str) -> Option<&str> {
    list.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
}

/// `list` with Crook's entry first and every other line — comments, the
/// terminals it named before — kept in its order after it.
///
/// A line that already names Crook is taken out, so asking twice does not
/// list it twice.
pub fn put_first(list: &str) -> String {
    let mut out = format!("{DESKTOP_ENTRY}\n");
    for line in list.lines().filter(|line| line.trim() != DESKTOP_ENTRY) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    use objc2::rc::Retained;
    use objc2_foundation::NSString;

    /// The type a script or any other executable file is.
    const UNIX_EXECUTABLE: &str = "public.unix-executable";

    /// Launch Services' `kLSRolesShell`: the application that runs it.
    const ROLE_SHELL: u32 = 0x0000_0008;

    // Declared rather than depended on, the way `proc_pidinfo` is: two
    // functions of a framework every Mac has. Deprecated, but the
    // replacement on `NSWorkspace` opens a type and has no shell role. A `CFStringRef` is passed as an `NSString`, which is the
    // same object.
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSCopyDefaultRoleHandlerForContentType(
            content_type: *const c_void,
            role: u32,
        ) -> *mut c_void;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: *const c_void,
            role: u32,
            handler: *const c_void,
        ) -> i32;
    }

    pub fn is_default() -> Option<bool> {
        let ours = crate::notify::macos::bundle_identifier()?;
        objc2::rc::autoreleasepool(|_| {
            let content_type = NSString::from_str(UNIX_EXECUTABLE);
            // SAFETY: the type is a live string; the answer is a string the
            // caller owns, or null, and `from_raw` takes that ownership over
            // and releases it on drop.
            let handler: Option<Retained<NSString>> = unsafe {
                let raw = LSCopyDefaultRoleHandlerForContentType(
                    Retained::as_ptr(&content_type).cast(),
                    ROLE_SHELL,
                );
                Retained::from_raw(raw.cast())
            };
            // Bundle identifiers compare without case, as Launch Services
            // compares them.
            Some(handler.is_some_and(|handler| handler.to_string().eq_ignore_ascii_case(&ours)))
        })
    }

    pub fn make_default() -> Result<(), String> {
        let ours = crate::notify::macos::bundle_identifier()
            .ok_or("Crook is not running from its application bundle")?;
        let status = objc2::rc::autoreleasepool(|_| {
            let content_type = NSString::from_str(UNIX_EXECUTABLE);
            let handler = NSString::from_str(&ours);
            // SAFETY: both are live strings for the length of the call, which
            // keeps no reference to either.
            unsafe {
                LSSetDefaultRoleHandlerForContentType(
                    Retained::as_ptr(&content_type).cast(),
                    ROLE_SHELL,
                    Retained::as_ptr(&handler).cast(),
                )
            }
        });
        if status == 0 {
            Ok(())
        } else {
            Err(format!("Launch Services refused, with status {status}"))
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::path::PathBuf;

    use super::{DESKTOP_ENTRY, first_entry, put_first};

    /// `$XDG_CONFIG_HOME/xdg-terminals.list`, or `~/.config`'s.
    fn list_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| Some(std::env::home_dir()?.join(".config")))?;
        Some(config.join("xdg-terminals.list"))
    }

    /// Whether Crook's desktop entry is somewhere `xdg-terminal-exec` looks,
    /// without which listing it would list a terminal that cannot be found.
    fn entry_installed() -> bool {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| Some(std::env::home_dir()?.join(".local/share")));
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        data_home
            .into_iter()
            .chain(data_dirs.split(':').map(PathBuf::from))
            .any(|dir| dir.join("applications").join(DESKTOP_ENTRY).is_file())
    }

    pub fn is_default() -> Option<bool> {
        if !entry_installed() {
            return None;
        }
        let list = std::fs::read_to_string(list_path()?).unwrap_or_default();
        Some(first_entry(&list) == Some(DESKTOP_ENTRY))
    }

    pub fn make_default() -> Result<(), String> {
        let path = list_path().ok_or("this account has no home directory")?;
        let list = match std::fs::read_to_string(&path) {
            Ok(list) => list,
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(why) => return Err(format!("could not read {}: {why}", path.display())),
        };
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|why| format!("could not make {}: {why}", directory.display()))?;
        }
        std::fs::write(&path, put_first(&list))
            .map_err(|why| format!("could not write {}: {why}", path.display()))
    }
}

#[cfg(windows)]
mod platform {
    pub fn is_default() -> Option<bool> {
        None
    }

    pub fn make_default() -> Result<(), String> {
        Err("Windows has no default terminal a program can set for itself".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_entry_skips_comments_and_blank_lines() {
        assert_eq!(
            first_entry("# mine\n\n  foot.desktop \nkitty.desktop\n"),
            Some("foot.desktop")
        );
        assert_eq!(first_entry("# nothing yet\n"), None);
    }

    #[test]
    fn crook_goes_first_and_everything_else_keeps_its_order() {
        assert_eq!(
            put_first("# mine\nfoot.desktop\ncrook.desktop\nkitty.desktop\n"),
            "crook.desktop\n# mine\nfoot.desktop\nkitty.desktop\n"
        );
        assert_eq!(put_first(""), "crook.desktop\n");
        let once = put_first("foot.desktop\n");
        assert_eq!(put_first(&once), once);
        assert_eq!(first_entry(&once), Some(DESKTOP_ENTRY));
    }
}
