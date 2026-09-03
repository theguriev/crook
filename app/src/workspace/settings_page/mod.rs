//! The settings page: a card over the window, with a rail of pages down its
//! left edge.
//!
//! # Why a card and not a pane
//!
//! Warp's settings are a *pane*: the same machinery a terminal session runs
//! in, so settings can be split beside the thing being configured, dragged,
//! maximised, and kept — one per window — with its scroll position and search
//! query intact across a close and a reopen. It is the best idea in that part
//! of Warp and it is the one thing here that deliberately does not copy it.
//!
//! The reason is Crook's premise. A tab is one agent's workspace and a pane is
//! one agent session; the tabs panel lists panes, gives each a status dot, a
//! branch and a working directory, and the whole strip is a list of what is
//! being worked on. A settings pane would have to be a second kind of pane —
//! one with no session, no directory, no status — and every row renderer,
//! every granularity rule and every git lookup in `row_content` would grow a
//! "unless it is the settings one" branch. That is a real cost paid so that
//! settings can be split beside a terminal Crook does not have yet.
//!
//! So it is a modal card, dismissed by clicking outside it, by the close
//! button, or with Escape. What it keeps from Warp is everything that is not
//! about being a pane: the rail of pages, the category headings with a rule
//! between them, the row anatomy, apply-on-click with no Save button, the
//! reset button that doubles as the modified indicator, and inert rows drawn
//! greyed rather than dropped.
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
use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::settings::{Density, Granularity, Layout, PrimaryInfo, Subtitle};
use crate::theme::THEME;

use super::action::{SettingsAction, WorkspaceAction};
use super::view::Workspace;

/// The card's size, when the window has room for it.
///
/// A window smaller than this gets a card the size of the window instead —
/// [`ConstrainedBox`] clamps a fixed size against the parent's maximum — so
/// there is no window in which the page hangs off the edge of the screen.
const CARD_WIDTH: f32 = 720.;
const CARD_HEIGHT: f32 = 480.;

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

/// The card's corner radius, matching the body panel's.
const CARD_RADIUS: f32 = 10.;

/// The size of the close button's glyph slot.
const CLOSE_BUTTON_SIZE: f32 = 20.;

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
    /// The × in the corner.
    Close,
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
    /// "Reset to defaults".
    ResetTabOptions,
}

/// Whether the page is up, which page it is on, and what the mouse is doing to
/// each of its controls.
#[derive(Default)]
pub(super) struct SettingsState {
    /// Whether the card is on screen.
    pub(super) open: bool,
    /// Which page the rail has selected. Not persisted: where somebody was
    /// last time they changed a setting is not a preference, and a settings
    /// file that recorded it would rewrite itself on a click that changed
    /// nothing.
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

/// The whole card, ready to be handed to a [`Dismiss`].
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    Align::new(
        ConstrainedBox::new(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_child(rail(workspace))
                    .with_child(Expanded::new(1., content(workspace, app)).finish())
                    .finish(),
            )
            .with_background_color(THEME.surface_raised)
            .with_border(Border::all(1.).with_border_color(THEME.border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
            .finish(),
        )
        .with_width(CARD_WIDTH)
        .with_height(CARD_HEIGHT)
        .finish(),
    )
    .finish()
}

/// The rail: a heading, the four pages, and what this build is.
fn rail(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Text::new("Settings", ui, widgets::LABEL_SIZE)
                    .with_color(THEME.text_muted)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Properties::default()
                    })
                    .finish(),
            )
            .with_padding(Padding {
                top: 4.,
                bottom: 12.,
                left: 10.,
                right: 10.,
            })
            .finish(),
        );

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
                    "crook {} · {}",
                    env!("CARGO_PKG_VERSION"),
                    workspace.channel()
                ),
                ui,
                10.,
            )
            .with_color(THEME.text_muted)
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
            .with_background_color(THEME.surface)
            .with_border(Border::right(1.).with_border_color(THEME.border))
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
            (THEME.overlay_3, THEME.text_primary)
        } else if mouse.is_hovered() {
            (THEME.overlay_1, THEME.text_primary)
        } else {
            (Color::TRANSPARENT, THEME.text_muted)
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

/// The right-hand column: a fixed header, then the page itself, scrolling.
///
/// Warp keeps the page title inside the scroll area, because a pane has no
/// chrome to put it in and the pane header already names the pane. A card has
/// chrome — it has to carry a close button somewhere — so the title shares
/// that row and stays put while the page moves under it.
fn content(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;

    let header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(widgets::page_title(settings.section.label(), ui))
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(close_button(workspace))
        .finish();

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(
                Expanded::new(
                    1.,
                    Scrollable::new(
                        settings.scroll.clone(),
                        Container::new(pages::render(workspace, settings.section, app))
                            .with_margin_right(SCROLLBAR_GUTTER)
                            .finish(),
                    )
                    .with_scrollbar(THEME.overlay_3)
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
        // `CONTENT_PADDING` from the card's edge, and the thumb lives in the
        // difference.
        right: CONTENT_PADDING - SCROLLBAR_GUTTER,
    })
    .finish()
}

/// The × that closes the card.
fn close_button(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page().control(Control::Close);

    Hoverable::new(state, move |mouse| {
        let (background, color) = if mouse.is_hovered() {
            (THEME.overlay_2, THEME.text_primary)
        } else {
            (Color::TRANSPARENT, THEME.text_muted)
        };

        Container::new(
            ConstrainedBox::new(
                Align::new(Text::new("\u{2715}", ui, 12.).with_color(color).finish()).finish(),
            )
            .with_width(CLOSE_BUTTON_SIZE)
            .with_height(CLOSE_BUTTON_SIZE)
            .finish(),
        )
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_margin_bottom(16.)
        .finish()
    })
    .on_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Settings(SettingsAction::Close));
    })
    .finish()
}
