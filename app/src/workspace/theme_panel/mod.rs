//! The Themes panel: a second side panel, and the only place a theme is made.
//!
//! # What Warp does, and what of it is here
//!
//! Warp's chooser is a **docked left panel**, 240px wide, a sibling of the
//! terminal in a row rather than a modal over it — so it pushes the work
//! aside instead of covering it, and you watch the theme land on a real shell
//! while you browse. There is no OK and no Cancel: moving the selection *is*
//! choosing, and the panel is dismissed by its own close button. Top to
//! bottom it is a header with a close button, the title "Themes" with a `+`
//! beside it, a one-line hint, a search field, and a scrolling list of preview
//! cards with names under them.
//!
//! All of that is here except the search field, and the reason is not
//! principle: Crook's text input is [`CommandInput`], which exists to send a
//! line to a shell — it is keyed by `PaneId`, its Enter is wired to `send()`,
//! and the arbitration that decides which one has the keyboard runs over the
//! panes. Reusing it outside a pane is a refactor of the input layer rather
//! than a panel feature. Warp needs a search box because it lists twenty-one
//! built-in themes plus everything a person has collected; Crook lists three
//! plus a folder, and the day that folder holds fifty this is the first thing
//! to build.
//!
//! [`CommandInput`]: super::input_element::CommandInput
//!
//! # The creator, and the image that is not there
//!
//! Warp's `+` opens a modal that makes a theme **out of a photograph**:
//! k-means the pixels, offer the five clusters as background swatches, and
//! decide everything else — see [`crate::theme::creator`] for the algorithm
//! and for what Crook replaces the photograph with. The flow here is that
//! flow, in the panel rather than in a modal, because a modal over a panel is
//! a second floating surface for a five-swatch choice.
//!
//! # Why a panel at all, when the settings page already lists themes
//!
//! Because the settings page is a *pane*, and a pane replaces the thing you
//! are theming. The panel's whole advantage is that a shell stays on screen
//! beside it: a theme is judged against real output, not against a preview
//! card. The two surfaces share one list and one write path — the settings
//! page's row opens this panel rather than listing themes a second time.

mod creator;
mod list;

pub(super) use list::ROW_HEIGHT;

use std::collections::HashMap;

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::theme::creator::Draft;
use crate::theme::theme;

use super::action::{ThemeAction, WorkspaceAction};
use super::view::Workspace;

/// The panel's width.
///
/// Warp's is 240 and the tabs panel beside it is 248; two docked panels of
/// almost the same width read as a mistake, so this is the one Crook already
/// has.
pub(super) const PANEL_WIDTH: f32 = 248.;

/// The strip at the top that carries the close button, matching the tabs
/// panel's control bar so the two line up.
const HEADER_HEIGHT: f32 = 32.;

/// The panel's own inset.
const PANEL_PADDING: f32 = 12.;

/// The title, which is Warp's word for this panel.
const TITLE: &str = "Themes";

/// What the panel is for, in one line. Warp's `SystemAgnostic` hint, verbatim —
/// it is the only one of its three that Crook can be in, having no OS-sync
/// pair to pick halves of.
const CHOOSING_HINT: &str = "Change your current theme.";

/// The same line for the creator.
const CREATING_HINT: &str =
    "A new theme, built from the colours of the one you are looking at. Pick its background.";

/// What the panel is doing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    /// Listing themes.
    #[default]
    Choosing,
    /// Making one.
    Creating,
}

/// One clickable thing in the panel.
///
/// Keyed by identity for the reason the settings page's controls are: the rows
/// come and go with what is in the themes folder, and a struct with a field
/// per control could not be written down.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Control {
    /// The × in the corner.
    Close,
    /// The `+` beside the title.
    Create,
    /// One theme's row.
    Theme(usize),
    /// One of the creator's background swatches.
    Swatch(usize),
    /// The creator's "Cancel".
    Cancel,
    /// The creator's "Create theme".
    Save,
}

/// Whether the panel is up, what it is doing, and what the mouse is doing to
/// it.
#[derive(Default)]
pub(super) struct ThemePanelState {
    /// Whether the panel is on screen.
    pub(super) open: bool,
    /// Listing themes, or making one.
    pub(super) mode: Mode,
    /// The row the keyboard is on. Not the theme in force: arrow keys move
    /// this *and* apply, so the two agree until a click on a card moves one
    /// without the other.
    pub(super) selected: usize,
    /// How far the list has been scrolled.
    pub(super) scroll: ScrollStateHandle,
    /// The theme being made, while one is.
    pub(super) draft: Option<Draft>,
    /// One mouse state per control, made on the control's first frame.
    controls: std::cell::RefCell<HashMap<Control, MouseStateHandle>>,
}

impl ThemePanelState {
    /// The mouse state for one control.
    pub(super) fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }

    /// Forgets every hover and press the panel was holding.
    ///
    /// Called when it closes and when it changes mode: every control is about
    /// to stop existing without seeing a hover-out, and the next opening would
    /// come back with a row lit under a pointer that is somewhere else.
    pub(super) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }
}

/// The whole panel.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let state = workspace.theme_panel();
    let ui = workspace.fonts().ui;

    let body = match state.mode {
        Mode::Choosing => list::render(workspace, app),
        Mode::Creating => creator::render(workspace),
    };

    ConstrainedBox::new(
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(header(workspace))
                .with_child(title_row(workspace, ui))
                .with_child(hint(state.mode, ui))
                .with_child(Expanded::new(1., body).finish())
                .finish(),
        )
        .with_background_color(theme().surface)
        .with_border(Border::right(1.).with_border_color(theme().border))
        .with_padding(Padding {
            top: 0.,
            bottom: PANEL_PADDING,
            left: PANEL_PADDING,
            right: PANEL_PADDING,
        })
        .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// The strip with the close button in it.
fn header(workspace: &Workspace) -> Box<dyn Element> {
    ConstrainedBox::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(icon_button(
                workspace,
                "\u{2715}",
                Control::Close,
                ThemeAction::ClosePanel,
            ))
            .finish(),
    )
    .with_height(HEADER_HEIGHT)
    .finish()
}

/// The title, and the `+` that makes a theme.
fn title_row(workspace: &Workspace, ui: crookui_core::fonts::FamilyId) -> Box<dyn Element> {
    let creating = workspace.theme_panel().mode == Mode::Creating;

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(if creating { "New theme" } else { TITLE }, ui, 16.)
                .with_color(theme().text_primary)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Properties::default()
                })
                .finish(),
        )
        .with_child(Expanded::new(1., Empty::new().finish()).finish());

    // Warp's `+` sits beside the title and has no label and no tooltip. It is
    // hidden while the creator is up, because the creator is what it opens.
    if !creating {
        row.add_child(icon_button(
            workspace,
            "\u{FF0B}",
            Control::Create,
            ThemeAction::StartCreating,
        ));
    }

    Container::new(row.finish()).with_margin_bottom(6.).finish()
}

/// The one line under the title.
fn hint(mode: Mode, ui: crookui_core::fonts::FamilyId) -> Box<dyn Element> {
    let text = match mode {
        Mode::Choosing => CHOOSING_HINT,
        Mode::Creating => CREATING_HINT,
    };

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start);
    for line in super::wrap(text, 30) {
        column.add_child(
            Text::new(line, ui, 11.)
                .with_color(theme().text_muted)
                .with_line_height_ratio(1.4)
                .finish(),
        );
    }

    Container::new(column.finish())
        .with_margin_bottom(10.)
        .finish()
}

/// A square button carrying one glyph.
fn icon_button(
    workspace: &Workspace,
    glyph: &'static str,
    control: Control,
    action: ThemeAction,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.theme_panel().control(control);

    Hoverable::new(state, move |mouse| {
        let (background, color) = if mouse.is_hovered() {
            (theme().overlay_2, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().text_muted)
        };

        Container::new(
            ConstrainedBox::new(
                Align::new(Text::new(glyph, ui, 12.).with_color(color).finish()).finish(),
            )
            .with_width(20.)
            .with_height(20.)
            .finish(),
        )
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Theme(action)))
    .finish()
}

/// The width a row of the panel has to work with.
pub(super) fn content_width() -> f32 {
    PANEL_WIDTH - PANEL_PADDING * 2.
}
