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
//! It needs no bundle and no permission. The dock tile belongs to the running
//! application, and a binary started from a shell has one as surely as
//! Crook.app does, so this is the half of "a waiting agent reaches you
//! outside the window" that a copy outside the app gets as well as the bounce.

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
