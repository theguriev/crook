//! The line under the header that says the last run ended in a panic.
//!
//! One line, drawn only in the window after a crash and only until somebody
//! answers it: *Show* opens the folder the report is in, and *Dismiss* takes
//! the line down. Either one renames the report to its seen name — see
//! [`crate::diagnostics::crash`] — so the next window says nothing about a
//! crash somebody has already been told of.
//!
//! It is under the header rather than in a corner or over the work, because
//! it is not urgent and it is not in the way: the header is the one strip of
//! the window a person's eye crosses on the way to everything else, and a line
//! beside it moves the body down by its height rather than covering any of
//! it. The body gets that height back when the line goes.
//!
//! The window's own and not a plugin's, for the reason the session file's
//! problem under the tab list is the window's: the report is a fact about
//! this process's last run, and there is no slot a plugin could be handed to
//! put a line there without also handing every installed plugin a banner
//! across the top of the window.

use std::path::{Path, PathBuf};

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::theme::theme;

use super::action::{CrashNoteAction, WorkspaceAction};
use super::settings_page::widgets;
use super::view::Workspace;

/// What the line says.
pub(crate) const SAID: &str = "Crook stopped unexpectedly last time.";

/// The report the line is about, and the state of its two buttons.
///
/// On the workspace rather than in the element tree for the reason every
/// mouse state is: the tree is rebuilt on every frame, and a press is half a
/// gesture.
pub(crate) struct CrashNote {
    /// The report, where it is now: its name changes when it is seen.
    report: PathBuf,
    show: MouseStateHandle,
    dismiss: MouseStateHandle,
}

impl CrashNote {
    /// A line about `report`.
    pub(crate) fn new(report: PathBuf) -> Self {
        Self {
            report,
            show: MouseStateHandle::default(),
            dismiss: MouseStateHandle::default(),
        }
    }

    /// The report, where it is now.
    pub(crate) fn report(&self) -> &Path {
        &self.report
    }

    /// Records that the report has been looked at, by renaming it.
    ///
    /// A folder that will not take the rename is a line in the log and the
    /// same line in the next window, which is the right way round: a report
    /// that could not be marked is one nobody has been told about for sure.
    pub(crate) fn mark_seen(&mut self) {
        match crate::diagnostics::crash::mark_seen(&self.report) {
            Ok(seen) => self.report = seen,
            Err(error) => log::warn!("could not mark {} as seen: {error}", self.report.display()),
        }
    }
}

/// The line, when there is a report to be told of.
pub(super) fn render(workspace: &Workspace) -> Option<Box<dyn Element>> {
    let note = workspace.crash_note()?;
    let ui = workspace.fonts().ui;

    let row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            // The flexible child, so it is the sentence that gives way in a
            // narrow window and never the two buttons after it.
            Expanded::new(
                1.,
                Text::new(SAID, ui, widgets::LABEL_SIZE)
                    .with_color(theme().text_primary)
                    .with_ellipsis(Cut::End)
                    .finish(),
            )
            .finish(),
        )
        .with_child(widgets::text_button(
            "Show",
            Some(WorkspaceAction::CrashNote(CrashNoteAction::Show)),
            note.show.clone(),
            ui,
        ))
        .with_child(
            Container::new(widgets::text_button(
                "Dismiss",
                Some(WorkspaceAction::CrashNote(CrashNoteAction::Dismiss)),
                note.dismiss.clone(),
                ui,
            ))
            .with_margin_left(6.)
            .finish(),
        )
        .finish();

    Some(
        Container::new(row)
            .with_padding(Padding {
                top: 4.,
                left: 12.,
                bottom: 6.,
                right: 10.,
            })
            // The header's own ground, so the line reads as the header's
            // second row rather than as a strip laid over the work.
            .with_background_color(theme().surface)
            .finish(),
    )
}
