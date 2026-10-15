//! AppKit's word that a window has finished going fullscreen, which winit
//! does not pass on.
//!
//! The fullscreen a window opens in on macOS is asked for until the window
//! has it — see [`FullscreenAsked`](super::placement::FullscreenAsked) — and
//! nothing winit says tells "has it" from "asked for it". winit takes the
//! window as fullscreen from the moment it asks, before macOS has answered,
//! and forgets it again only when the answer is no; and a window's size says
//! nothing either, since one opened maximised is already as wide as its
//! monitor when it asks. AppKit posts `NSWindowDidEnterFullScreenNotification`
//! once the window is there and not before, so that is what is listened for —
//! only while the fullscreen is still being asked for, and through the 0.6
//! bindings for the reason [`dock`](super::dock) gives.

#[cfg(target_os = "macos")]
pub(super) use macos::FullscreenEntered;

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2_app_kit::{NSView, NSWindowDidEnterFullScreenNotification};
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// Listens for one window entering fullscreen, from when it is made until
    /// it is dropped.
    pub(in crate::windowing) struct FullscreenEntered {
        /// Set by the notification, which may come on any thread AppKit posts
        /// it from — the main one, for a window — so an atomic rather than a
        /// cell.
        entered: Arc<AtomicBool>,
        /// What the notification centre hands back, to stop listening with.
        observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl FullscreenEntered {
        /// Starts listening for `window` entering fullscreen: before it is
        /// asked to, so that no answer can come first. `None` when its
        /// `NSWindow` cannot be reached, which leaves the asking to stop on
        /// its own clock.
        pub(in crate::windowing) fn watch(window: &winit::window::Window) -> Option<Self> {
            let handle = window.window_handle().ok()?;
            let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
                return None;
            };
            // SAFETY: winit's AppKit handle is the window's content view,
            // alive for as long as `window` is, and this runs on the main
            // thread, where winit creates and drives its windows.
            let view = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
            let Some(ns_window) = view.window() else {
                log::debug!("the window's NSWindow could not be reached to hear it go fullscreen");
                return None;
            };
            Some(Self::watching(&ns_window))
        }

        /// Starts listening for the notification AppKit posts when `object`
        /// has entered fullscreen.
        fn watching(object: &AnyObject) -> Self {
            let entered = Arc::new(AtomicBool::new(false));
            let block = RcBlock::new({
                let entered = entered.clone();
                move |_: NonNull<NSNotification>| entered.store(true, Ordering::Relaxed)
            });
            // SAFETY: the name is AppKit's own constant. `object` is only
            // compared with the sender of each notification. With no queue,
            // the block runs on the thread that posts, and all it touches is
            // an atomic behind an `Arc`, so it may run on any of them.
            let observer = unsafe {
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(NSWindowDidEnterFullScreenNotification),
                    Some(object),
                    None,
                    &block,
                )
            };
            Self { entered, observer }
        }

        /// Whether AppKit has said the window entered fullscreen since this
        /// started listening.
        pub(in crate::windowing) fn entered(&self) -> bool {
            self.entered.load(Ordering::Relaxed)
        }
    }

    impl Drop for FullscreenEntered {
        fn drop(&mut self) {
            let observer: &AnyObject = (*self.observer).as_ref();
            // SAFETY: `observer` is what the same centre handed back when the
            // block was added, which is what it takes to remove it.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer) };
        }
    }

    #[cfg(test)]
    mod tests {
        use objc2::runtime::NSObject;

        use super::*;

        /// Posts AppKit's "entered fullscreen" with `sender` as the window.
        fn post_entered(sender: &AnyObject) {
            // SAFETY: AppKit's own name, posted with no user info.
            unsafe {
                NSNotificationCenter::defaultCenter().postNotificationName_object(
                    NSWindowDidEnterFullScreenNotification,
                    Some(sender),
                )
            };
        }

        #[test]
        fn only_the_window_listened_for_entering_fullscreen_is_heard() {
            let window = NSObject::new();
            let another = NSObject::new();
            let heard = FullscreenEntered::watching(&window);
            assert!(!heard.entered(), "heard before anything was posted");

            post_entered(&another);
            assert!(
                !heard.entered(),
                "another window's fullscreen was taken for this one's"
            );

            post_entered(&window);
            assert!(heard.entered());

            // No longer listened for once dropped: a notification after it
            // reaches nothing.
            let flag = heard.entered.clone();
            flag.store(false, Ordering::Relaxed);
            drop(heard);
            post_entered(&window);
            assert!(
                !flag.load(Ordering::Relaxed),
                "still listening after it was dropped"
            );
        }
    }
}

/// Nothing to listen for off macOS, where the fullscreen a window opens in
/// is not asked for again: an enum with no values, since none is ever made.
#[cfg(not(target_os = "macos"))]
pub(super) enum FullscreenEntered {}

#[cfg(not(target_os = "macos"))]
impl FullscreenEntered {
    /// Nothing: see the type.
    pub(super) fn watch(_: &winit::window::Window) -> Option<Self> {
        None
    }

    /// Never asked, since there is never one to ask.
    pub(super) fn entered(&self) -> bool {
        match *self {}
    }
}
