//! The tab panel's rows, the two places anything may be pinned to one, and
//! the menu a secondary press opens.
//!
//! The rows themselves are still drawn by `workspace::tabs_panel::row`; what
//! this plugin owns is the pair of slots at the head of one — the 24px mark
//! that stands for the tab, and the small badge on its corner. Owning them is
//! what makes the leading edge of a row something other things can reach: it
//! used to be a status-coloured disc and nothing else, drawn by a private
//! function with no way in.
//!
//! # Why two slots and not one
//!
//! Because the two answer different questions. The mark says *what this tab
//! is* — a disc, a picture, a letter — and there is one of it, so
//! [`TAB_ROW_MARK`] is [`Cardinality::Single`] for the reason the header's
//! slot is: a row is 24 pixels wide at the front and two plugins drawing over
//! one another there is not a layout, it is a collision. The badge says *one
//! more thing about it* — that this checkout is a worktree, that this agent
//! wants an answer — and it is a second slot rather than a second contribution
//! to the first because a plugin that wanted to add a mark should not have to
//! take the tab's own away to do it.
//!
//! # Why the disc is not a contribution
//!
//! It would have been the tidier arrangement, and it is the wrong one. A
//! contribution to a row slot may **decline a row** — that is what
//! [`RowContribution`](crate::plugin::RowContribution) answering `None` means,
//! and it is the whole reason a badge on the worktrees only is expressible at
//! all. If the disc were a contribution at order 100, a plugin that won the
//! slot and then declined a row would leave that row with nothing at its
//! front, because `Slots::one` hands back the lowest order and does not go
//! looking for a second opinion. So the disc is what the *host* draws when
//! the slot has nothing to say, and "declined" means "as it was" rather than
//! "empty" — which is the only meaning a person reading the panel could
//! predict.
//!
//!
//! This plugin owns [`TAB_MENU_ENTRIES`] — the place a tab's context menu is
//! made of — and contributes the entries that are about a tab as such: putting
//! it in a group, copying what it says, closing it. The popup those rows land
//! in is drawn by [`workspace::tab_context_menu`](crate::workspace), for the
//! reason every surface in this application is drawn by the workspace: an
//! element tree is native work. What is owned *here* is the slot, and owning
//! it is what makes a tab's menu something a stranger's plugin can put a row
//! in — the same move `crook/header` made for the header's right-hand side.
//!
//! # Why these entries are commands
//!
//! Every one of them is registered with
//! [`register_command`](crate::plugin::Host::register_command) rather than
//! wired straight to a click, so the palette lists them and a person can bind
//! a chord to one. That is not generosity; it is what an entry *is* once the
//! menu is a slot. A row that dispatched a `WorkspaceAction` of its own would
//! be reachable by exactly one gesture, and the enum would grow an arm per row
//! — which is the arrangement `WorkspaceAction::Run` was built to end.
//!
//! # What a command acts on when no menu is up
//!
//! [`Workspace::menu_target`](crate::workspace::Workspace::menu_target): the
//! tab and pane the menu was opened over, and failing that the active tab's
//! focused pane. A command reached from the palette or from a chord therefore
//! means the tab a person is looking at, which is the only thing it could
//! sensibly mean, and the same handler serves both without a branch.

use std::path::Path;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::git::GitFacts;
use crate::plugin::{BuildError, Host, Plugin};
use crate::tab::{AgentStatus, PaneId, TabAction, TabId};
use crate::theme::theme;
use crate::workspace::tab_context_menu::{entry, inert_entry};
use crate::workspace::{Workspace, WorkspaceAction, status_color};

/// The mark at the head of a tab's row.
pub const TAB_ROW_MARK: SlotId = SlotId::new("tab.row.mark");

/// The small mark on the corner of that one.
pub const TAB_ROW_BADGE: SlotId = SlotId::new("tab.row.badge");

/// Warp's `VERTICAL_TABS_ICON_SIZE`: the box a mark is drawn in, reserved on
/// every row in both densities so that every row's text starts at the same x.
pub const MARK_SIZE: f32 = 24.;

/// Warp's `CIRCLE_RATIO` from `ui_components/icon_with_status.rs`: the disc
/// fills 76% of that box and the rest is breathing room.
const DISC_RATIO: f32 = 0.76;

/// How much of the box the badge on its corner takes.
///
/// Warp hangs a status ring off the bottom-right of its own 24px mark at
/// roughly this fraction, and the number is doing one job: a badge large
/// enough to read as a second thing and small enough that what it sits on is
/// still recognisable. Below about a third it is a smudge; above a half it is
/// two marks fighting.
const BADGE_RATIO: f32 = 0.46;

/// The ring drawn round the badge, in the panel's own ground.
///
/// Without it a badge overlapping a mark of a similar tone reads as one dented
/// shape rather than as a mark on a mark. The colour is
/// [`surface`](crate::theme::Theme::surface) in every state rather than
/// whatever the row is painted with: a selected row's ground is the
/// foreground at ten percent *over* that surface, and a ring that tried to
/// match it would be one more colour to keep in step with the row for a
/// difference of ten percent on eleven pixels. What this draws instead is a
/// hole in the mark, which is what a badge on an avatar has always been.
const BADGE_RING: f32 = 1.5;

/// What a row is, for a plugin being asked what to draw on it.
///
/// Borrowed rather than owned: it is made once per row per frame, and a
/// contribution that wanted to keep any of it can copy what it needs. Every
/// field here is something the panel already had — nothing is gathered for
/// the sake of this struct, because a fact that costs a syscall per row per
/// frame is a fact this cannot afford to carry.
pub struct TabRow<'a> {
    /// The tab the row stands for.
    pub tab: TabId,
    /// The pane inside it the row is drawn for, which under `Panes`
    /// granularity is one of several.
    pub pane: PaneId,
    /// What the row's first line says.
    pub title: &'a str,
    /// Whether this is the tab being looked at.
    pub active: bool,
    /// What the agent in the pane is doing.
    pub status: AgentStatus,
    /// Where the pane is working, when it has answered where that is.
    pub directory: Option<&'a Path>,
    /// What git knows about that directory, when it is in a repository and the
    /// answer has come home.
    pub git: Option<&'a GitFacts>,
}

/// Every row in the menu a tab's secondary press opens.
///
/// A list rather than a single, and ordered in bands of
/// [`BAND`](crate::workspace::tab_context_menu::BAND) — see that module for
/// what a band buys and why it is a division of `order` rather than a field.
pub const TAB_MENU_ENTRIES: SlotId = SlotId::new("tab.menu.entries");

/// The plugin that owns the tabs' menu.
pub struct Tabs;

impl Plugin for Tabs {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_row_slot(TAB_ROW_MARK, Cardinality::Single);
        host.declare_row_slot(TAB_ROW_BADGE, Cardinality::Single);
        host.declare_slot(TAB_MENU_ENTRIES, Cardinality::List);

        host.register_command(
            action("new-group-with-tab"),
            "New group with tab",
            |workspace, ctx| {
                let Some((tab, _)) = workspace.menu_target() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.handle_action(&WorkspaceAction::Tab(TabAction::NewInGroupOf(tab)), ctx);
            },
        );

        host.register_command(
            action("copy-pane-title"),
            "Copy pane title",
            |workspace, ctx| {
                let Some(text) = workspace.menu_pane_title() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.clipboard().write(&text);
            },
        );

        host.register_command(
            action("copy-working-directory"),
            "Copy working directory",
            |workspace, ctx| {
                let Some(directory) = workspace.menu_pane_directory() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.clipboard().write(&directory.to_string_lossy());
            },
        );

        host.register_command(action("close-tab"), "Close tab", |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.close_tab_context_menu(ctx);
            workspace.handle_action(&WorkspaceAction::Tab(TabAction::Close(tab)), ctx);
        });

        // Band 0: what this tab is *with*. Band 1: what it says, as text.
        // Band 2: it goes away. The gap between 2 and the worktree menu's 3 is
        // deliberate — a destructive entry sits alone, so the pointer on its
        // way down the column has a hairline to stop at before it.
        contribute(host, "new-group-with-tab", 0, |workspace| {
            workspace.menu_target().is_some()
        });
        contribute(host, "copy-pane-title", 100, |workspace| {
            workspace.menu_pane_title().is_some()
        });
        contribute(host, "copy-working-directory", 101, |workspace| {
            workspace.menu_pane_directory().is_some()
        });
        contribute(host, "close-tab", 200, |workspace| {
            workspace.menu_target().is_some()
        });

        Ok(())
    }
}

/// Built once and leaked, because a manifest outlives everything that reads it
/// and `PluginId` cannot be constructed in a `const`.
/// Contributes one entry that runs this plugin's action of the same name.
///
/// The label is the command's own title, read back off the host rather than
/// written twice: the palette and the menu say the same words about the same
/// action because there is one place the words are.
///
/// `live` decides whether the row can be pressed. These four are about the tab
/// the menu is on and a tab always has them, so a false answer draws an *inert*
/// row rather than none — a directory the shell has not reported yet is a copy
/// that cannot happen this frame, not a menu with a hole in it.
fn contribute(
    host: &mut Host,
    name: &'static str,
    order: i32,
    live: impl Fn(&Workspace) -> bool + 'static,
) {
    let label = host.title_of(&action(name)).unwrap_or(name).to_owned();
    let id = host.action(&action(name));
    let key = format!("crook/tabs/{name}");

    host.contribute(TAB_MENU_ENTRIES, name, order, move |workspace, _| {
        let Some(id) = id.filter(|_| live(workspace)) else {
            return inert_entry(workspace, &key, label.clone());
        };
        entry(workspace, &key, label.clone(), WorkspaceAction::Run(id))
    });
}

/// `crook/tabs/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/tabs/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/tabs").expect("a literal that parses"),
        name: "Tabs",
        description: "The rows down the left edge, what may be pinned to the front of one, and the menu a tab opens.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// The mark at the head of one row, with whatever is on its corner.
pub fn mark(workspace: &Workspace, row: &TabRow<'_>, app: &AppContext) -> Box<dyn Element> {
    let host = workspace.host();
    let face = host
        .rows()
        .one(TAB_ROW_MARK, |build| build(workspace, row, app))
        .flatten()
        .unwrap_or_else(|| disc(row.status));

    let mut stack = Stack::new().with_child(Align::new(face).finish());
    if let Some(badge) = host
        .rows()
        .one(TAB_ROW_BADGE, |build| build(workspace, row, app))
        .flatten()
    {
        stack.add_child(Align::new(ringed(badge)).bottom_right().finish());
    }

    ConstrainedBox::new(stack.finish())
        // Reserved whole, in both densities, so every row's text starts at the
        // same x however tall the row is.
        .with_width(MARK_SIZE)
        .with_height(MARK_SIZE)
        .finish()
}

/// What a row's mark is when nothing has replaced it: a status-coloured disc.
///
/// Warp draws a glyph inside the circle and a status ring past its
/// bottom-right corner. Crook has no icon font, so the status is the disc's
/// own colour — one mark instead of two, in the same reserved box, so rows
/// line up with Warp's. The corner it left free is what [`TAB_ROW_BADGE`] is.
fn disc(status: AgentStatus) -> Box<dyn Element> {
    let diameter = MARK_SIZE * DISC_RATIO;

    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(status_color(status))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(diameter)
    .with_height(diameter)
    .finish()
}

/// A badge, in its own disc of the row's ground.
///
/// The size is the host's and not the plugin's, for the reason no measurement
/// in this tier is a plugin's: a badge that could name its own size is a badge
/// that is the wrong size on a display it was not written for, and this one
/// has 11 pixels to be right in.
fn ringed(badge: Box<dyn Element>) -> Box<dyn Element> {
    let inside = MARK_SIZE * BADGE_RATIO - 2. * BADGE_RING;

    Container::new(
        ConstrainedBox::new(Align::new(badge).finish())
            .with_width(inside)
            .with_height(inside)
            .finish(),
    )
    .with_background_color(theme().surface)
    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
    .with_uniform_padding(BADGE_RING)
    .finish()
}
