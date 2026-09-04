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
//! # Why the palette's own keys are actions too
//!
//! Escape, Enter and the arrows are claimed through [`Host::claim_surface`],
//! which names an action rather than doing anything. So they end at
//! `WorkspaceAction::Run` exactly as a chord out of somebody's `keybindings.json`
//! does, there is one dispatch path rather than two, and a person who wants
//! Ctrl-N to move the selection can bind it.
//!
//! They are registered as plain actions rather than as *commands*, so they do
//! not appear in the palette itself. A list of things to do whose first four
//! rows are "close this list" is a list nobody reads.

mod list;
mod state;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::clipboard::Clipboard;
use crate::input_keys::Platform;
use crate::plugin::{ActionId, BuildError, Host, Plugin};
use crate::workspace::Workspace;

use state::{Command, Palette};

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
            if !keystroke.modifiers.is_empty() {
                return None;
            }
            let name = match keystroke.key.as_str() {
                "escape" => "close",
                "up" => "previous",
                "down" => "next",
                "enter" => "run",
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

        host.register_action(action("close"), {
            let palette = palette.clone();
            move |workspace, ctx| {
                palette.close();
                workspace.sync_input_keys();
                ctx.notify();
            }
        });

        for (name, by) in [("next", 1_isize), ("previous", -1)] {
            host.register_action(action(name), {
                let palette = palette.clone();
                move |workspace, ctx| {
                    let count = matching(workspace, &palette).len();
                    palette.move_selection(by, count);
                    scroll_selection_into_view(&palette);
                    ctx.notify();
                }
            });
        }

        host.register_action(action("run"), {
            let palette = palette.clone();
            move |workspace, ctx| {
                let commands = matching(workspace, &palette);
                let chosen = commands.get(palette.selected()).map(|(_, id)| *id);

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

        host.contribute(WINDOW_OVERLAY, "palette", 0, {
            let palette = palette.clone();
            move |workspace, _| {
                // The query is read every frame rather than watched, so a
                // keystroke into the field re-filters the list without
                // anything having to notice that it changed. What it costs is
                // that the selection has to be put back when the list under it
                // has: see `matching`.
                let commands = matching(workspace, &palette);
                list::render(&palette, &commands)
            }
        });

        Ok(())
    }
}

/// Every command that matches what has been typed, in the order it is shown.
///
/// Recomputed wherever it is needed rather than cached, because the two things
/// it depends on — the query and which plugins are loaded — both change
/// without this plugin being told. At the size of this list that is a dozen
/// substring comparisons on a keystroke.
fn matching(workspace: &Workspace, palette: &Palette) -> Vec<(Command, ActionId)> {
    let host = workspace.host();
    let query = palette.query().editor().text().to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();

    let mut matched: Vec<(Command, ActionId)> = Vec::new();
    for (owner, action, title) in host.commands() {
        // Every term has to match something, and they need not match the same
        // thing: this is a person narrowing a list, not writing a pattern.
        // The same rule the settings page's search follows, and the same
        // reason it is substring rather than fuzzy — a fuzzy match over a
        // short list answers every query with a row, and the answer stops
        // meaning anything.
        let found = terms.iter().all(|term| {
            title.to_lowercase().contains(term)
                || action.as_str().contains(term)
                || owner.name().contains(term)
        });
        if !found {
            continue;
        }

        // Only what is registered *now*. A command whose plugin has been
        // disabled since it was registered has no id, and a row that did
        // nothing when it was clicked would be worse than no row.
        let Some(id) = host.action(action) else {
            continue;
        };
        matched.push((
            Command {
                action: action.clone(),
                title: title.clone(),
            },
            id,
        ));
    }

    matched.sort_by(|(left, _), (right, _)| left.title.cmp(&right.title));
    palette.settle(&query, matched.len());
    matched
}

/// Keeps the row the keyboard is on inside the list.
///
/// Rows are a fixed height, so this is arithmetic — the same trade the Themes
/// panel and the tabs panel both name for their own lists.
fn scroll_selection_into_view(palette: &Palette) {
    let row = list::ROW_HEIGHT;
    let top = palette.selected() as f32 * row;
    let scroll = palette.scroll();
    let mut scroll = scroll.lock();

    let offset = scroll.offset();
    let viewport = scroll.viewport();
    if top < offset {
        scroll.scroll_to(top);
    } else if top + row > offset + viewport {
        scroll.scroll_to(top + row - viewport);
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
    })
}
