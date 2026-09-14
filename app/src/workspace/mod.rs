//! The root view: a header, and beside the panel down the left edge whatever
//! the panel's chosen section puts there — the tabs and the active tab's body,
//! or a section's list and its page.
//!
//! The tabs live in a panel down the left edge, the full height of the window,
//! with the header and the body beside it. There was once a second arrangement
//! — a strip across the header, which is Warp's default — and it is gone:
//! two arrangements meant every surface that touched the tabs had two answers,
//! and the panel is the one Crook is built around.
//!
//! [`Workspace`] owns the state — the tab strip, the mouse state each control
//! keeps between renders, the handles to the models behind the panes — and the
//! sibling modules own the pixels. They are free functions over `&Workspace`,
//! not views of their own, because a header that cannot hold state cannot
//! drift out of sync with the strip it draws.
//!
//! Views that belong to a feature rather than to the chrome do not live here
//! at all any more: what a plugin pins to the header is the plugin's view,
//! built by the plugin that makes it, and this module never learns its name.
//!
//! [`Workspace`]'s own action type is [`WorkspaceAction`], which carries the
//! strip's vocabulary and the options menu's side by side. A view handles
//! exactly one action type, and keeping the menu out of `TabAction` is what
//! lets the tab model stay a thing that can be tested with no window.

mod action;
mod block_list;
pub(crate) mod block_menu;
mod body;
mod controls;
mod header_toolbar;
mod input_element;
mod pane_output;
mod row_content;
pub(crate) mod section;
pub(crate) mod settings_page;
pub(crate) mod tab_context_menu;
mod tab_menu;
mod tab_options_menu;
pub(crate) mod tabs_panel;
mod terminal_element;
mod text_field;
mod theme_panel;
pub(crate) mod theme_preview;
mod title_bar;
mod view;
mod window_room;

#[cfg(test)]
mod tests;

use crate::theme::theme;

pub use action::{
    BlockAction, BlockEdge, BlockPart, OptionsAction, SearchAction, SettingsAction, Subject,
    TabMenuAction, ThemeAction, WindowAction, WorkspaceAction, WorktreeAction,
};
pub use block_list::BlockList;
pub use input_element::CommandInput;
pub use pane_output::{Keys, Output};
pub(crate) use settings_page::widgets::Category;
pub use terminal_element::TerminalElement;
pub use text_field::TextField;
pub use view::{BlockMenuState, Fonts, Opening, QuitRequest, Workspace};

/// The square reserved for a tab's close button, drawn or not.
pub(crate) const CLOSE_BUTTON_SIZE: f32 = 16.;

/// The cross inside that square, in the panel and in the Themes panel.
///
/// Smaller than the square it sits in, the way every icon in this interface
/// is: the button is the box, the icon is what it says.
pub(crate) const CLOSE_ICON_SIZE: f32 = 12.;

/// The colour that says what an agent is doing.
///
/// One function, because the same mark is drawn at more than one size — a
/// small disc on a row, a larger one on its hover card — and two palettes for
/// one meaning is how "running" ends up a different colour depending on where
/// you look at it.
pub(crate) fn status_color(status: crate::tab::AgentStatus) -> crookui_core::geometry::Color {
    use crate::tab::AgentStatus;

    match status {
        AgentStatus::Idle => theme().border,
        AgentStatus::Running => theme().accent,
        AgentStatus::NeedsInput => theme().usage_high,
        AgentStatus::Failed => theme().usage_critical,
    }
}
