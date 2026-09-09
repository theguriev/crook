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

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::icons::Lucide;
use crookui_core::prelude::*;

use crook_plugin::{ActionName, Cardinality, Manifest, PluginId, SlotId, Tier};

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use crate::git::GitFacts;
use crate::input_keys::Platform;
use crate::plugin::{ActionId, BuildError, Host, Plugin, Showing};
use crate::plugins::header::HEADER_LEFT;
use crate::tab::{AgentStatus, PaneId, Tab, TabAction, TabColor, TabId, TabStrip};
use crate::text_input::TextInput;
use crate::theme::theme;
use crate::workspace::tab_context_menu::{entry, field_entry, inert_entry, nothing, swatch_entry};
use crate::workspace::{OptionsAction, TabMenuAction, Workspace, WorkspaceAction, status_color};

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
    /// How many rows before this one, in panel order, are working in the same
    /// [`place`].
    ///
    /// Zero for all but the second and later rows in one directory, and the
    /// only reason it is here: a row is named to a plugin as a hash of where
    /// it is working, every tab opened from the window starts in Crook's own
    /// directory, and a person with four tabs in one checkout was four rows
    /// that every plugin could only see as one. See [`place_ordinal`].
    pub nth: usize,
}

/// What a row is *called*, where a plugin's key is concerned: where it is
/// working, or what it says when nothing has answered where that is.
///
/// A borrow in both cases and on the path anything is likely to be — a
/// directory that is valid UTF-8 is a `Cow::Borrowed` — because this is asked
/// asked of every row *before* each row, once a frame, by [`place_ordinal`],
/// and a rule that allocated to compare two rows would be a panel allocating
/// with the square of its tabs.
pub fn place<'a>(directory: Option<&'a Path>, title: &'a str) -> Cow<'a, str> {
    directory.map_or(Cow::Borrowed(title), Path::to_string_lossy)
}

/// Which of the rows working in one place this one is, counted in panel order.
///
/// A row's key is a hash of [`place`] and nothing else, which is what makes it
/// the same number tomorrow — a `PaneId` is minted from a counter and means
/// nothing in a later process, so an id could not have done this job. The cost
/// of that is the collision this counts past: every session starts in the
/// directory Crook itself was started in, so a window of new tabs is a window
/// of rows that are all *the same place*, and a plugin drawing a mark per tab
/// drew one mark on all of them.
///
/// The ordinal is what a restored session brings back rather than what a
/// process mints: `session.json` remembers the panes in order, so the second
/// tab in a checkout is the second one again tomorrow and keeps its mark.
/// Closing the first of them does move the second's — it is now the first —
/// and that is the honest price of naming a row by where it works rather than
/// by an identity nothing outlives the process to carry.
pub fn place_ordinal(strip: &TabStrip, pane: PaneId) -> usize {
    let Some(mine) = strip.pane(pane) else {
        return 0;
    };
    let mine = place(
        mine.session().working_directory.as_deref(),
        mine.session().display_title(),
    );

    strip
        .panes()
        .take_while(|(_, other)| other.id() != pane)
        .filter(|(_, other)| {
            place(
                other.session().working_directory.as_deref(),
                other.session().display_title(),
            ) == mine
        })
        .count()
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
    /// The pointer's relationship to the chip in the header, kept here for
    /// the reason the rename state is: the contribution that draws the chip
    /// is a closure that outlives `build`, and a fresh state on every frame
    /// would be a chip that never knew it was hovered.
    waiting: MouseStateHandle,
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

        // "Pin tab" and "Unpin tab" are one action under one name, because
        // they are one gesture: a person reaches for the row that says what
        // will happen, and a palette that offered both would offer one that
        // does nothing on whichever tab is in front of them.
        host.register_command(action("pin-tab"), "Pin tab", |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.close_tab_context_menu(ctx);
            workspace.handle_action(&WorkspaceAction::Tab(TabAction::TogglePin(tab)), ctx);
        });

        for color in TabColor::ALL.map(Some).into_iter().chain([None]) {
            let name = color.map_or("no-color".to_owned(), |color| {
                format!("color-{}", color.name())
            });
            // A command rather than a bare action, like every other entry of
            // this menu. Seven of them is more rows than the palette wants of
            // any one plugin, and it is still the right trade: somebody who
            // colours tabs has one colour they use, and a name is the only
            // thing they can hang a chord off.
            let title = color.map_or("Remove the tab's colour".to_owned(), |color| {
                format!("Colour the tab {}", color.name())
            });
            host.register_command(action(&name), title, move |workspace, ctx| {
                let Some((tab, _)) = workspace.menu_target() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.handle_action(
                    &WorkspaceAction::Tab(TabAction::SetColor { tab, color }),
                    ctx,
                );
            });
        }

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

        // The menu itself, by name. Everything *in* it has been a command
        // since this plugin was written, and the popup that holds them was the
        // one thing only a secondary press could reach — so a person who never
        // touches the pointer could run every entry and never see the list
        // they belong to.
        host.register_command(action("open-menu"), "Tab menu", |workspace, ctx| {
            let Some((tab, pane)) = workspace.menu_target() else {
                return;
            };
            // The tabs first, because the popup is drawn by the row it hangs
            // off: asked for from the palette with the settings on screen
            // there would be nowhere to put it, and `open_tab_context_menu`
            // would rightly refuse. Somebody asking for a tab's menu is asking
            // to look at it.
            workspace.handle_action(&WorkspaceAction::ShowSection(None), ctx);
            workspace.handle_action(&TabMenuAction::Open { tab, pane }.into(), ctx);
        });

        // The panel's own menu, on the empty space under the tab list. Named
        // for the same reason: it is the only way to reach "View as: Panes"
        // and the four other options that are not on the settings page.
        host.register_command(
            action("view-options"),
            "Tab panel view options",
            |workspace, ctx| {
                // The tabs first, for the reason "Tab menu" above needs them:
                // this popup hangs off the empty space under the list.
                workspace.handle_action(&WorkspaceAction::ShowSection(None), ctx);
                workspace.handle_action(&OptionsAction::TogglePopup.into(), ctx);
            },
        );

        // The two things a *group* of tabs can be asked, which until now were
        // the chevron and the × on its heading and nothing else. Both resolve
        // the group from the tab the menu is on — or, with no menu, from the
        // active tab — so "fold this away" means the group in front of you.
        for (name, title, fold) in [
            ("toggle-group", "Collapse or expand the tab's group", true),
            ("close-group", "Close every tab in the group", false),
        ] {
            host.register_command(action(name), title, move |workspace, ctx| {
                let Some((tab, _)) = workspace.menu_target() else {
                    return;
                };
                let Some(group) = workspace.tabs().get(tab).and_then(Tab::group) else {
                    // A tab that belongs to no group. Nothing to fold, and
                    // nothing to close that `close-tab` does not already do.
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                let action = if fold {
                    TabAction::ToggleGroup(group)
                } else {
                    TabAction::CloseGroup(group)
                };
                workspace.handle_action(&WorkspaceAction::Tab(action), ctx);
            });
        }

        // The one command here that is about the strip rather than a row of
        // it: go to the next pane that is waiting for a person. A pane, not a
        // tab, because a split can hold two agents and only one of them
        // asked; focusing it activates its tab the way a click on its row
        // does, and looking at it is what answers the request.
        let next_waiting =
            host.register_command(action("next-waiting"), "Next tab waiting for you", {
                |workspace, ctx| {
                    let Some(pane) = next_waiting(workspace.tabs()) else {
                        return;
                    };
                    workspace.close_tab_context_menu(ctx);
                    workspace.handle_action(&WorkspaceAction::Tab(TabAction::FocusPane(pane)), ctx);
                }
            });
        // A suggestion, on the same terms as the palette's chord: a person's
        // own file and every one of Crook's own chords win over it. `cmd-j`
        // is nobody's on macOS, and the Shift off it is the one every chord
        // takes there, since bare ctrl-j is a newline to the tty.
        host.suggest_binding(
            match Platform::current() {
                Platform::Mac => "cmd+j",
                Platform::Other => "ctrl+shift+j",
            },
            action("next-waiting"),
        );
        host.contribute(HEADER_LEFT, "waiting", 0, {
            let hover = self.waiting.clone();
            move |workspace, _| waiting_chip(workspace, hover.clone(), next_waiting)
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

        // Warp's grouping, band for band. Band 0: whether it stays put.
        // Band 1: what it is *with*. Band 2: what it says, copied. Band 3:
        // what it is called. Band 4: it goes away, alone — so the pointer on
        // its way down has a hairline to stop at before the one entry here
        // that cannot be undone. Band 5 is the worktree menu's.
        pin_entry(host, 0);
        contribute(host, "new-group-with-tab", 100, |workspace| {
            workspace.menu_target().is_some()
        });
        contribute(host, "copy-pane-title", 200, |workspace| {
            workspace.menu_pane_title().is_some()
        });
        contribute(host, "copy-working-directory", 201, |workspace| {
            workspace.menu_pane_directory().is_some()
        });
        rename_entry(host, "rename-tab", 300, Renaming::Tab, &self.rename, &field);
        rename_entry(
            host,
            "rename-pane",
            301,
            Renaming::Pane,
            &self.rename,
            &field,
        );
        contribute(host, "close-tab", 400, |workspace| {
            workspace.menu_target().is_some()
        });
        // Band 6, under the worktree menu's 5: the swatches are the foot of
        // Warp's menu, and they are the one entry that is not a sentence.
        swatch_row(host, 600);

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

/// Contributes the entry that pins, which is the one entry whose *label*
/// changes.
///
/// It says what pressing it will do rather than what is true, which is the
/// only way one row can serve both states: "Unpin tab" on a pinned tab is a
/// promise, and "Pinned" would be a label a person has to work out the verb
/// for.
fn pin_entry(host: &mut Host, order: i32) {
    let id = host.action(&action("pin-tab"));

    host.contribute(TAB_MENU_ENTRIES, "pin-tab", order, move |workspace, _| {
        let key = "crook/tabs/pin-tab";
        let pinned = workspace
            .menu_target()
            .and_then(|(tab, _)| workspace.tabs().get(tab))
            .map(crate::tab::Tab::is_pinned);
        match (pinned, id) {
            (Some(pinned), Some(id)) => entry(
                workspace,
                key,
                if pinned { "Unpin tab" } else { "Pin tab" },
                WorkspaceAction::Run(id),
            ),
            _ => inert_entry(workspace, key, "Pin tab"),
        }
    });
}

/// Contributes the row of colour swatches at the foot of the menu.
fn swatch_row(host: &mut Host, order: i32) {
    let none = host.action(&action("no-color"));
    let colors: Vec<(TabColor, Option<ActionId>)> = TabColor::ALL
        .into_iter()
        .map(|color| {
            (
                color,
                host.action(&action(&format!("color-{}", color.name()))),
            )
        })
        .collect();

    host.contribute(TAB_MENU_ENTRIES, "color", order, move |workspace, _| {
        let Some(chosen) = workspace
            .menu_target()
            .and_then(|(tab, _)| workspace.tabs().get(tab))
            .map(crate::tab::Tab::color)
        else {
            return nothing();
        };

        // The one that takes a colour off leads, which is Warp's order and the
        // right one: it is the state a tab starts in.
        let mut swatches = vec![(
            "crook/tabs/no-color".to_owned(),
            None,
            chosen.is_none(),
            match none {
                Some(id) => WorkspaceAction::Run(id),
                None => return nothing(),
            },
        )];
        for (color, id) in &colors {
            let Some(id) = id else {
                continue;
            };
            swatches.push((
                format!("crook/tabs/color-{}", color.name()),
                Some(theme().terminal.bright[color.index()]),
                chosen == Some(*color),
                WorkspaceAction::Run(*id),
            ));
        }
        swatch_entry(workspace, swatches)
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
/// The pane the "next waiting" chord goes to: the first one after the active
/// tab, in the panel's order and round the end of it, that is waiting for a
/// person — or `None` when nothing is.
///
/// The panel's order rather than most-recent-first, because a person working
/// down a list of agents wants the list's order back: the chord pressed
/// three times visits three tabs and not the same two in turn.
pub fn next_waiting(strip: &TabStrip) -> Option<PaneId> {
    let panes: Vec<(TabId, PaneId, bool)> = strip
        .panes()
        .map(|(tab, pane)| {
            let active = strip.is_active(tab) && strip.focused_pane_id() == Some(pane.id());
            (tab, pane.id(), pane.session().is_waiting(active))
        })
        .collect();
    let after_active = panes
        .iter()
        .rposition(|(tab, _, _)| strip.is_active(*tab))
        .map_or(0, |index| index + 1);
    panes
        .iter()
        .cycle()
        .skip(after_active)
        .take(panes.len())
        .find(|(_, _, waiting)| *waiting)
        .map(|(_, pane, _)| *pane)
}

/// How many panes are waiting for a person, which is the number on the chip.
pub fn waiting_count(strip: &TabStrip) -> usize {
    strip
        .panes()
        .filter(|(tab, pane)| {
            let active = strip.is_active(*tab) && strip.focused_pane_id() == Some(pane.id());
            pane.session().is_waiting(active)
        })
        .count()
}

/// The chip in the header that counts the waiting panes and goes to the next
/// one when pressed — or nothing at all, which is what the header shows
/// while nobody is waiting. A count of zero is not information.
fn waiting_chip(workspace: &Workspace, hover: MouseStateHandle, go: ActionId) -> Box<dyn Element> {
    let count = waiting_count(workspace.tabs());
    if count == 0 {
        return Empty::new().finish();
    }
    let ui = workspace.fonts().ui;
    let label = if count == 1 {
        "1 waiting".to_owned()
    } else {
        format!("{count} waiting")
    };

    Hoverable::new(hover, move |mouse| {
        let ground = if mouse.is_hovered() {
            theme().overlay_2
        } else {
            theme().overlay_1
        };
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(WAITING_GAP)
                .with_child(
                    Icon::new(Lucide::Bell, WAITING_ICON)
                        .with_color(theme().usage_high)
                        .finish(),
                )
                .with_child(
                    Text::new(label.clone(), ui, WAITING_TEXT)
                        .with_color(theme().text_primary)
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(ground)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(WAITING_RADIUS)))
        .with_padding(Padding {
            top: 3.,
            bottom: 3.,
            left: 8.,
            right: 9.,
        })
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(go)))
    .finish()
}

/// The chip's type, the same size as a row's chips in the panel.
const WAITING_TEXT: f32 = 11.;
/// Its bell, a little larger than the type beside it so the two read as one
/// line.
const WAITING_ICON: f32 = 13.;
/// The air between the bell and the count.
const WAITING_GAP: f32 = 5.;
/// The corner the chip is cut to — the pill a row's chips use, one up.
const WAITING_RADIUS: f32 = 5.;

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
