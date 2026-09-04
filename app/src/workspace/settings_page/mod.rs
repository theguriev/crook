//! The settings page: the rail of pages, and the page it has selected.
//!
//! # A pane, like Warp's
//!
//! Settings are not a modal here. They are a **pane** — the same thing an
//! agent session lives in — which means they open in a tab of their own, sit
//! in the strip beside the work they configure, can be split next to it, and
//! close with the same close button, the same middle click and the same close
//! chord — `cmd-w`, and `ctrl-shift-w` off macOS — as anything else. That is
//! Warp's design
//! (`app/src/pane_group/pane/settings_pane.rs`, plus the per-window manager
//! that keeps at most one of them), and it is the best idea in that part of
//! Warp: the thing you are configuring stays on screen while you configure it.
//!
//! What it cost is written down where it is paid: [`PaneContent`] is now an
//! enum, [`Pane::session`] and [`Pane::status`] return `Option`s, and the two
//! row renderers each carry one branch for a row that stands for something
//! other than an agent. That is the whole bill.
//!
//! [`PaneContent`]: crate::tab::PaneContent
//! [`Pane::session`]: crate::tab::Pane::session
//! [`Pane::status`]: crate::tab::Pane::status
//!
//! # What the pane draws
//!
//! A rail down the left edge and a page beside it, which is Warp's layout: the
//! rail is a fixed column with a right border, and the content is top-centred
//! against a maximum width so a wide pane does not stretch a row of settings
//! across a metre of screen. The page's own chrome is the panel's — the fill,
//! the border that says which pane is focused, the corner radius — so this
//! file paints no background of its own and the rail is a border rather than a
//! second surface.
//!
//! There is no close button in the corner and no Escape binding. Both would be
//! a second way to do what the row's close button and the close chord already
//! do to every pane, and a settings pane is not special enough to have its
//! own.
//!
//! # What it does not have
//!
//! **Search.** Warp's is the most interesting behaviour on its settings page —
//! one field filtering the rail and the content together, with per-widget
//! keyword blobs and match counts beside each page. It needs a text input, and
//! `crookui_core` has none. At four pages and eleven controls there is also
//! nothing to find: the whole surface fits in two screens.
//!
//! **A settings-file footer.** Warp's rail ends in "Open settings file", and
//! an inline alert when that file failed to parse. Crook's file cannot fail
//! visibly — every unreadable value falls back to a default and logs a line —
//! so there is nothing to alert about, and the path is on the About page for
//! anyone who wants to open it themselves.

mod pages;
mod widgets;

use std::collections::HashMap;

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::settings::{Density, Granularity, Layout, PrimaryInfo, Subtitle};
use crate::theme::theme;

use super::action::{SettingsAction, WorkspaceAction};
use super::view::Workspace;

/// The widest the content column is allowed to get, before it is centred in
/// whatever is left.
///
/// Warp's is 800 against a 12px body; this is that, scaled to a pane that also
/// has a 160px rail in front of it. Past it a row's label and its control end
/// up so far apart that the eye loses which control belongs to which row.
const CONTENT_MAX_WIDTH: f32 = 560.;

/// The rail down the left edge.
///
/// Warp's is 200. Crook's rail holds four one-word labels rather than eleven
/// pages and two collapsible umbrellas, so it is narrower, and the number it
/// is narrower than is written here rather than in a commit message.
pub(super) const RAIL_WIDTH: f32 = 160.;

/// The inset around the content column. Warp's is 28, against a page that may
/// be 800 wide; this card's content column is 560.
const CONTENT_PADDING: f32 = 20.;

/// The space kept clear down the right of the scrolling column, so the
/// scrollbar's thumb has somewhere to be that is not on top of a switch.
///
/// The thumb rides the inside edge of the scrollable, which is inside the
/// content's padding; without this the two share the same twenty pixels and
/// the thumb crosses every segmented control on the page.
const SCROLLBAR_GUTTER: f32 = 12.;

/// One page of the settings, and one row of the rail.
///
/// Warp's order — the account, then what the application does, then what it
/// looks like, then the shortcuts, then About — with everything Crook does not
/// have removed. About stays last, because that is where every settings window
/// ever written puts it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Section {
    /// The tab strip: where it lives, what a row stands for, what a row says.
    #[default]
    Appearance,
    /// The usage chip, and therefore whether Crook talks to the network.
    Usage,
    /// The bindings, which are fixed. Read-only, and honest about it.
    Keys,
    /// Version, channel, where the settings live, and the licence.
    About,
}

impl Section {
    /// Every page, in rail order.
    pub(super) const ALL: [Self; 4] = [Self::Appearance, Self::Usage, Self::Keys, Self::About];

    /// What the rail calls it, and what the page's own heading says.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Usage => "Usage",
            Self::Keys => "Keys",
            Self::About => "About",
        }
    }
}

/// One clickable thing on the page.
///
/// The key of the mouse-state map the view holds. [`MenuState`] names its
/// fourteen handles one field at a time, which is right for a popup whose
/// controls are fixed; a page whose rows come and go with the section on
/// screen would need every field of that struct to be reachable from a
/// renderer that does not know which section it is drawing. Keying by identity
/// gives the same guarantee the struct does — one handle per control, never a
/// shared one — with the identity written once, at the call site, instead of
/// once in a declaration and once in a renderer that can drift from it.
///
/// [`MenuState`]: super::view::MenuState
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Control {
    /// A row of the rail.
    Section(Section),
    /// One half of "Tab placement".
    Layout(Layout),
    /// One half of "View as".
    Granularity(Granularity),
    /// One half of "Density".
    Density(Density),
    /// One row of "Pane title as".
    PrimaryInfo(PrimaryInfo),
    /// One row of "Additional metadata". Keyed by the value rather than by the
    /// slot, so that changing the title field — which changes *which* two of
    /// the three are offered — cannot hand a row the hover state of the option
    /// that used to be in its place.
    Subtitle(Subtitle),
    /// "Show: PR link".
    ShowPrLink,
    /// "Show: Diff stats".
    ShowDiffStats,
    /// "Show details on hover".
    ShowDetailsOnHover,
    /// "Show the usage chip".
    ShowUsageChip,
    /// "Follow the desktop".
    FollowSystemTheme,
    /// The minus of "Text size".
    FontSmaller,
    /// The plus of it.
    FontBigger,
    /// "Reset to defaults".
    ResetTabOptions,
    /// The preview in the "Current theme" row.
    ThemeRow,
    /// The row around it, which is what opens the Themes panel.
    ThemeRowButton,
}

/// Which page the rail has selected, and what the mouse is doing to each of
/// its controls.
///
/// Whether the page is *open* is not here: the settings pane's existence is
/// that answer, and a flag beside it would be a second copy of it to keep
/// true. What is here outlives the pane on purpose — closing the tab and
/// opening it again comes back to the page you were on, scrolled where you
/// left it, which is what Warp's per-window pane manager buys by holding its
/// view handle across a close.
#[derive(Default)]
pub(super) struct SettingsState {
    /// Which page the rail has selected. Not persisted to disk: where somebody
    /// was last time they changed a setting is not a preference, and a
    /// settings file that recorded it would rewrite itself on a click that
    /// changed nothing.
    pub(super) section: Section,
    /// How far the content column has been scrolled.
    pub(super) scroll: ScrollStateHandle,
    /// One mouse state per control, created the first time that control is
    /// drawn and kept for as long as the window lives.
    ///
    /// A `HashMap` behind a `RefCell` rather than a field per control: see
    /// [`Control`]. The interior mutability is what lets a render — which
    /// holds `&Workspace` — ask for the handle of a control it is about to
    /// draw for the first time.
    controls: std::cell::RefCell<HashMap<Control, MouseStateHandle>>,
}

impl SettingsState {
    /// The mouse state for one control, creating it if this is its first
    /// frame.
    pub(super) fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }

    /// Forgets every hover and press the page was holding.
    ///
    /// Called when the page closes. Every control on it is about to stop
    /// existing without seeing a hover-out, and a switch that came back
    /// believing it was hovered would light up with the pointer somewhere else
    /// entirely — the same trap the close button, the info dot and the layout
    /// switch each close for themselves.
    pub(super) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }
}

/// The whole page: the rail, and the page it has selected.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(rail(workspace))
        .with_child(Expanded::new(1., content(workspace, app)).finish())
        .finish()
}

/// The rail: the four pages, and what this build is.
///
/// No background of its own — the panel behind it already painted one — and a
/// right border instead, which is what Warp's rail is too. A filled rail
/// inside a rounded panel would also have to know the panel's corner radius to
/// avoid painting square into it.
fn rail(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for section in Section::ALL {
        column.add_child(rail_row(workspace, section, ui));
    }

    // Warp's rail ends in a button that opens the settings file. Crook's ends
    // in the one fact somebody looking at a rail wants: which build this is.
    column.add_child(Expanded::new(1., Empty::new().finish()).finish());
    column.add_child(
        Container::new(
            Text::new(
                format!(
                    "crook {} \u{b7} {}",
                    env!("CARGO_PKG_VERSION"),
                    workspace.channel()
                ),
                ui,
                10.,
            )
            .with_color(theme().text_muted)
            .finish(),
        )
        .with_padding(Padding {
            top: 8.,
            bottom: 4.,
            left: 10.,
            right: 10.,
        })
        .finish(),
    );

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_border(Border::right(1.).with_border_color(theme().border))
            .with_uniform_padding(12.)
            .finish(),
    )
    .with_width(RAIL_WIDTH)
    .finish()
}

/// One page's row in the rail.
fn row_of_rail_is_selected(workspace: &Workspace, section: Section) -> bool {
    workspace.settings_page().section == section
}

fn rail_row(
    workspace: &Workspace,
    section: Section,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let selected = row_of_rail_is_selected(workspace, section);
    let state = workspace.settings_page().control(Control::Section(section));

    Hoverable::new(state, move |mouse| {
        let (background, color) = if selected {
            (theme().overlay_3, theme().text_primary)
        } else if mouse.is_hovered() {
            (theme().overlay_1, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().text_muted)
        };

        Container::new(
            Text::new(section.label(), ui, widgets::LABEL_SIZE)
                .with_color(color)
                .finish(),
        )
        .with_padding(Padding {
            top: 6.,
            bottom: 6.,
            left: 10.,
            right: 10.,
        })
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_margin_bottom(2.)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Settings(SettingsAction::Select(section)));
    })
    .finish()
}

/// The right-hand column: a fixed heading, then the page itself, scrolling.
///
/// Warp keeps the page title inside the scroll area. Here it is above it and
/// stays put, because a pane that can be a hundred pixels tall should not have
/// to scroll to find out which page it is on.
fn content(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(centred(widgets::page_title(settings.section.label(), ui)))
            .with_child(
                Expanded::new(
                    1.,
                    Scrollable::new(
                        settings.scroll.clone(),
                        centred(pages::render(workspace, settings.section, app)),
                    )
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_padding(Padding {
        top: CONTENT_PADDING,
        bottom: CONTENT_PADDING,
        left: CONTENT_PADDING,
        // The gutter makes up the rest of it: the content still stops
        // `CONTENT_PADDING` from the panel's edge, and the thumb lives in the
        // difference.
        right: CONTENT_PADDING - SCROLLBAR_GUTTER,
    })
    .finish()
}

/// `child`, capped at [`CONTENT_MAX_WIDTH`] and centred in whatever is left.
///
/// A row of two flexible spacers rather than an [`Align`], because this goes
/// inside a [`Scrollable`] — which measures its child against an unbounded
/// height — and `Align` takes every finite axis it is offered, so it would
/// report a height of infinity and there would be nothing to scroll. A flex
/// row hugs its children's height, which is the half of `Align` this wanted.
fn centred(child: Box<dyn Element>) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(
            ConstrainedBox::new(
                // A stretching column around the child, so that what is
                // centred is the *column* and not the child's own text: a
                // heading handed straight to the row above would measure to
                // its word and end up centred over the settings it names.
                Flex::column()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(child)
                    .finish(),
            )
            .with_max_width(CONTENT_MAX_WIDTH)
            .finish(),
        )
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish()
}
