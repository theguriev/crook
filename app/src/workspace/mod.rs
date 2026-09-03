//! The root view: a header, and the active tab's body under it.
//!
//! [`Workspace`] owns the state — the tab strip, the mouse state each control
//! keeps between renders, the handle to the usage model — and the four sibling
//! modules own the pixels. They are free functions over `&Workspace`, not
//! views of their own, because a header that cannot hold state cannot drift
//! out of sync with the strip it draws.
//!
//! The one exception is [`UsageChip`], which *is* a view. It has its own
//! action type, and it observes the usage model itself so a reading that lands
//! repaints a pill rather than the whole header.
//!
//! [`Workspace`]'s own action type is [`WorkspaceAction`], which carries the
//! strip's vocabulary and the options menu's side by side. A view handles
//! exactly one action type, and keeping the menu out of `TabAction` is what
//! lets the tab model stay a thing that can be tested with no window.

mod action;
mod body;
mod header_toolbar;
mod tab_bar;
mod tab_options_menu;
mod usage_chip;
mod view;

#[cfg(test)]
mod tests;

pub use action::{OptionsAction, WorkspaceAction};
pub use usage_chip::{UsageChip, UsageChipAction};
pub use view::{Fonts, QuitRequest, Workspace};

/// The tallest a tab is allowed to get, however few of them there are.
///
/// Warp's number. Past it a tab stops reading as one item in a row and starts
/// reading as a panel.
pub(crate) const TAB_MAX_WIDTH: f32 = 220.;

/// The square reserved for a tab's close button, drawn or not.
pub(crate) const CLOSE_BUTTON_SIZE: f32 = 16.;

/// The dot that carries the agent's status.
pub(crate) const STATUS_DOT_SIZE: f32 = 7.;
