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
//! # Search
//!
//! Warp's is the most interesting behaviour on its settings page, and it is
//! here: one field at the top of the rail filtering the rail and the content
//! together, per-widget keyword blobs, and a match count beside each page.
//! See [`search`] for what "matches" means and [`field`] for the box itself,
//! which is the first text input in Crook that is not a pane's.
//!
//! Two deliberate differences from Warp, both of which are the same
//! disagreement. Warp matches a query against the keyword blob **only**, so
//! typing a page's own name does not reliably find that page and typing a
//! row's own label does not reliably find that row; here the label, the line
//! under it, the value on its right, the category and the page all count as
//! words the row is found by, and the keywords are what is *added* to them.
//! And Warp moves the rail's selection when the page you are on filters out,
//! which loses where you were; here the selection never moves on its own —
//! which page is shown is worked out from the query, so clearing the box puts
//! you back.
//!
//! # What it does not have
//!
//! **A settings-file footer.** Warp's rail ends in "Open settings file", and
//! an inline alert when that file failed to parse. Crook's file cannot fail
//! visibly — every unreadable value falls back to a default and logs a line —
//! so there is nothing to alert about, and the path is on the About page for
//! anyone who wants to open it themselves.

mod field;
mod pages;
mod search;
mod widgets;

use std::collections::HashMap;

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::settings::{Density, Granularity, Layout, PrimaryInfo, Subtitle};
use crate::text_input::TextInput;
use crate::theme::theme;

use search::Query;
use widgets::Category;

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
    /// "Reset to defaults".
    ResetTabOptions,
    /// The preview in the "Current theme" row.
    ThemeRow,
    /// The row around it, which is what opens the Themes panel.
    ThemeRowButton,
    /// The rail's search box.
    Search,
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
    /// What has been typed into the rail's search box.
    ///
    /// Not cleared when the page changes — a query narrows every page at once,
    /// so clicking through the rail while searching is browsing the answers —
    /// but cleared when the pane closes. A filter that came back with the page
    /// would be a settings page that had silently lost most of its rows, and
    /// the box that explains why is at the top of a rail somebody has to look
    /// at to find out.
    pub(super) search: TextInput,
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

    /// What has been typed, ready to match rows against.
    fn query(&self) -> Query {
        Query::new(self.search.editor().text())
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
///
/// # What the search does to this
///
/// One query filters the rail and the page at the same time, which is Warp's
/// design and the only one that makes sense: a rail that still listed every
/// page while the page beside it held two rows would be telling you the
/// opposite of what the field is for.
///
/// So while something is being searched for, **every** page is built — the
/// rail says how many rows each of them holds, and it cannot say that about a
/// page it has not built. Four pages of about thirty rows once per frame is a
/// rounding error beside the frame they are part of, and at rest only the page
/// on screen is built at all.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let state = workspace.settings_page();
    let query = state.query();

    let built: Vec<(Section, Vec<Category>)> = if query.is_empty() {
        vec![(state.section, pages::of(workspace, state.section, app))]
    } else {
        Section::ALL
            .into_iter()
            .map(|section| (section, pages::of(workspace, section, app)))
            .collect()
    };

    let counts: Vec<(Section, usize)> = built
        .iter()
        .map(|(section, categories)| (*section, matches_in(categories, &query, section.label())))
        .collect();
    let showing = showing(state.section, &counts, &query);
    let categories = built
        .into_iter()
        .find(|(section, _)| *section == showing)
        .map(|(_, categories)| categories)
        .unwrap_or_default();

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(rail(workspace, showing, &counts, &query))
        .with_child(Expanded::new(1., content(workspace, categories, showing, &query)).finish())
        .finish()
}

/// How many of a page's rows a query is looking for.
fn matches_in(categories: &[Category], query: &Query, page: &str) -> usize {
    categories
        .iter()
        .map(|category| {
            category
                .entries
                .iter()
                .filter(|entry| entry.words.is_some())
                .filter(|entry| entry.matches(query, &[page, category.title]))
                .count()
        })
        .sum()
}

/// Which page the content actually shows.
///
/// The one the rail has selected, unless the search has emptied it — in which
/// case the first page that has anything, because a page that has gone blank
/// under you while its neighbours have answers is a search that looks broken.
///
/// Computed rather than assigned, which is the whole difference from Warp's:
/// Warp moves the selection when a page filters out, and has then lost where
/// you were. Here the selection never moves on its own, so clearing the box
/// puts you back on the page you were reading.
fn showing(selected: Section, counts: &[(Section, usize)], query: &Query) -> Section {
    if query.is_empty() {
        return selected;
    }
    if counts
        .iter()
        .any(|(section, found)| *section == selected && *found > 0)
    {
        return selected;
    }

    counts
        .iter()
        .find(|(_, found)| *found > 0)
        .map(|(section, _)| *section)
        .unwrap_or(selected)
}

/// The rail: the four pages, and what this build is.
///
/// No background of its own — the panel behind it already painted one — and a
/// right border instead, which is what Warp's rail is too. A filled rail
/// inside a rounded panel would also have to know the panel's corner radius to
/// avoid painting square into it.
fn rail(
    workspace: &Workspace,
    showing: Section,
    counts: &[(Section, usize)],
    query: &Query,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    column.add_child(
        Container::new(
            field::SearchField::new(
                state.search.clone(),
                workspace.clipboard().clone(),
                workspace.fonts(),
                state.control(Control::Search),
            )
            .finish(),
        )
        .with_margin_bottom(10.)
        .finish(),
    );

    for section in Section::ALL {
        let found = counts
            .iter()
            .find(|(page, _)| *page == section)
            .map(|(_, found)| *found);

        // A page with nothing in it is not listed at all while something is
        // being searched for. Warp drops it too, and the alternative — a row
        // that says "(0)" — is a row whose only purpose is to be declined.
        if !query.is_empty() && found.unwrap_or(0) == 0 {
            continue;
        }
        column.add_child(rail_row(
            workspace,
            section,
            section == showing,
            found.filter(|_| !query.is_empty()),
            ui,
        ));
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

fn rail_row(
    workspace: &Workspace,
    section: Section,
    selected: bool,
    found: Option<usize>,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let state = workspace.settings_page().control(Control::Section(section));
    // Warp's `Features (3)`: the count is part of the label rather than a
    // badge beside it, which is what keeps a rail of counted and uncounted
    // rows from having two different shapes.
    let label = match found {
        Some(found) => format!("{} ({found})", section.label()),
        None => section.label().to_owned(),
    };

    Hoverable::new(state, move |mouse| {
        let (background, color) = if selected {
            (theme().overlay_3, theme().text_primary)
        } else if mouse.is_hovered() {
            (theme().overlay_1, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().text_muted)
        };

        Container::new(
            Text::new(label.clone(), ui, widgets::LABEL_SIZE)
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
fn content(
    workspace: &Workspace,
    categories: Vec<Category>,
    showing: Section,
    query: &Query,
) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;
    let body = page(categories, showing, query, ui).unwrap_or_else(|| nothing_found(query, ui));

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(centred(widgets::page_title(showing.label(), ui)))
            .with_child(
                Expanded::new(
                    1.,
                    Scrollable::new(settings.scroll.clone(), centred(body))
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

/// One page's categories, filtered, or `None` when the query emptied it.
///
/// The divider above a category is decided here rather than at the call site,
/// because which category is first is a property of what survived.
fn page(
    categories: Vec<Category>,
    section: Section,
    query: &Query,
    ui: crookui_core::fonts::FamilyId,
) -> Option<Box<dyn Element>> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    let mut drawn = 0;

    for category in categories {
        let rows: Vec<Box<dyn Element>> = category
            .entries
            .into_iter()
            .filter(|entry| entry.matches(query, &[section.label(), category.title]))
            .map(|entry| entry.element)
            .collect();

        if rows.is_empty() {
            continue;
        }
        column.add_child(widgets::category_element(
            category.title,
            drawn == 0,
            rows,
            ui,
        ));
        drawn += 1;
    }

    (drawn > 0).then(|| column.finish())
}

/// What the page says when the query found nothing anywhere.
///
/// Warp's two lines, which are the two things worth saying: that the search
/// is the reason the page is empty, and that the way out is different words.
fn nothing_found(query: &Query, ui: crookui_core::fonts::FamilyId) -> Box<dyn Element> {
    let _ = query;

    let line = |text: &'static str, size: f32, color: Color| {
        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        for text in super::wrap(text, 60) {
            column.add_child(
                Text::new(text, ui, size)
                    .with_color(color)
                    .with_line_height_ratio(1.45)
                    .finish(),
            );
        }
        column.finish()
    };

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(line(
                "No settings match your search.",
                widgets::LABEL_SIZE,
                theme().text_primary,
            ))
            .with_child(
                Container::new(line(
                    "Try different keywords, or check for a typo.",
                    widgets::DESCRIPTION_SIZE,
                    theme().text_muted,
                ))
                .with_margin_top(6.)
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme().overlay_1)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .with_uniform_padding(16.)
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
