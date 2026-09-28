//! The count on the application's icon in the macOS dock.
//!
//! The badge Mail puts its unread count in, carrying the number of panes
//! waiting for a person: the one place that number is on screen while the
//! window is minimised, on another Space, or behind something else. It is the
//! dock tile's `badgeLabel`, which AppKit draws and winit has no word for, so
//! it is set here, where the rest of the platform is — through the 0.6
//! bindings rather than winit's 0.5 ones, because they are the ones the rest
//! of the graph builds and one object answers both.
//!
//! # Drawn with the person's leave
//!
//! Setting the label is not the same as the dock drawing it. Since macOS 12,
//! by the accounts of the applications that ran into it, an application that
//! has a bundle identifier is badged only once it has asked Notification
//! Center for leave to badge, and from then on only while the Badges switch
//! under System Settings → Notifications says so; until it has asked, a label
//! it sets is dropped without an error. Asking is the application's: it knows
//! whether there is a bundle to ask as, and this module only draws. So the
//! application asks the first time it has a count to show, and once the
//! answer is yes has [`show_again`] called, because a label set before the
//! leave may not be drawn after it until it is set again.
//!
//! A binary started from a shell has no bundle identifier and cannot ask.
//! The badge is set for it all the same; whether the dock draws it is not
//! established — nobody has watched it happen on a Mac.

/// What the badge says for `waiting` panes: the number, or no badge at all.
///
/// Nothing at zero rather than a "0". A badge is a thing somebody notices,
/// and an application with nothing waiting has nothing to be noticed for.
pub(super) fn label(waiting: usize) -> Option<String> {
    (waiting > 0).then(|| waiting.to_string())
}

/// Puts `label` on the dock icon as its badge, or takes the badge off for
/// `None`.
///
/// On the main thread, which is where the event loop hands it over and the
/// only thread AppKit's application object may be asked anything on. The dock
/// redraws the tile itself when its label changes.
#[cfg(target_os = "macos")]
pub(super) fn set_badge(label: Option<&str>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    let Some(main_thread) = MainThreadMarker::new() else {
        log::debug!("the dock badge was asked for off the main thread, and left as it was");
        return;
    };
    let label = label.map(NSString::from_str);
    NSApplication::sharedApplication(main_thread)
        .dockTile()
        .setBadgeLabel(label.as_deref());
}

/// Nothing: off macOS there is no dock tile to badge, and
/// [`Proxy::set_badge`](super::Proxy::set_badge) sends nothing that gets
/// here.
#[cfg(not(target_os = "macos"))]
pub(super) fn set_badge(_: Option<&str>) {}

/// Sets the badge the dock icon has on it again, for the dock to draw what it
/// may have dropped before the person gave leave; nothing when there is none.
///
/// Off and back on, since AppKit may take a label equal to the one it holds
/// as no change to pass on. Both in one turn of the main thread, so the dock
/// is not left a frame without it.
#[cfg(target_os = "macos")]
pub(super) fn show_again() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else {
        log::debug!("the dock badge was asked to show again off the main thread, and was not");
        return;
    };
    let tile = NSApplication::sharedApplication(main_thread).dockTile();
    if let Some(label) = tile.badgeLabel() {
        tile.setBadgeLabel(None);
        tile.setBadgeLabel(Some(&label));
    }
}

/// Nothing, as [`set_badge`] is nothing off macOS.
#[cfg(not(target_os = "macos"))]
pub(super) fn show_again() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_is_the_count_and_there_is_none_at_zero() {
        assert_eq!(label(0), None);
        assert_eq!(label(1).as_deref(), Some("1"));
        assert_eq!(label(12).as_deref(), Some("12"));
    }
}
