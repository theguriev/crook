//! The command palette: everything the window can be asked to do, by name.
//!
//! The first plugin that is not an extraction. Everything before it moved a
//! feature Crook already had onto a seam; this one exists *because* of the
//! seam — a system whose primary noun is "a named action" needs a place to
//! find one, and until there were named actions there was nothing to list.
//!
//! # It knows nothing about what it lists
//!
//! Every row comes from [`Host::commands`], so the palette has no table of its
//! own and cannot fall behind one. A plugin that registers a command is in the
//! palette the moment it loads, and a plugin that is disabled is out of it the
//! moment it is — including this one.
//!
//! # Two lists, one box
//!
//! Typing `?` at the front of the query turns the launcher into the list of
//! keys: the same card, the same field and the same keys, with a different
//! list under them, and Tab turns it back. A mode rather than a
//! second surface, because a second surface would be a second contributor to
//! `WINDOW_OVERLAY` competing with this one for a keystroke; and a mode kept
//! in the *text* rather than in a flag, because that is what a search
//! survives — `split` and `?split` are one question asked of two lists, and
//! there is no second source of truth for a paste or an undo to put out of
//! step with what is on screen.
//!
//! It spends no chord. `crook/palette/keys` is a command like any other, so
//! it is a row here, a line on the Keyboard Shortcuts page and bindable by
//! anyone who wants it, but nothing is suggested for it: a chord for a
//! surface visited once a fortnight is a chord nobody remembers on the
//! fortnight they need it. The footer says `? for the keys` on every open
//! instead, which is a discovery path no chord and no settings page can
//! claim.
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
use crate::workspace::Workspace;

use state::Palette;

use super::window::WINDOW_OVERLAY;

/// The plugin.
pub struct CommandPalette;

impl Plugin for CommandPalette {
    fn manifest(&self) -> &'static Manifest {
        manifest()
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

        let palette = std::rc::Rc::new(Palette::new(showing, Clipboard::new(), host.fonts()));

        host.register_command(action("open"), "Command palette", {
            let palette = palette.clone();
            move |workspace, ctx| {
                palette.open();
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

        // A command and not an action, so that the second list is a row in the
        // first: findable by typing `keys`, listed on the Keyboard Shortcuts
        // page, and bindable by anyone who wants a chord for it. It ships with
        // none — see this module's own doc.
        //
        // What is said seeds the query, which is how a row anywhere else hands
        // an argumentless action its subject, and the only way this surface
        // can be pictured with `--snapshot`.
        host.register_command(action("keys"), "Commands and their keys", {
            let palette = palette.clone();
            move |workspace, ctx| {
                palette.open_keys(&workspace.host().said());
                workspace.sync_input_keys();
                ctx.notify();
            }
        });

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
                    let rows = rows::showing(workspace, &palette);
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
                let rows = rows::showing(workspace, &palette);
                // A match on what the line *is*, not arithmetic over an index
                // counted somewhere else: a heading and a key a pane eats have
                // nothing for Enter to mean, and a list of keys that ran the
                // command one row further down would do it with no symptom.
                let chosen = rows.command_at(palette.selected()).map(|(_, id)| id);

                // Down before the action runs, so that a command which opens
                // something of its own is not opening it behind the palette —
                // and so that a command which fails leaves no surface up.
                palette.close();
                workspace.sync_input_keys();

                if let Some(id) = chosen {
                    workspace.run_action(id, ctx);
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
                let before = rows::showing(workspace, &palette);
                let keep = before
                    .command_at(palette.selected())
                    .and_then(|(entry, _)| entry.action.clone());

                palette.toggle_mode();

                let after = rows::showing(workspace, &palette);
                if let Some(at) = keep.and_then(|name| after.find(&name)) {
                    palette.select(at);
                    rows::scroll_into_view(&palette, &after);
                }
                ctx.notify();
            }
        });

        host.contribute(WINDOW_OVERLAY, "palette", 0, {
            let palette = palette.clone();
            move |workspace, _| {
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
                let rows = rows::showing(workspace, &palette);
                list::render(&palette, &rows)
            }
        });

        Ok(())
    }
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
