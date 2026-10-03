//! The bell's sound: the system's own alert, as every other application on
//! the machine plays it.
//!
//! On macOS `NSBeep`, which plays the alert sound chosen in System Settings →
//! Sound and stays silent when alerts are muted; on Windows `MessageBeep`,
//! the same for the Sounds control panel's default beep. Linux has no one
//! call, so the freedesktop sound theme's `bell` is played through
//! `canberra-gtk-play`, which honours the desktop's sound settings, and
//! failing that the theme's file through `paplay`; with neither installed the
//! bell is silent, and the mark on a pane is all it does.
//!
//! Switched on from the Shell settings' **Audible bell**.

use std::time::Duration;

/// How long after one sound another is not played: see
/// `Workspace::sound_the_bell`.
pub const QUIET: Duration = Duration::from_millis(150);

/// Plays the alert sound, without waiting for it.
pub fn sound() {
    platform::sound();
}

#[cfg(target_os = "macos")]
mod platform {
    // Declared rather than depended on: one AppKit function, in a framework
    // the window already links.
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSBeep();
    }

    pub fn sound() {
        // SAFETY: `NSBeep` takes nothing, returns nothing and may be called
        // from any thread.
        unsafe { NSBeep() }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    /// The freedesktop sound theme's bell, where distributions install it.
    const BELL_FILE: &str = "/usr/share/sounds/freedesktop/stereo/bell.oga";

    pub fn sound() {
        let played = crate::process::command("canberra-gtk-play")
            .args(["--id=bell", "--description=Crook bell"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|child| crate::process::reap(child, "canberra-gtk-play"));
        if played.is_ok() || !std::path::Path::new(BELL_FILE).is_file() {
            return;
        }
        match crate::process::command("paplay")
            .arg(BELL_FILE)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => crate::process::reap(child, "paplay"),
            Err(why) => log::debug!("nothing could sound the bell: {why}"),
        }
    }
}

#[cfg(windows)]
mod platform {
    /// winuser.h `MB_OK`: the default beep.
    const MB_OK: u32 = 0;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBeep(kind: u32) -> i32;
    }

    pub fn sound() {
        // SAFETY: `MessageBeep` takes a sound's number and plays it
        // asynchronously; it touches no memory of this process's.
        unsafe {
            MessageBeep(MB_OK);
        }
    }
}
