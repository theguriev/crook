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
//! Restoring turned on in such a window brings a line back, even one that
//! was dismissed, since the switch asks for tabs this window will not write.
//! What it says then is narrower: see [`SAID_WHEN_TURNED_ON`]. A window whose
//! keeper has closed by then takes the session in its place and says
//! nothing; see `Workspace::keep_the_session_if_its_keeper_closed`.
//!
//! A launcher's window says nothing. It never came back with the tabs, and a
//! file manager's "Open terminal here" is not where anybody expects them.

use crookui_core::elements::MouseStateHandle;
use crookui_core::prelude::*;

use super::action::WorkspaceAction;
use super::crash_note;
use super::settings_page::widgets;
use super::view::Workspace;

/// What the line says when the window opens while another one keeps the
/// session, with restoring tabs on.
pub(crate) const SAID: &str = "Another Crook window keeps your tabs. This one opened fresh, and its tabs will not be remembered.";

/// What the line says when restoring tabs is turned on in this window while
/// another one keeps the session.
///
/// Not [`SAID`], which would be claiming more than this window knows. No
/// window reads another's settings once it is open, so the window that keeps
/// the session goes on with restoring as it was there. When this window
/// opened with restoring off, that window most likely has it off too, and
/// writes no tabs at all until it is turned on there as well.
pub(crate) const SAID_WHEN_TURNED_ON: &str = "These tabs will not be remembered. Another Crook window's will, once Bring the tabs back is on there too.";

/// The line, while it is up: what it says, and the state of its one button.
///
/// On the workspace rather than in the element tree, as every mouse state
/// is: the tree is rebuilt on every frame, and a press is half a gesture.
pub(crate) struct SessionNote {
    said: &'static str,
    dismiss: MouseStateHandle,
}

impl SessionNote {
    /// The line saying `said`, [`SAID`] or [`SAID_WHEN_TURNED_ON`].
    pub(crate) fn saying(said: &'static str) -> Self {
        Self {
            said,
            dismiss: MouseStateHandle::default(),
        }
    }
}

/// The line, while the window leaves the session to another one and nobody
/// has dismissed it.
pub(super) fn render(workspace: &Workspace) -> Option<Box<dyn Element>> {
    let note = workspace.session_note()?;
    let ui = workspace.fonts().ui;
    Some(crash_note::line(
        note.said,
        [widgets::text_button(
            "Dismiss",
            Some(WorkspaceAction::DismissSessionNote),
            note.dismiss.clone(),
            ui,
        )],
        ui,
    ))
}
