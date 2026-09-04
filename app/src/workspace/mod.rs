//! The root view: the tabs, a header, the active tab's body — and, over all
//! three when it is up, the settings page.
//!
//! Where those three sit relative to one another is [`Layout`]'s to say, and
//! it is the one setting that rearranges the window rather than a row inside
//! it. [`Layout::Vertical`] — Crook's default — puts the tabs in a panel down
//! the left edge, full window height, with the header and the body beside it;
//! [`Layout::Horizontal`] puts them in a strip across the header, with the
//! body under it. The two are mutually exclusive: [`tab_bar`] renders nothing
//! in one and [`tabs_panel`] renders nothing in the other, because both read
//! the same options and would otherwise both be listening for the same clicks.
//!
//! [`Workspace`] owns the state — the tab strip, the mouse state each control
//! keeps between renders, the handle to the usage model — and the sibling
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
mod block_list;
mod body;
mod controls;
mod header_toolbar;
mod input_element;
mod pane_output;
mod row_content;
mod settings_page;
mod tab_bar;
mod tab_menu;
mod tab_options_menu;
mod tabs_panel;
mod terminal_element;
mod text_field;
mod theme_panel;
mod theme_preview;
mod title_bar;
mod usage_chip;
mod view;

#[cfg(test)]
mod tests;

use crookui_core::icons::Lucide;

use crate::theme::theme;

pub use action::{
    OptionsAction, SettingsAction, ThemeAction, WindowAction, WorkspaceAction, WorktreeAction,
};
pub use block_list::BlockList;
pub use input_element::CommandInput;
pub use pane_output::{Keys, Output};
pub use settings_page::Section;
pub use terminal_element::TerminalElement;
pub(crate) use title_bar::caption_area;
pub use usage_chip::{UsageChip, UsageChipAction};
pub use view::{Fonts, QuitRequest, Workspace};

/// The tallest a tab is allowed to get, however few of them there are.
///
/// Warp's number. Past it a tab stops reading as one item in a row and starts
/// reading as a panel.
pub(crate) const TAB_MAX_WIDTH: f32 = 220.;

/// The square reserved for a tab's close button, drawn or not.
pub(crate) const CLOSE_BUTTON_SIZE: f32 = 16.;

/// The cross inside that square, in both layouts and in the Themes panel.
///
/// Smaller than the square it sits in, the way every icon in this interface
/// is: the button is the box, the icon is what it says.
pub(crate) const CLOSE_ICON_SIZE: f32 = 12.;

/// The dot that carries the agent's status.
pub(crate) const STATUS_DOT_SIZE: f32 = 7.;

/// The mark a settings row leads with, where a session's row carries its
/// status.
///
/// One constant for the strip, the panel and the gear button, because they are
/// the same symbol standing for the same thing and three copies of it is how
/// two of them end up different.
pub(crate) const GEAR_ICON: Lucide = Lucide::Settings;

/// Breaks `text` into lines of at most `max_chars` characters each.
///
/// A word longer than the budget gets a line of its own and overflows it,
/// because breaking inside a word would be worse and the note has no such
/// word. Counted rather than measured: measuring needs the shaper, and this
/// runs while the element tree is being built.
pub(crate) fn wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= max_chars => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// The colour that says what an agent is doing.
///
/// One function, because both layouts draw the same mark at different sizes —
/// a 7px dot in the strip, an 18px disc in the panel — and two palettes for
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
