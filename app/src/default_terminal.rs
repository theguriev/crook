//! Making Crook the terminal the system opens, and opening what it is handed.
//!
//! Each desktop keeps the answer in one place of the person's own: on macOS,
//! Launch Services' handler for the shell role of `public.unix-executable` —
//! the type of a script or a program with no extension of its own, which is
//! what Finder runs when one is double-clicked — and on Linux the list
//! `xdg-terminal-exec` reads, `xdg-terminals.list`, whose first entry is the
//! terminal a launcher's *Open in terminal* and a `Terminal=true` entry get.
//! Windows has no such setting a program may change for itself, so there is
//! nothing to offer there.
//!
//! A `.command` file is not one of them. It is a type of Terminal's own,
//! `com.apple.terminal.shell-script`, which Finder goes on opening in
//! Terminal: Crook.app offers to open it, under Open With and dropped on its
//! Dock icon, and takes none of it over.
//!
//! Asked from the General settings, which show **Make default** while Crook is
//! not and say so once it is. Never done unasked: the installer prints the
//! command and leaves the list alone, and this is the button that runs it.
//!
//! # What Finder hands over
//!
//! A Mac's handler is an application Launch Services asks to *open* the file,
//! and it asks only one that says it can. So `script/macos-app` declares both
//! types in Crook.app's `Info.plist`, with the role `Shell` — it runs them —
//! and the rank `Alternate`, which never makes it the first choice by itself;
//! and `crookui` hears the request AppKit makes of it and hands the files to
//! the window. [`opening`] is what each one becomes there: a tab of its own,
//! started in the file's folder, running the file at its shell's first prompt.

use std::path::{Path, PathBuf};

/// The type of a script or a program with no extension of its own, whose
/// shell handler a Mac's default terminal is.
#[cfg(any(target_os = "macos", all(test, unix)))]
const UNIX_EXECUTABLE: &str = "public.unix-executable";

/// What the Linux list calls Crook: the desktop entry the installer writes.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
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
#[cfg(any(all(unix, not(target_os = "macos")), test))]
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
#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub fn put_first(list: &str) -> String {
    let mut out = format!("{DESKTOP_ENTRY}\n");
    for line in list.lines().filter(|line| line.trim() != DESKTOP_ENTRY) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// What a file the desktop hands Crook to open becomes: a tab, where its
/// shell starts, and what that shell is given to run there.
#[derive(Debug, PartialEq, Eq)]
pub struct Opening {
    /// Where the tab's shell starts: the folder the file is in, or the folder
    /// itself.
    pub directory: PathBuf,
    /// The line typed into the tab's field and sent at its shell's first
    /// prompt, or `None` for a folder, which is only somewhere to start.
    pub line: Option<String>,
}

/// What opening `path` means, `is_directory` saying whether it names a
/// folder; or why it cannot be opened, as a phrase.
///
/// A file is run as a person would run it at a prompt: by its full name,
/// quoted for the shell the way `-e`'s words are — see [`crate::shell_word`]
/// — from a shell started in the folder it is in. That is where a script that
/// reads the files beside it expects to be, and the line at the top of so
/// many `.command` files, `cd "$(dirname "$0")"`, is there because Terminal
/// starts them in the home directory instead.
///
/// Not `exec`'d, as `-e`'s program is. A script double-clicked in Finder
/// prints and ends, and a tab that closed with it would take what it printed
/// with it before anybody had read it; the shell stays instead, as
/// Terminal's window does, with the script's output a block above its
/// prompt. With a space in front, as `-e`'s line has and for its reason: the
/// line is Crook's rather than the person's, and the history the field
/// suggests from is theirs.
///
/// A folder is a tab started in it, with nothing to run: what `open -a Crook
/// <folder>` asks.
///
/// Refused: a path that is not absolute, which would be read against Crook's
/// own directory — for an application Launch Services started, the root of
/// the disk; a name that is not text, since the line typed into the field is;
/// and a name with a control character in it, which in a line is not a
/// character of the name — a newline ends one command and starts another.
pub fn opening(path: &Path, is_directory: bool) -> Result<Opening, &'static str> {
    if !path.is_absolute() {
        return Err("it is not a full path");
    }
    if is_directory {
        return Ok(Opening {
            directory: path.to_path_buf(),
            line: None,
        });
    }
    let name = path
        .to_str()
        .ok_or("its name is not text, which a line typed into a shell has to be")?;
    if name.chars().any(char::is_control) {
        return Err("its name holds a control character, which a shell's line cannot");
    }
    let directory = path.parent().ok_or("it is in no folder to start in")?;
    Ok(Opening {
        directory: directory.to_path_buf(),
        line: Some(format!(" {}", crate::shell_word(name))),
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    use objc2::rc::Retained;
    use objc2_foundation::NSString;

    use super::UNIX_EXECUTABLE;

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

    #[test]
    #[cfg(unix)]
    fn a_file_is_run_by_its_full_name_from_the_folder_it_is_in() {
        assert_eq!(
            opening(Path::new("/Users/someone/bin/deploy"), false),
            Ok(Opening {
                directory: PathBuf::from("/Users/someone/bin"),
                line: Some(" /Users/someone/bin/deploy".to_owned()),
            })
        );
        // Quoted as `-e`'s words are, so a name is one word whatever is in it.
        assert_eq!(
            opening(Path::new("/Users/someone/My Scripts/it's done"), false),
            Ok(Opening {
                directory: PathBuf::from("/Users/someone/My Scripts"),
                line: Some(r" '/Users/someone/My Scripts/it'\''s done'".to_owned()),
            })
        );
        assert_eq!(
            opening(Path::new("/build"), false).map(|opening| opening.directory),
            Ok(PathBuf::from("/"))
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_folder_is_a_tab_started_in_it_with_nothing_to_run() {
        assert_eq!(
            opening(Path::new("/Users/someone/work"), true),
            Ok(Opening {
                directory: PathBuf::from("/Users/someone/work"),
                line: None,
            })
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_name_no_line_can_hold_opens_nothing() {
        use std::os::unix::ffi::OsStrExt as _;

        // Read against Crook's own directory, which for an application Launch
        // Services started is the root of the disk.
        assert!(opening(Path::new("bin/deploy"), false).is_err());
        // A newline typed into the field is the end of one command and the
        // start of another.
        assert!(opening(Path::new("/tmp/two\nlines"), false).is_err());
        assert!(opening(Path::new("/tmp/a\ttab"), false).is_err());
        // A line is text, and this name is not.
        let bytes = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9"));
        assert!(opening(bytes, false).is_err());

        assert!(opening(Path::new("/tmp/one line"), false).is_ok());
    }

    /// A directory of the test's own, removed when it is dropped.
    #[cfg(unix)]
    struct Scratch(PathBuf);

    #[cfg(unix)]
    impl Scratch {
        fn new(label: &str) -> Self {
            static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "crook-default-terminal-{label}-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
    }

    #[cfg(unix)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[cfg(unix)]
    fn every_shell_runs_the_file_from_its_own_folder() {
        // In a folder and under a name each shell would otherwise read as
        // something else: a space, a quote, a parenthesis, a dollar. A shell
        // this machine lacks is skipped; `sh` never is.
        let scratch = Scratch::new("run");
        let folder = scratch.0.join("my scripts");
        std::fs::create_dir_all(&folder).expect("the folder is made");
        let script = folder.join("it's $HOME (done)");
        crate::process::write_program(&script, "#!/bin/sh\nprintf '%s\\n' \"$0\"\npwd -P\n");

        let opened = opening(&script, false).expect("a script");
        assert_eq!(opened.directory, folder);
        let line = opened.line.expect("a script is run");
        // The physical path, which is what `pwd -P` prints: a Mac's
        // temporary directory is behind a symbolic link.
        let expected = format!(
            "{}\n{}\n",
            script.display(),
            std::fs::canonicalize(&folder)
                .expect("the folder is there")
                .display()
        );

        let mut heard = 0;
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Ok(output) = crate::process::command(shell)
                .arg("-c")
                .arg(&line)
                .current_dir(&opened.directory)
                .output()
            else {
                assert_ne!(shell, "sh", "a Unix machine with no sh");
                continue;
            };
            heard += 1;
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                expected,
                "{shell} ran `{line}` as something else (stderr: {})",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(heard > 0);
    }

    /// The document types Crook.app declares, by the name it gives each and
    /// the type it is.
    #[cfg(unix)]
    const DOCUMENT_TYPES: [(&str, &str); 2] = [
        ("Unix executable", UNIX_EXECUTABLE),
        ("Shell script", "com.apple.terminal.shell-script"),
    ];

    /// The `Info.plist` `script/macos-app` writes into Crook.app, as it
    /// prints it.
    #[cfg(unix)]
    fn info_plist() -> String {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root")
            .join("script/macos-app");
        let output = crate::process::command("sh")
            .arg(script)
            .arg("--info-plist")
            .output()
            .expect("sh should start");
        assert!(
            output.status.success(),
            "script/macos-app --info-plist failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("the Info.plist is text")
    }

    #[test]
    #[cfg(unix)]
    fn the_app_says_it_runs_the_scripts_finder_hands_its_terminal() {
        // Launch Services hands a file only to an application that declares
        // its type, so without these the button left a script double-clicked
        // in Finder with nowhere to go. The type the button claims is the
        // first.
        let squeezed = |text: &str| text.split_whitespace().collect::<String>();
        let entries: String = DOCUMENT_TYPES
            .iter()
            .map(|(name, content_type)| {
                format!(
                    "<dict>
                        <key>CFBundleTypeName</key><string>{name}</string>
                        <key>CFBundleTypeRole</key><string>Shell</string>
                        <key>LSHandlerRank</key><string>Alternate</string>
                        <key>LSItemContentTypes</key><array><string>{content_type}</string></array>
                    </dict>"
                )
            })
            .collect();
        let declared = squeezed(&format!(
            "<key>CFBundleDocumentTypes</key><array>{entries}</array>"
        ));

        let plist = info_plist();
        assert!(
            squeezed(&plist).contains(&declared),
            "the Info.plist does not declare the document types:\n{plist}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_info_plist_is_one_macos_reads() {
        // What `plutil -lint` says of it in the script, which runs only when
        // a release is cut; here it runs on every change.
        let scratch = Scratch::new("plist");
        let path = scratch.0.join("Info.plist");
        std::fs::write(&path, info_plist()).expect("the plist is written");
        let output = crate::process::command("plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .expect("plutil should start");
        assert!(
            output.status.success(),
            "plutil refused the Info.plist: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
