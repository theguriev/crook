//! The command palette: one box for everything the window holds.
//!
//! The first plugin that is not an extraction. Everything before it moved a
//! feature Crook already had onto a seam; this one exists *because* of the
//! seam — a system whose primary noun is "a named action" needs a place to
//! find one, and until there were named actions there was nothing to list.
//!
//! What it lists is no longer only the actions. A window with a command
//! palette, a tab search box and a settings search box is a window that asks a
//! person to know *which* box answers the thing they are looking for before
//! they can look for it — and the answer is often "I do not know, that is why
//! I am searching". So the chord opens all three at once, under a heading
//! each, and a sigil narrows it to one when somebody does know: `>` the
//! commands, `@` the tabs, `#` the settings, `?` the keys. The two boxes in
//! the sidebar stay exactly as they were; this is the one that answers
//! without being told where to look.
//!
//! # It knows nothing about what it lists
//!
//! Every command row comes from [`Host::commands`], so the palette has no
//! table of its own and cannot fall behind one. A plugin that registers a
//! command is in the palette the moment it loads, and a plugin that is
//! disabled is out of it the moment it is — including this one. The tabs come
//! from the strip and the settings rows from the pages the settings slot
//! holds, on the same terms: nothing is written down here that something else
//! is already the truth about.
//!
//! # Five lists, one box
//!
//! Which list is showing is worked out from the *text*, not held in a flag,
//! and that is the design the whole surface turns on — see
//! [`rows::Mode`](rows::Mode). A mode rather than five surfaces, because a
//! second surface would be a second contributor to `WINDOW_OVERLAY` competing
//! with this one for a keystroke; and a mode in the text because that is what
//! a search survives — `split`, `>split` and `?split` are one question asked
//! of three lists, and there is no second source of truth for a paste or an
//! undo to put out of step with what is on screen.
//!
//! None of them spends a chord. `crook/palette/keys`, `.../tabs` and
//! `.../settings` are commands like any other, so each is a row here, a line
//! on the Keyboard Shortcuts page and bindable by anyone who wants it, but
//! nothing is suggested for them: a chord for a surface visited once a
//! fortnight is a chord nobody remembers on the fortnight they need it. The
//! footer names the list Tab goes to next on every open instead, which is a
//! discovery path no chord and no settings page can claim.
//!
//! # Why the palette's own keys are actions too
//!
//! Escape, Enter, Tab and the arrows are claimed through
//! [`Host::claim_surface`],
//! which names an action rather than doing anything. So they end at
//! `WorkspaceAction::Run` exactly as a chord out of somebody's `keybindings.json`
//! does, there is one dispatch path rather than two, and a person who wants
//! Ctrl-N to move the selection can bind it.
//!
//! They are registered as plain actions rather than as *commands*, so they do
//! not appear in the palette itself. A list of things to do whose first four
//! rows are "close this list" is a list nobody reads.

mod chord;
mod list;
mod rows;
mod state;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::clipboard::Clipboard;
use crate::input_keys::Platform;
use crate::plugin::{BuildError, Host, Plugin};
use crate::plugins::settings::SETTINGS_SECTION;
use crate::tab::{Tab, TabAction};
use crate::workspace::Workspace;

use rows::Mode;
use state::{Goto, Palette};

use super::window::WINDOW_OVERLAY;

/// The plugin.
pub struct CommandPalette;

impl Plugin for CommandPalette {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Command)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Claimed before anything else is registered, because the flag it
        // hands back is what the palette's state is built around.
        let showing = host.claim_surface(|keystroke| {
            // Bare keys only, still: that is what keeps `cmd+t` opening a tab
            // over an open palette, and it is the rule the Themes panel
            // states for its own arrows. Tab is safe to take because
            // `Workspace::action_for` consults a surface *before* the field,
            // so it never reaches the query box — and while a surface is up
            // no pane has the keyboard to complete a word with.
            if !keystroke.modifiers.is_empty() {
                return None;
            }
            let name = match keystroke.key.as_str() {
                "escape" => "close",
                "up" => "previous",
                "down" => "next",
                "enter" => "run",
                "tab" => "mode",
                _ => return None,
            };
            Some(action(name))
        });

        // Registered before the palette is built, because the ids are what a
        // row about a tab or a settings line carries: see [`Goto`].
        let goto = Goto {
            tab: host.register_action(action("go-to-tab"), go_to_tab),
            setting: host.register_action(action("go-to-setting"), go_to_setting),
        };

        let palette = std::rc::Rc::new(Palette::new(
            showing,
            Clipboard::new(),
            host.fonts(),
            host.voice(),
            goto,
        ));

        host.register_command(action("open"), "Command palette", {
            let palette = palette.clone();
            move |workspace, ctx| {
                // Whatever was said seeds the query, which is how every row
                // anywhere else hands an argumentless action its subject —
                // and the only way this surface can be pictured with a query
                // in it by `--snapshot`. Nothing said is an empty box, which
                // is what the chord does.
                palette.open_in(Mode::Everything, &workspace.host().said());
                // The panes have to be told, because a surface going up is
                // exactly the kind of thing `sync_input_keys` exists to hear
                // about and nothing else would say so.
                workspace.sync_input_keys();
                ctx.notify();
            }
        });
        // The chord every editor uses for this. A suggestion, not a binding:
        // a person's own file wins, and so does every one of Crook's own
        // chords — see `Host::suggest_binding`.
        host.suggest_binding(
            match Platform::current() {
                Platform::Mac => "shift+cmd+p",
                Platform::Other => "ctrl+shift+p",
            },
            action("open"),
        );

        // Commands and not actions, so that each of the other lists is a row
        // in the first one: findable by typing `keys`, `tabs` or `settings`,
        // listed on the Keyboard Shortcuts page, and bindable by anyone who
        // wants a chord for one. They ship with none — see this module's own
        // doc.
        //
        // What is said seeds the query, which is how a row anywhere else hands
        // an argumentless action its subject, and the only way these surfaces
        // can be pictured with `--snapshot`.
        for (name, title, mode) in [
            ("keys", "Commands and their keys", Mode::Keys),
            ("tabs", "Go to a tab", Mode::Tabs),
            ("settings", "Search the settings", Mode::Settings),
        ] {
            host.register_command(action(name), title, {
                let palette = palette.clone();
                move |workspace, ctx| {
                    palette.open_in(mode, &workspace.host().said());
                    workspace.sync_input_keys();
                    ctx.notify();
                }
            });
        }

        host.register_action(action("close"), {
            let palette = palette.clone();
            move |workspace, ctx| {
                palette.close();
                workspace.sync_input_keys();
                ctx.notify();
            }
        });

        // Guarded — here, and on `run` and `mode` below — because these are
        // actions and an action is bindable: a chord on `crook/palette/run`
        // pressed with the card down would run whatever the last list it drew
        // had left selected.
        for (name, by) in [("next", 1_isize), ("previous", -1)] {
            host.register_action(action(name), {
                let palette = palette.clone();
                move |workspace, ctx| {
                    if !palette.is_open() {
                        return;
                    }
                    // Built once and passed along: `showing` settles the
                    // selection, and settling twice in one frame would tell
                    // the second call the query had not changed when what it
                    // means is that the first call has already been told.
                    let rows = rows::showing(workspace, &palette, ctx);
                    palette.move_selection(by, &rows);
                    rows::scroll_into_view(&palette, &rows);
                    ctx.notify();
                }
            });
        }

        host.register_action(action("run"), {
            let palette = palette.clone();
            move |workspace, ctx| {
                if !palette.is_open() {
                    return;
                }
                let rows = rows::showing(workspace, &palette, ctx);
                // A match on what the line *is*, not arithmetic over an index
                // counted somewhere else: a heading and a key a pane eats have
                // nothing for Enter to mean, and a list of keys that ran the
                // command one row further down would do it with no symptom.
                let chosen = rows
                    .command_at(palette.selected())
                    .map(|(_, target)| target.clone());

                // Down before the action runs, so that a command which opens
                // something of its own is not opening it behind the palette —
                // and so that a command which fails leaves no surface up.
                palette.close();
                workspace.sync_input_keys();

                if let Some(target) = chosen {
                    // Said first and taken by the action when it runs, which
                    // is the same order a click on the row uses.
                    if let Some(subject) = target.subject {
                        workspace.host().say(subject);
                    }
                    workspace.run_action(target.action, ctx);
                }
                ctx.notify();
            }
        });

        host.register_action(action("mode"), {
            let palette = palette.clone();
            move |workspace, ctx| {
                if !palette.is_open() {
                    return;
                }
                // The row is kept by *name* across the switch, which is the
                // whole argument for a mode that lives in the text: the same
                // command is somewhere in both lists, at two different places
                // in two different index spaces. Two builds, because the
                // second one's `settle` is what makes the fallback right when
                // the row did not survive the query.
                let before = rows::showing(workspace, &palette, ctx);
                let keep = before
                    .command_at(palette.selected())
                    .and_then(|(entry, _)| entry.action.clone());

                palette.next_mode();

                let after = rows::showing(workspace, &palette, ctx);
                if let Some(at) = keep.and_then(|name| after.find(&name)) {
                    palette.select(at);
                    rows::scroll_into_view(&palette, &after);
                }
                ctx.notify();
            }
        });

        host.contribute(WINDOW_OVERLAY, "palette", 0, {
            let palette = palette.clone();
            move |workspace, app| {
                // Nothing to draw and nothing to work out: a closed palette is
                // a branch per frame rather than a grouped list built for a
                // card that is not on screen.
                if !palette.is_open() {
                    return Empty::new().finish();
                }
                // The query is read every frame rather than watched, so a
                // keystroke into the field re-filters the list without
                // anything having to notice that it changed. What it costs is
                // that the selection has to be put back when the list under it
                // has: see `rows::showing`.
                let rows = rows::showing(workspace, &palette, app);
                list::render(&palette, &rows)
            }
        });

        Ok(())
    }
}

/// Makes a tab the active one: what a row under `Tabs` does.
///
/// Which tab arrives through [`Host::said`](crate::plugin::Host::said) as the
/// number behind its id, because an action takes no argument of its own and
/// this enum-free path is the one a click and Enter can share. A number that
/// names no tab is a tab closed between the frame that listed it and the press
/// — nothing to do, and nothing worth saying.
///
/// The sidebar is put back on the tabs first: the palette can be opened over
/// the settings or the plugins, and going to a tab that stays hidden behind a
/// section would be a command that appears to have done nothing.
fn go_to_tab(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    let said = workspace.host().said();
    let Ok(wanted) = said.trim().parse::<u64>() else {
        log::warn!("crook/palette/go-to-tab was asked for {said:?}, which is no tab");
        return;
    };
    let Some(tab) = workspace
        .tabs()
        .iter()
        .find(|tab| tab.id().as_u64() == wanted)
        .map(Tab::id)
    else {
        return;
    };

    workspace.show_section(None, ctx);
    workspace.apply(TabAction::Select(tab), ctx);
}

/// Opens the settings where a row lives: what a row under `Settings` does.
///
/// Said as `<page key>\t<what the row is called>`, the two things the page
/// itself is addressed by — a tab separates them because a page key holds
/// slashes and a label holds spaces, and neither holds a tab.
///
/// **The label is typed into the rail's own search box**, and that is the
/// whole of how a person arrives at the row rather than at the page. A page is
/// thirty rows tall and there is nothing on it to point at one of them; the
/// box is the thing that already narrows a page to what somebody asked for,
/// and it is on screen with the words in it, so the way out is visible and is
/// the way out they already know.
fn go_to_setting(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    let said = workspace.host().said();
    let Some((key, label)) = said.split_once('\t') else {
        log::warn!("crook/palette/go-to-setting was asked for {said:?}, which names no row");
        return;
    };
    let Some(page) = workspace.host().settings_page_id(key) else {
        return;
    };

    let label = label.to_owned();
    workspace.open_settings_page(Some(page), ctx);
    // **After**, and that ordering is the whole of this function: arriving at
    // a section empties every field on the way — `Workspace::clear_fields`,
    // so that a filter is never left in force over a list somebody comes back
    // to — and a box seeded before the page is a box seeded into the last
    // frame of the section being left.
    let (_, search) = workspace.field(SETTINGS_SECTION, "search");
    search.edit(|editor| editor.set_text(label));
    ctx.notify();
}

/// `crook/palette/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/palette/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/palette").expect("a literal that parses"),
        name: "Command palette",
        description: "Everything the window and its plugins can be asked to do, by name.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
