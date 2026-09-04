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
//! # Every page comes from a plugin
//!
//! This module owns the *pane* — the rail, the search field, the scrolling
//! column, and the rule about which page is showing — and none of the pages.
//! They are contributed to `settings.page`, which `crook/settings` declares,
//! and each of the five Crook ships belongs to the plugin whose feature it
//! configures: the Usage page is `crook/usage`'s, so somebody who disables
//! that plugin loses the page along with the chip.
//!
//! What that costs is written down where it is paid. A page is named by a key
//! (`owner/entry`) rather than by a variant of an enum, so the rail's order is
//! the `order` each page asks for; the mouse-state map is keyed by a string
//! rather than by a `Control` variant, because a plugin's rows are not
//! enumerable here; and [`Words`](search::Words) and
//! [`Category`](widgets::Category) hold owned text, because a plugin's labels
//! are not literals in this crate.
//!
//! # What it does not have
//!
//! **A settings-file footer.** Warp's rail ends in "Open settings file", and
//! an inline alert when that file failed to parse. Crook's file cannot fail
//! visibly — every unreadable value falls back to a default and logs a line —
//! so there is nothing to alert about, and the path is on the About page for
//! anyone who wants to open it themselves.

pub(crate) mod search;
pub(crate) mod widgets;

use std::collections::HashMap;
use std::fmt;

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::theme::theme;

use search::Query;
use widgets::Category;

use super::action::{SettingsAction, WorkspaceAction};
use super::view::Workspace;
use crate::plugin::{Host, PageId};
use crate::plugins::settings::SETTINGS_SECTION;

/// The widest the content column is allowed to get, before it is centred in
/// whatever is left.
///
/// Warp's is 800 against a 12px body; this is that, scaled to a pane that also
/// has a 160px rail in front of it. Past it a row's label and its control end
/// up so far apart that the eye loses which control belongs to which row.
const CONTENT_MAX_WIDTH: f32 = 560.;

/// What the rail's search box says while nothing has been typed. Warp's word,
/// and there is only one word this box can say.
const SEARCH_PLACEHOLDER: &str = "Search";

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

/// One clickable thing on the page, by a name of its own.
///
/// The key of the mouse-state map the view holds. It used to be a closed enum
/// with a variant per control, which was right while every row on this page
/// was written down in this crate: a page's rows come and go with the section
/// on screen, so a struct with a field per handle would need every field
/// reachable from a renderer that does not know which page it is drawing.
///
/// It is a string now for the reason [`Words`](super::search::Words) holds
/// owned text: a page contributed by a plugin has controls nobody enumerated,
/// and an enum cannot have a variant for them. What that costs is that two
/// plugins can collide on a name, so each writes its own id in front — the
/// same discipline an [`ActionName`](crate::plugin::ActionName) enforces, one
/// rung less formally.
pub(crate) type Control = String;

/// A control with a name of its own.
///
/// The name is the page's to choose and has to be unique within the whole
/// page, not within the plugin — one map serves every page, because the map
/// outlives whichever page is on screen. A plugin's page should put its own
/// name in front.
pub(crate) fn named(name: &str) -> Control {
    name.to_owned()
}

/// One of a group of controls, told apart by the value it stands for.
///
/// Keyed by the *value* rather than by its place in the group, so that
/// changing which options are offered — the subtitle row does exactly that —
/// cannot hand a control the hover state of the option that used to be in its
/// slot.
pub(crate) fn keyed(group: &str, value: impl fmt::Debug) -> Control {
    format!("{group}.{value:?}")
}

/// Which page the rail has selected/// Which page the rail has selected, and what the mouse is doing to each of
/// its controls.
///
/// Whether the page is *open* is not here: the settings pane's existence is
/// that answer, and a flag beside it would be a second copy of it to keep
/// true. What is here outlives the pane on purpose — closing the tab and
/// opening it again comes back to the page you were on, scrolled where you
/// left it, which is what Warp's per-window pane manager buys by holding its
/// view handle across a close.
#[derive(Default)]
pub(crate) struct SettingsState {
    /// Which page the rail has selected, by its `owner/entry` key, or `None`
    /// for "whichever is first".
    ///
    /// A key rather than a variant, because the pages come from a slot and the
    /// set of them depends on which plugins are loaded. A key whose page has
    /// gone — the plugin was disabled while the pane was closed — reads as
    /// `None` and the rail lands on the first page, which is the same thing it
    /// does before anything has been chosen.
    ///
    /// Not persisted to disk: where somebody was last time they changed a
    /// setting is not a preference, and a settings file that recorded it would
    /// rewrite itself on a click that changed nothing.
    pub(crate) page: Option<String>,
    /// How far the content column has been scrolled.
    pub(super) scroll: ScrollStateHandle,
    /// One mouse state per control, created the first time that control is
    /// drawn and kept for as long as the window lives.
    ///
    /// The scroll positions a page brought with it, by a name of its own.
    ///
    /// A page that draws itself does its own scrolling and may do it in more
    /// than one place, so one handle per name, made the first time it is asked
    /// for. The rows layout's own [`Self::scroll`] is not one of these.
    scrolls: std::cell::RefCell<HashMap<String, ScrollStateHandle>>,
    /// A `HashMap` behind a `RefCell` rather than a field per control: see
    /// [`Control`]. The interior mutability is what lets a render — which
    /// holds `&Workspace` — ask for the handle of a control it is about to
    /// draw for the first time.
    controls: std::cell::RefCell<HashMap<Control, MouseStateHandle>>,
}

impl SettingsState {
    /// A scroll position a page brought with it, made the first time it is
    /// asked for.
    pub(crate) fn scroll_named(&self, key: &str) -> ScrollStateHandle {
        self.scrolls
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// Which page the rail has selected, resolved against what is loaded.
    pub(crate) fn selected(&self, host: &Host) -> Option<PageId> {
        let chosen = self
            .page
            .as_deref()
            .and_then(|key| host.settings_page_id(key));
        chosen.or_else(|| host.settings_pages().first().map(|(id, _)| *id))
    }

    /// The mouse state for one control, creating it if this is its first
    /// frame.
    pub(crate) fn control(&self, control: Control) -> MouseStateHandle {
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
/// page it has not built. Five pages of about thirty rows once per frame is a
/// rounding error beside the frame they are part of, and at rest only the page
/// on screen is built at all.
pub(crate) fn render(
    workspace: &Workspace,
    app: &AppContext,
) -> (Box<dyn Element>, Box<dyn Element>) {
    let state = workspace.settings_page();
    let host = workspace.host();
    let (_, search) = workspace.field(SETTINGS_SECTION, "search");
    let query = Query::new(search.editor().text());
    let pages = host.settings_pages();
    let Some(selected) = state.selected(host) else {
        // No plugin contributed a page, which is a build with the settings
        // plugins taken out rather than a state to design for.
        return (Empty::new().finish(), Empty::new().finish());
    };

    let title_of = |id: PageId| {
        pages
            .iter()
            .find(|(page, _)| *page == id)
            .map(|(_, title)| title.clone())
            .unwrap_or_default()
    };

    // While something is being searched for the rail has to say how many rows
    // each page holds, and it cannot say that about a page it has not built.
    // A page that draws itself has no rows to count: it is found by its title
    // or not at all, which is one, or none.
    let counts: Vec<(PageId, usize)> = pages
        .iter()
        .map(|(id, title)| {
            if query.is_empty() {
                return (*id, 0);
            }
            let found = match host.build_settings_page(*id, workspace, app) {
                Some(categories) => matches_in(&categories, &query, title),
                None => 0,
            };
            (*id, found)
        })
        .collect();

    let showing = showing(selected, &counts, &query);
    let body = host.build_settings_page(showing, workspace, app);

    (
        rail(workspace, &pages, showing, &counts, &query),
        content(workspace, body, &title_of(showing), &query),
    )
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
                .filter(|entry| entry.matches(query, &[page, &category.title]))
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
fn showing(selected: PageId, counts: &[(PageId, usize)], query: &Query) -> PageId {
    if query.is_empty() {
        return selected;
    }
    if counts
        .iter()
        .any(|(page, found)| *page == selected && *found > 0)
    {
        return selected;
    }

    counts
        .iter()
        .find(|(_, found)| *found > 0)
        .map(|(page, _)| *page)
        .unwrap_or(selected)
}

/// The rail: every page there is, and what this build is.
///
/// No background of its own — the panel behind it already painted one — and a
/// right border instead, which is what Warp's rail is too. A filled rail
/// inside a rounded panel would also have to know the panel's corner radius to
/// avoid painting square into it.
fn rail(
    workspace: &Workspace,
    pages: &[(PageId, String)],
    showing: PageId,
    counts: &[(PageId, usize)],
    query: &Query,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let (field, search) = workspace.field(SETTINGS_SECTION, "search");

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    column.add_child(
        Container::new(
            super::text_field::TextField::new(
                search.clone(),
                workspace.clipboard().clone(),
                workspace.fonts(),
                state.control(named("search")),
                SEARCH_PLACEHOLDER,
            )
            .with_icon(Lucide::Search)
            .with_focus(WorkspaceAction::Settings(SettingsAction::FocusField(Some(
                field,
            ))))
            .finish(),
        )
        .with_margin_bottom(10.)
        .finish(),
    );

    for (id, title) in pages {
        let found = counts
            .iter()
            .find(|(page, _)| page == id)
            .map(|(_, found)| *found);

        // A page with nothing in it is not listed at all while something is
        // being searched for. Warp drops it too, and the alternative — a row
        // that says "(0)" — is a row whose only purpose is to be declined.
        if !query.is_empty() && found.unwrap_or(0) == 0 {
            continue;
        }
        column.add_child(rail_row(
            workspace,
            *id,
            title,
            *id == showing,
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

    // No width and no border of its own: the rail *is* the sidebar's body
    // while the settings are showing, and the panel around it has both.
    Container::new(column.finish())
        .with_uniform_padding(12.)
        .finish()
}

fn rail_row(
    workspace: &Workspace,
    id: PageId,
    title: &str,
    selected: bool,
    found: Option<usize>,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let state = workspace.settings_page().control(keyed("page", id));
    // Warp's `Features (3)`: the count is part of the label rather than a
    // badge beside it, which is what keeps a rail of counted and uncounted
    // rows from having two different shapes.
    let label = match found {
        Some(found) => format!("{title} ({found})"),
        None => title.to_owned(),
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
        ctx.dispatch_typed_action(WorkspaceAction::Settings(SettingsAction::Select(id)));
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
    categories: Option<Vec<Category>>,
    title: &str,
    query: &Query,
) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;
    let rows = page(categories.unwrap_or_default(), title, query, ui)
        .unwrap_or_else(|| nothing_found(query, ui));

    let column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(centred(widgets::page_title(title, ui)))
        .with_child(
            Expanded::new(
                1.,
                Scrollable::new(settings.scroll.clone(), centred(rows))
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .finish(),
        )
        .finish();

    Container::new(column)
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
    title: &str,
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
            .filter(|entry| entry.matches(query, &[title, &category.title]))
            .map(|entry| entry.element)
            .collect();

        if rows.is_empty() {
            continue;
        }
        column.add_child(widgets::category_element(
            &category.title,
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
