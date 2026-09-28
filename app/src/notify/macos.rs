//! Posting a notification on macOS, through Notification Center.
//!
//! `UNUserNotificationCenter` is how a Mac application posts a banner —
//! `NSUserNotificationCenter` before it is deprecated — and it is a framework
//! to call rather than a program to run, so this is the one notifier that
//! speaks Objective-C, through the `objc2` bindings the rest of the graph
//! already builds for macOS.
//!
//! # Only from inside Crook.app
//!
//! Notification Center files everything under the bundle identifier of the
//! application that posts — the permission the person gave, the style they
//! chose, the banners in the list — so a process without one has nothing to
//! post as. Asking for the center from such a process is worse than useless:
//! it raises an Objective-C exception rather than returning an error, and an
//! exception that unwinds into Rust ends the process. So [`bundle_identifier`]
//! is asked first, by [`Service::here`](super::Service::here), and a
//! `NotificationCenter` is made, or leave to badge asked for, only when there
//! is one. The binary `script/install` puts on `PATH` has none, and has the
//! dock's bounce instead, and a badge the dock may or may not draw.
//!
//! # Asking with the first banner, or the first badge
//!
//! macOS asks the person whether an application may post the first time the
//! application asks for leave, and this asks with each notification rather
//! than at launch: a person who never has a pane wait behind another window
//! is never asked, and the one who is asked is asked with a reason on the
//! screen. After the first time the answer comes from what the person chose,
//! without a prompt, so asking every time also follows a change they make
//! later in System Settings, and nothing is kept here to go stale. The answer
//! arrives on a queue of the system's, and the banner is handed over from
//! there. A refusal is said once, in the log.
//!
//! The same leave covers the dock icon's badge, which the dock draws for
//! Crook.app only once it has been asked for, and a pane can wait with the
//! window in front, where nothing is posted. So the window asks too, through
//! [`super::ask_to_badge`], the first time there is a count to show, and is
//! handed the yes so that the badge set before it is set again.

/// The identifier of the bundle this process was started from — Crook.app's
/// — or `None` for a binary outside one.
///
/// In a pool of its own, because it is asked from whichever thread asks
/// first — a test's, where no run loop drains one — and what Foundation hands
/// back may wait in one.
#[cfg(target_os = "macos")]
pub fn bundle_identifier() -> Option<String> {
    objc2::rc::autoreleasepool(|_| {
        objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|identifier| identifier.to_string())
    })
}

/// `None`: bundles are macOS's, and there is none to have been started from.
#[cfg(not(target_os = "macos"))]
pub fn bundle_identifier() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
pub use center::{NotificationCenter, ask_to_badge};

/// The half that calls the framework, which is on macOS alone.
#[cfg(target_os = "macos")]
mod center {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use crookui_core::executor::Background;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString, NSUUID};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
        UNNotificationSound, UNUserNotificationCenter,
    };

    use crate::notify::{Notice, Notifier};

    /// Posts to Notification Center, with the person's leave.
    ///
    /// Made only for a process started from inside an application bundle:
    /// see the module above for why that is not checked here.
    pub struct NotificationCenter {
        /// Whether the log has been told that macOS refused, so that a person
        /// who said no is told so once and not once a question.
        refused: Arc<AtomicBool>,
    }

    impl NotificationCenter {
        /// A notifier for this process, which has a bundle identifier.
        pub fn new() -> Self {
            Self {
                refused: Arc::default(),
            }
        }
    }

    impl Default for NotificationCenter {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Notifier for NotificationCenter {
        /// Asks for leave, and posts `notice` from the answer if it is yes.
        ///
        /// Without the pool: nothing here waits. Both calls send a message to
        /// the notification daemon and return, and the answers come back on
        /// the system's own queues.
        fn post(&self, notice: Notice, _: &Background) {
            let refused = self.refused.clone();
            let answered = RcBlock::new(move |granted: Bool, error: *mut NSError| {
                if granted.as_bool() {
                    add(&notice);
                } else if !refused.swap(true, Ordering::Relaxed) {
                    log::info!(
                        "macOS does not let Crook post notifications{}; System Settings → \
                         Notifications → Crook is where that changes",
                        reason(error)
                    );
                }
            });
            UNUserNotificationCenter::currentNotificationCenter()
                .requestAuthorizationWithOptions_completionHandler(leave(), &answered);
        }
    }

    /// Asks for the leave the dock needs before it draws Crook.app's badge,
    /// and calls `allowed`, on a queue of the system's, if it is given.
    ///
    /// The whole of `leave`, not the badge alone: one prompt, the first
    /// time either is asked, covers the banners as well. A refusal is not
    /// logged here — [`NotificationCenter`] says it, the first time it has
    /// something to post — and a badge refused is one the dock leaves off.
    pub fn ask_to_badge(allowed: impl Fn() + 'static) {
        let answered = RcBlock::new(move |granted: Bool, _: *mut NSError| {
            if granted.as_bool() {
                allowed();
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(leave(), &answered);
    }

    /// What Crook asks leave for: a banner, the sound it comes with, and the
    /// badge the dock icon shows the count of waiting panes in — which, for an
    /// application with a bundle, the dock draws only with this leave.
    fn leave() -> UNAuthorizationOptions {
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge
    }

    /// Hands `notice` to Notification Center, to show now.
    fn add(notice: &Notice) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        // As the pane said it: Notification Center reads no markup, so
        // nothing is escaped, unlike the body a Linux server may read as
        // HTML.
        content.setBody(&NSString::from_str(&notice.body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        // A new identifier every time, because a request under one already
        // delivered replaces that banner, and the ones a person has not read
        // yet are ones they may still want — the coalescing is `Cooldown`'s.
        let identifier = NSUUID::UUID().UUIDString();
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            None,
        );
        let added = RcBlock::new(|error: *mut NSError| {
            if !error.is_null() {
                log::debug!(
                    "Notification Center did not take a notification{}",
                    reason(error)
                );
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&added));
    }

    /// What `error` says, for the log: `: <why>`, or nothing when there is no
    /// error to say it.
    fn reason(error: *mut NSError) -> String {
        // SAFETY: the pointer is the one a completion handler was given,
        // which is null or an `NSError` the caller keeps alive for the call.
        match unsafe { error.as_ref() } {
            Some(error) => format!(": {}", error.localizedDescription()),
            None => String::new(),
        }
    }
}
