//! The line under the header that says another Crook window keeps the
//! session.
//!
//! An ordinary window opened while another one is open does not come back
//! with the last tabs and does not write them; see [`crate::session`] for
//! why. A person who launched Crook expecting their tabs gets an empty window,
//! and the tabs they open in it will not be there at the next launch. Both
//! are surprises that need explaining. The line says so once, beside the
//! crash note, in the same shape, and stays until it is dismissed.
//!
//! A launcher's window says nothing. It never came back with the tabs, and a
//! file manager's "Open terminal here" is not where anybody expects them.

use crookui_core::elements::MouseStateHandle;
use crookui_core::prelude::*;

use super::action::WorkspaceAction;
use super::crash_note;
use super::settings_page::widgets;
use super::view::Workspace;

/// What the line says.
pub(crate) const SAID: &str = "Another Crook window keeps your tabs. This one opened fresh, and its tabs will not be remembered.";

/// The state of the line's one button, while the line is up.
///
/// On the workspace rather than in the element tree, as every mouse state
/// is: the tree is rebuilt on every frame, and a press is half a gesture.
#[derive(Default)]
pub(crate) struct SessionNote {
    dismiss: MouseStateHandle,
}

/// The line, while the window leaves the session to another one and nobody
/// has dismissed it.
pub(super) fn render(workspace: &Workspace) -> Option<Box<dyn Element>> {
    let note = workspace.session_note()?;
    let ui = workspace.fonts().ui;
    Some(crash_note::line(
        SAID,
        [widgets::text_button(
            "Dismiss",
            Some(WorkspaceAction::DismissSessionNote),
            note.dismiss.clone(),
            ui,
        )],
        ui,
    ))
}
