//! The panel that says why a row's dot is what it is.
//!
//! A row is amber, and nothing on it says whether the agent asked for an
//! answer, the shell rang, a person marked it to come back to, or a command
//! ended under an agent that never said it stopped. Every one of those is a
//! fact the session already holds — see [`AgentSession`] — and this is the
//! one place they are read out in words. herdr answers the same question with
//! `herdr agent explain <target>`; Crook has no socket to ask over, so the
//! answer is an entry on the row's own menu, "Why this status", contributed by
//! [`crook/tabs`](crate::plugins::tabs).
//!
//! # Where it hangs
//!
//! Off the tab menu's corner, where the worktree list hangs, and for the same
//! reason that list is not a popup: a second popup would float above the
//! menu's underlay while the underlay ate the press meant to dismiss it. It is
//! read-only — no rows, no field, nothing to press — so it takes no key but
//! Escape, which takes it down and leaves the menu standing, and the arrows
//! keep walking the menu under it.
//!
//! # Nothing ticks
//!
//! "2 minutes ago" is a subtraction done when the panel is drawn, from an
//! [`Instant`] the session wrote when the report arrived. The panel is drawn
//! when something else redraws the window, and a person who leaves it up
//! reads the age it had when they opened it — which is what they asked, and
//! what keeps a timer out of a window that has none.

use std::time::Instant;

use crookui_core::elements::Paragraph;
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::plugins::store::ago;
use crate::tab::{AgentSession, AgentStatus, Attention, Pane, StatusSource};
use crate::theme::theme;

use super::tab_menu::{LABEL_SIZE, MENU_WIDTH, PATH_SIZE, ROW_INSET};
use super::view::Workspace;

/// The popup's corner radius, which is every other menu's in this application.
const PANEL_RADIUS: f32 = 6.;

/// The whole panel: a heading, and one sentence per fact.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let session = workspace
        .menu_target()
        .and_then(|(_, pane)| workspace.tabs().pane(pane))
        .map(Pane::session);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(heading("Why this status", ui));

    // A menu is only ever up on a row that exists, so the branch with no
    // session is a frame between a pane closing and the menu hearing of it.
    if let Some(session) = session {
        for line in lines(session, Instant::now()) {
            column.add_child(sentence(line, ui));
        }
    }

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PANEL_RADIUS)))
            .with_vertical_padding(6.)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish()
}

/// What the panel says about a session, one sentence per line, at `now`.
///
/// Separated from the drawing so that the words can be read without a frame:
/// the sentences are the feature, and the elements around them are the
/// worktree menu's. The order is the order a person asks — what the dot
/// says and who said it, then what the agent is doing, then who asked for a
/// look, then what the shell is up to.
pub(super) fn lines(session: &AgentSession, now: Instant) -> Vec<String> {
    let status = session.status.label();
    let mut lines = vec![match session.source {
        StatusSource::NoReport => format!("Status: {status} — no report yet."),
        StatusSource::Agent(at) => {
            format!("Status: {status} — the agent said so {}.", since(at, now))
        }
        StatusSource::CommandEnded(at) => format!(
            "Status: {status} — the command ended {} and took the agent's last report back.",
            since(at, now)
        ),
    }];

    // The one case the dot and the status disagree, and the case most worth
    // a sentence: an amber dot on a row whose agent said nothing of the sort.
    let shown = session.shown_status();
    if shown != session.status {
        lines.push(format!(
            "The dot says {}, because the row asks for a look.",
            shown.label()
        ));
    }

    if let Some(title) = &session.derived_title {
        lines.push(format!("The agent calls its work “{title}”."));
    }
    if let Some(message) = &session.message {
        lines.push(format!("It asks: {message}"));
    }

    match session.attention {
        Some(Attention::Bell) => {
            lines.push("Attention: the bell rang while nobody was looking.".to_owned());
        }
        Some(Attention::StatusChange) => {
            lines.push("Attention: the status changed while nobody was looking.".to_owned());
        }
        None => {}
    }
    if session.marked {
        lines.push("Marked as waiting by you.".to_owned());
    }
    if session.attention.is_none() && !session.marked {
        lines.push(match session.status {
            AgentStatus::NeedsInput => "Nothing else asks for a look.".to_owned(),
            _ => "Nothing asks for a look.".to_owned(),
        });
    }

    lines.push(match &session.running_command {
        Some(command) => format!("The shell is running {command}."),
        None => "The shell is at a prompt.".to_owned(),
    });

    lines
}

/// "just now", "2 minutes ago": how long before `now` the instant was.
///
/// Saturating, because an `Instant` written on one thread and read on
/// another can be a hair in the future, and a panic in a menu over a
/// nanosecond would be the wrong answer to it.
fn since(at: Instant, now: Instant) -> String {
    ago(now.saturating_duration_since(at).as_secs())
}

/// The panel's title, in the worktree list's own heading style.
fn heading(title: &'static str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(title, ui, PATH_SIZE)
            .with_color(theme().text_muted)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Properties::default()
            })
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .with_margin_top(2.)
    .finish()
}

/// One fact, wrapped to the panel's width.
fn sentence(text: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text, ui, LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}
