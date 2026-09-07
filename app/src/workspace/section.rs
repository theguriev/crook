//! The shape a sidebar section is drawn in: a list in the panel, and the page
//! it has chosen beside it.
//!
//! A section hands back both halves at once — that is what `sidebar.section`
//! asks for — and until this file existed each section wrote its own two. The
//! Settings section and the Plugins section ended up with two of everything:
//! rows inset by ten pixels and rows inset by eight, a list that scrolled and
//! a rail that could not, a heading that stayed put and a heading that
//! scrolled away, a page centred against a measure and a page pinned to the
//! left of one.
//!
//! None of that was a decision. The settings were a **pane** when their half
//! was written — a card in the middle of the window, with a rail down its own
//! left edge — and the shape a pane wanted followed them into the sidebar
//! unexamined. The plugins page was written a commit earlier for a shape that
//! no longer existed either. So the shape is here, once, and a section says
//! what goes in it rather than how it is arranged.
//!
//! # What the pane era left behind, and why it had to go
//!
//! The settings page centred its content against [`CONTENT_MAX_WIDTH`] with a
//! flex row of two spacers. A flex measures an inflexible child **free along
//! the main axis**, so the column came back 560 wide whatever it had been
//! offered: in the 1024x640 window Crook opens, docking the Themes panel
//! leaves the body 528, and every value on the right of a settings row — the
//! theme pair, the fonts folder, the stepper's `+` — was drawn past the edge
//! of the window. A measure is a *maximum*, and the one thing the old one
//! could not do was be smaller than itself.
//!
//! [`measured`] is that maximum said in a way the layer can enforce: a
//! [`ConstrainedBox`] takes the narrower of the measure and the room it was
//! given, and an [`Align`] places the result in whatever room there was. The
//! `Align` reports its child's height on an unbounded axis, which is what
//! makes it safe inside a [`Scrollable`] — the reason the old code gave for
//! not using one.
//!
//! # Against the list, not in the middle of the room
//!
//! The measure is pinned to the left, which is where the plugins card already
//! had it. Centred was the pane's answer and it does not survive the move: the
//! page is no longer a card floating in a window, it is the half of the window
//! the list is the other half of, and centring puts a hundred pixels of
//! nothing *between* the two — a gap in the middle of the interface, with the
//! slack that should have been at the outside edge sitting where the eye
//! travels. It also made the page slide sideways whenever the Themes panel
//! docked, which is a page moving for a reason that has nothing to do with it.
//!
//! What is left goes to the right, where empty room reads as room.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::theme::theme;

use super::action::WorkspaceAction;
use super::settings_page::widgets;

/// The widest the page beside the list is allowed to get, before what is left
/// over is put around it.
///
/// Warp's is 800 against a 12px body, and 560 is what that became once the
/// page stopped being a card in the middle of a window and became one half of
/// one. It is not a new number: the settings page has held its rows to 560
/// since it had a rail of its own, and the plugins card took the same measure
/// from it when it was written. Past it a row's label and its control end up
/// so far apart that the eye loses which control belongs to which row.
const CONTENT_MAX_WIDTH: f32 = 560.;

/// The inset around that page.
const CONTENT_PADDING: f32 = 20.;

/// The space kept clear down the right of the measure, so the scrollbar's
/// thumb has somewhere to be that is not on top of a control.
///
/// The thumb rides the right edge of the *scrollable*, which is the whole
/// width of the page — so the two only meet when the measure has shrunk to
/// fill it, which is exactly the narrow window the measure was made to
/// survive. Reserved inside the measure rather than taken off the page's
/// padding, so that the title above the scroll and the rows inside it still
/// line up: they are measured the same way, so they give up the same twelve
/// pixels.
const SCROLLBAR_GUTTER: f32 = 12.;

/// The inset around the list in the panel.
///
/// Wider than the tab list's eight, and deliberately: the tab list's rows are
/// cards with their own fill, and these are labels.
pub(super) const PANEL_PADDING: f32 = 12.;

/// The gap under the field at the top of a list.
pub(super) const FIELD_GAP: f32 = 10.;

/// One row of that list: its inset, the radius of its fill, and the gap under
/// it.
///
/// Eight rather than the settings rail's ten, which is the number the tab
/// rows and the plugin rows already used and the number a row of the panel is
/// therefore expected to have.
pub(crate) const ROW_PADDING: Padding = Padding {
    top: 6.,
    bottom: 6.,
    left: 8.,
    right: 8.,
};
/// See [`ROW_PADDING`].
pub(crate) const ROW_RADIUS: f32 = 6.;
/// See [`ROW_PADDING`].
pub(crate) const ROW_GAP: f32 = 2.;

/// The gap between a row's leading mark and its label.
pub(crate) const LEADING_GAP: f32 = 8.;

/// How a row's label is lit.
///
/// Two rules, not one, and the difference is what the list is *for*. A rail of
/// pages is read for which page you are on, so its rows are lit by the
/// pointer and by the selection. A list of plugins is read for which ones are
/// running, so its rows are lit by that and the pointer does not get a vote —
/// the alternative is a list whose whole meaning changes as the pointer
/// crosses it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Emphasis {
    /// Lit while the row is the chosen one or under the pointer, muted
    /// otherwise. Every row stands for the same kind of thing and only one of
    /// them is being read.
    Quiet,
    /// Lit whatever the pointer is doing: a row that stands for something that
    /// is running.
    Lit,
    /// Muted whatever the pointer is doing: a row that stands for something
    /// switched off.
    ///
    /// The pair with [`Self::Lit`] is what the plugins list is scanned for,
    /// and it has to survive the pointer being somewhere else entirely. The
    /// only other thing that says a plugin is off is a six-pixel dot, and a
    /// list is read for what is off at the width of a word.
    Dim,
}

/// One row of the list in the panel.
///
/// A struct rather than six arguments, for the reason
/// [`Segment`](widgets::Segment) is one: the parts a caller varies are named
/// where they are set rather than counted out at a call site.
pub(crate) struct Row {
    /// What it says.
    pub(crate) label: String,
    /// A mark before the label, where the section has one to draw — the
    /// plugins list's running dot. Nothing reserves room for it, so a list
    /// without one is not indented by the space it would have taken.
    pub(crate) leading: Option<Box<dyn Element>>,
    /// Whether this is the row the page beside the list is about.
    ///
    /// The fill is always the selection's; only the label is [`Emphasis`]'s.
    pub(crate) selected: bool,
    /// How its label is lit.
    pub(crate) emphasis: Emphasis,
    /// Its own hover state, never shared with another row.
    pub(crate) state: MouseStateHandle,
    /// What clicking it does, or `None` for a row that cannot be clicked.
    pub(crate) command: Option<WorkspaceAction>,
}

/// The panel half: a field, the list under it, and whatever the section ends
/// in.
///
/// No width, no border and no background: the panel around this has all
/// three, and a filled column inside a bordered one would only have to know
/// the border to avoid painting over it.
///
/// The list scrolls. The settings rail did not, which was invisible while five
/// pages fitted and would have hidden the sixth — a plugin contributes a
/// settings page, so how many rows this holds is not a number this file gets
/// to know.
pub(crate) fn sidebar(
    field: Box<dyn Element>,
    list: Box<dyn Element>,
    scroll: ScrollStateHandle,
    footer: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(Container::new(field).with_margin_bottom(FIELD_GAP).finish())
        .with_child(
            Expanded::new(
                1.,
                Scrollable::new(scroll, list)
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .finish(),
        );

    // Below the scroll rather than inside it: what a section ends in is a fact
    // about the section, and a fact that scrolled out of sight would be one
    // nobody could rely on finding.
    if let Some(footer) = footer {
        column.add_child(footer);
    }

    Container::new(column.finish())
        .with_uniform_padding(PANEL_PADDING)
        .finish()
}

/// One row of that list, drawn.
pub(crate) fn row(row: Row, ui: FamilyId) -> Box<dyn Element> {
    let Row {
        label,
        leading,
        selected,
        emphasis,
        state,
        command,
    } = row;

    // Built inside the builder rather than beside it, because what the row
    // draws depends on the mouse state the builder is handed: the label's
    // colour is the hover's, and there is one label.
    let hoverable = Hoverable::new(state, move |mouse| {
        // The fill is the selection's and the pointer's in every list. The
        // label is not: see [`Emphasis`].
        let background = if selected {
            theme().overlay_3
        } else if mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        };
        let color = match emphasis {
            Emphasis::Lit => theme().text_primary,
            Emphasis::Dim => theme().text_muted,
            Emphasis::Quiet if selected || mouse.is_hovered() => theme().text_primary,
            Emphasis::Quiet => theme().text_muted,
        };

        let mut line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        let indented = leading.is_some();
        if let Some(leading) = leading {
            line.add_child(leading);
        }
        line.add_child(
            Container::new(
                Text::new(label, ui, widgets::LABEL_SIZE)
                    .with_color(color)
                    .finish(),
            )
            .with_margin_left(if indented { LEADING_GAP } else { 0. })
            .finish(),
        );

        Container::new(line.finish())
            .with_padding(ROW_PADDING)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_RADIUS)))
            .with_margin_bottom(ROW_GAP)
            .finish()
    });

    match command {
        Some(command) => hoverable
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(command))
            .finish(),
        None => hoverable.finish(),
    }
}

/// The window half: a title that stays put, and the page scrolling under it.
///
/// The title is above the scroll rather than in it — a body a hundred pixels
/// tall should not have to be scrolled to find out what it is a page of — and
/// it is held to the same measure as the page, so the two line up rather than
/// the heading sitting over the middle of what it names.
pub(crate) fn content(
    title: &str,
    body: Box<dyn Element>,
    scroll: ScrollStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(measured(widgets::page_title(title, ui)))
        .with_child(
            Expanded::new(
                1.,
                Scrollable::new(scroll, measured(body))
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .finish(),
        )
        .finish();

    Container::new(column)
        .with_uniform_padding(CONTENT_PADDING)
        .finish()
}

/// `child`, no wider than [`CONTENT_MAX_WIDTH`] and no wider than the room
/// there is, with whatever is left over put around it.
///
/// The [`ConstrainedBox`] is what makes the measure a maximum: it takes the
/// narrower of the two, so a body narrower than the measure gets a column that
/// fits in it rather than one that runs off the edge.
///
/// [`Align`] rather than a row of flexible spacers, which is what this was.
/// A flex measures an inflexible child free along the main axis, so the child
/// of that row was never told how much room there was and could not be smaller
/// than the measure. `Align` hands its child the room it has and falls back to
/// the child on an axis that has none — which is also what makes it safe
/// inside a [`Scrollable`], where the height is unbounded.
fn measured(child: Box<dyn Element>) -> Box<dyn Element> {
    Align::new(
        Container::new(
            ConstrainedBox::new(
                // A stretching column around the child, so that what is
                // measured is the *column* and not the child's own text: a
                // heading handed straight to this would measure to its word
                // and end up placed over the middle of the settings it names.
                Flex::column()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(child)
                    .finish(),
            )
            .with_max_width(CONTENT_MAX_WIDTH)
            .finish(),
        )
        .with_margin_right(SCROLLBAR_GUTTER)
        .finish(),
    )
    .top_left()
    .finish()
}
