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
//! # Renaming, which is the whole of why the host has a field registry
//!
//! Two of these entries turn into a text field, and until this plugin was
//! written nothing outside the binary could have done that. A plugin could put
//! a popup on screen, draw a box in it and claim Escape — and still have
//! nowhere for a keystroke to land, because which field is listening is a fact
//! the element tree cannot work out and `Workspace::sync_input_keys` answered
//! by naming, in source, every field in the window. There were two, and
//! neither was a plugin's.
//!
//! [`Host::claim_field`](crate::plugin::Host::claim_field) is the other half
//! of [`claim_surface`](crate::plugin::Host::claim_surface), and the two are
//! used together here: the surface claims Enter and Escape while a rename is
//! being typed, and the field is what the letters in between land in. Nothing
//! about renaming is in the menu's shell — it draws the field, and that is the
//! whole of its involvement.
//!
//! The state is a `Cell` on an `Rc` the contributions and the handlers share,
//! which is how a native plugin owns anything: `build` runs before the
//! workspace exists, so a plugin's state cannot live in the workspace and its
//! closures cannot borrow one.
//!
//! # What a command acts on when no menu is up
//!
//! [`Workspace::menu_target`](crate::workspace::Workspace::menu_target): the
//! tab and pane the menu was opened over, and failing that the active tab's
//! focused pane. A command reached from the palette or from a chord therefore
//! means the tab a person is looking at, which is the only thing it could
//! sensibly mean, and the same handler serves both without a branch.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Cardinality, Manifest, PluginId, SlotId, Tier};

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use crate::git::GitFacts;
use crate::plugin::{BuildError, Host, Plugin, Showing};
use crate::tab::{AgentStatus, PaneId, TabAction, TabId};
use crate::text_input::TextInput;
use crate::theme::theme;
use crate::workspace::tab_context_menu::{entry, field_entry, inert_entry};
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

/// What a rename in progress is about.
///
/// Two entries and not one, because a tab and its pane are two names that mean
/// different things — the tab's is what the panel's heading says about a piece
/// of work, the pane's is what a row says about one agent — and a tab with one
/// pane shows only the second of them. Warp offers both for the same reason.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Renaming {
    /// The tab the menu is on.
    Tab,
    /// The pane whose row it was opened over.
    Pane,
}

/// What a rename field says before anything is typed into it.
const RENAME_PLACEHOLDER: &str = "name";

/// Whether a rename is being typed, and what it is about.
///
/// The flag the surface is raised by lives in here beside the answer rather
/// than next to it, because the two must never disagree: a surface left up
/// with nothing being renamed would go on eating Escape from the menu, and one
/// left down would leave Enter to be typed into a shell nobody can see.
/// Setting them apart is a bug waiting for the one path that forgets.
#[derive(Default)]
struct Rename {
    /// What is being renamed, if anything.
    what: Cell<Option<Renaming>>,
    /// The host's flag for this plugin's surface, once it has been claimed.
    surface: RefCell<Option<Showing>>,
}

impl Rename {
    /// Starts one.
    fn begin(&self, what: Renaming) {
        self.what.set(Some(what));
        self.raise();
    }

    /// Ends one, and says what it was about.
    fn end(&self) -> Option<Renaming> {
        let was = self.what.take();
        self.raise();
        was
    }

    /// What is being renamed, if anything.
    fn what(&self) -> Option<Renaming> {
        self.what.get()
    }

    /// Puts the surface's flag where the answer is.
    fn raise(&self) {
        if let Some(surface) = self.surface.borrow().as_ref() {
            surface.set(self.what.get().is_some());
        }
    }
}

/// The plugin that owns the tabs' menu.
///
/// It holds the one thing a menu of contributions cannot hold for it: which of
/// its two rename entries is being typed into. `Rc` because the contributions
/// and the action handlers are closures that outlive `build` and share it.
#[derive(Default)]
pub struct Tabs {
    rename: Rc<Rename>,
}

impl Tabs {
    /// A plugin with nothing being renamed.
    pub fn new() -> Self {
        Self::default()
    }
}

/// What the thing a rename is about is called right now.
///
/// `None` where there is no tab at all, which is a window on its way out.
fn current_name(workspace: &Workspace, what: Renaming) -> Option<String> {
    let (tab, pane) = workspace.menu_target()?;
    let tab = workspace.tabs().get(tab)?;
    Some(match what {
        Renaming::Tab => tab.name().to_owned(),
        Renaming::Pane => tab.panes().get(pane)?.title().to_owned(),
    })
}

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

        // The field every rename is typed into. One, not two: only one of the
        // two entries can be being renamed at a time, and a second field would
        // be a second place the same answer could be kept.
        let field = host.claim_field("rename", {
            let rename = self.rename.clone();
            move |workspace| rename.what().is_some() && workspace.tab_context_menu().is_open()
        });

        for (name, title, what) in [
            ("rename-tab", "Rename tab", Renaming::Tab),
            ("rename-pane", "Rename pane", Renaming::Pane),
        ] {
            let rename = self.rename.clone();
            let field = field.clone();
            host.register_command(action(name), title, move |workspace, ctx| {
                let Some(current) = current_name(workspace, what) else {
                    return;
                };
                // Selected, not just filled: the name is almost always being
                // replaced rather than edited, so the first letter typed
                // should be the new name's first letter. The worktree
                // creator's branch field opens the same way.
                field.edit(|editor| {
                    editor.set_text(&current);
                    editor.select_all();
                });
                rename.begin(what);
                workspace.sync_input_keys();
                ctx.notify();
            });
        }

        // Neither of these is a command. A palette row that put the keyboard
        // into a field behind the palette would be a row nobody could use, and
        // a chord for "stop renaming" is a chord for a state that only exists
        // while a field already has every key.
        host.register_action(action("commit-rename"), {
            let rename = self.rename.clone();
            let field = field.clone();
            move |workspace, ctx| {
                let Some(what) = rename.end() else {
                    return;
                };
                let typed = field.editor().text().trim().to_owned();
                // An emptied field is "put back the name I started with",
                // which is what `None` means everywhere this reaches.
                let name = (!typed.is_empty()).then_some(typed);
                match (what, workspace.menu_target()) {
                    (Renaming::Tab, Some((tab, _))) => workspace.rename_tab(tab, name, ctx),
                    (Renaming::Pane, Some((_, pane))) => {
                        workspace.update_session(pane, ctx, |session| {
                            session.custom_title = name;
                        });
                    }
                    (_, None) => {}
                }
                workspace.close_tab_context_menu(ctx);
            }
        });

        host.register_action(action("cancel-rename"), {
            let rename = self.rename.clone();
            move |workspace, ctx| {
                if rename.end().is_none() {
                    return;
                }
                // Back to the menu rather than out of it: Escape is one step
                // back, which is the rule the worktree submenu follows and the
                // one a person has already been taught by it.
                workspace.sync_input_keys();
                ctx.notify();
            }
        });

        // Enter and Escape, while a rename is being typed. A surface rather
        // than bindings, because these two keys belong to whatever is in front
        // of a person and this is only in front of them sometimes.
        let showing = host.claim_surface(|keystroke| {
            if !keystroke.modifiers.is_empty() {
                return None;
            }
            match keystroke.key.as_str() {
                "enter" => Some(action("commit-rename")),
                "escape" => Some(action("cancel-rename")),
                _ => None,
            }
        });
        *self.rename.surface.borrow_mut() = Some(showing);

        // Warp's grouping, band for band. Band 0: what this tab is *with*.
        // Band 1: what it says, copied. Band 2: what it is called. Band 3: it
        // goes away, alone — so the pointer on its way down the column has a
        // hairline to stop at before the one entry here that cannot be undone.
        // Band 4 is the worktree menu's, and the gap is deliberate.
        contribute(host, "new-group-with-tab", 0, |workspace| {
            workspace.menu_target().is_some()
        });
        contribute(host, "copy-pane-title", 100, |workspace| {
            workspace.menu_pane_title().is_some()
        });
        contribute(host, "copy-working-directory", 101, |workspace| {
            workspace.menu_pane_directory().is_some()
        });
        rename_entry(host, "rename-tab", 200, Renaming::Tab, &self.rename, &field);
        rename_entry(
            host,
            "rename-pane",
            201,
            Renaming::Pane,
            &self.rename,
            &field,
        );
        contribute(host, "close-tab", 300, |workspace| {
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

/// Contributes one of the two entries that can turn into a field.
///
/// The row draws itself as a label until this is what is being renamed, and as
/// the field from then on — in place, in the column the entry was pressed in.
/// A dialog somewhere else would have to say which tab it was about; a row
/// that became a box does not.
fn rename_entry(
    host: &mut Host,
    name: &'static str,
    order: i32,
    what: Renaming,
    rename: &Rc<Rename>,
    field: &TextInput,
) {
    let label = host.title_of(&action(name)).unwrap_or(name).to_owned();
    let id = host.action(&action(name));
    let key = format!("crook/tabs/{name}");
    let rename = rename.clone();
    let field = field.clone();

    host.contribute(TAB_MENU_ENTRIES, name, order, move |workspace, _| {
        if rename.what() == Some(what) {
            return field_entry(workspace, &key, &field, RENAME_PLACEHOLDER);
        }
        // Inert while the *other* one is being typed into, rather than absent:
        // a menu that reflowed under the field a person is typing in would
        // move the field.
        let Some(id) = id.filter(|_| rename.what().is_none()) else {
            return inert_entry(workspace, &key, label.clone());
        };
        if current_name(workspace, what).is_none() {
            return inert_entry(workspace, &key, label.clone());
        }
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
