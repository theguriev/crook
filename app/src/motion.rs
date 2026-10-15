//! Whether the desktop asks for less motion, which is what the Appearance
//! page's "Motion" does as until a person chooses otherwise.
//!
//! Each desktop keeps the answer in a switch of its own. On macOS it is
//! System Settings → Accessibility → Display → Reduce motion, read as
//! `NSWorkspace`'s `accessibilityDisplayShouldReduceMotion`. On Windows it
//! is Settings → Accessibility → Visual effects → Animation effects, read as
//! `SPI_GETCLIENTAREAANIMATION`. On Linux it is GNOME's
//! `org.gnome.desktop.interface enable-animations`, read through
//! `gsettings`, which is the switch GTK itself obeys; a desktop that keeps
//! no such key — or a machine with no `gsettings` — answers nothing, and
//! nothing is taken as a desktop that asks for nothing.
//!
//! Asked when the window opens and again whenever the settings are shown,
//! never on a timer: a person who has just switched it is likely to look at
//! the settings next to see whether Crook heard, and nobody switches it
//! often enough to be worth a poll.

/// Whether the desktop asks for less motion, or `None` where it cannot be
/// asked.
///
/// Blocking — on Linux it runs a program — so it is asked on the pool and
/// never on the frame.
pub fn desktop_reduces_motion() -> Option<bool> {
    platform::reduces_motion()
}

/// What `gsettings get org.gnome.desktop.interface enable-animations`
/// printed, as whether the desktop asks for less motion: animations off is
/// less motion.
pub fn reduced_by_gsettings(printed: &str) -> Option<bool> {
    match printed.trim() {
        "true" => Some(false),
        "false" => Some(true),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};

    pub fn reduces_motion() -> Option<bool> {
        objc2::rc::autoreleasepool(|_| {
            // Looked up rather than linked against: AppKit is the window's,
            // and the class is in it on every macOS Crook runs on.
            let class = AnyClass::get(c"NSWorkspace")?;
            // SAFETY: `sharedWorkspace` takes nothing and returns the shared
            // workspace, which AppKit lets any thread use.
            let workspace: Option<Retained<AnyObject>> =
                unsafe { msg_send![class, sharedWorkspace] };
            let workspace = workspace?;
            // SAFETY: a `BOOL` property with no arguments, on the object the
            // class method above returned, present since macOS 10.12.
            let reduce: bool =
                unsafe { msg_send![&*workspace, accessibilityDisplayShouldReduceMotion] };
            Some(reduce)
        })
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    /// winuser.h `SPI_GETCLIENTAREAANIMATION`: whether animations inside
    /// windows are on, which is what "Animation effects" switches.
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, value: *mut c_void, ini: u32) -> i32;
    }

    pub fn reduces_motion() -> Option<bool> {
        let mut animates: i32 = 1;
        // SAFETY: this action writes one `BOOL` through `value`, which points
        // at a live `i32` of this frame; it reads nothing else.
        let read = unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animates).cast(), 0)
        };
        (read != 0).then_some(animates == 0)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::process::Stdio;

    pub fn reduces_motion() -> Option<bool> {
        let output = crate::process::command("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .inspect_err(|why| log::debug!("gsettings could not be asked about motion: {why}"))
            .ok()?;
        if !output.status.success() {
            return None;
        }
        super::reduced_by_gsettings(&String::from_utf8_lossy(&output.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animations_off_is_less_motion_and_anything_else_is_no_answer() {
        assert_eq!(reduced_by_gsettings("true\n"), Some(false));
        assert_eq!(reduced_by_gsettings("false\n"), Some(true));
        // A key this schema does not have prints nothing on stdout; an
        // older or newer one might print anything at all.
        assert_eq!(reduced_by_gsettings(""), None);
        assert_eq!(reduced_by_gsettings("@mb nothing"), None);
    }
}
