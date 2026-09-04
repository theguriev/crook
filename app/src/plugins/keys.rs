//! The Keys page: the chords, and everything reachable by name.
//!
//! Two halves with two different origins. The chords are written down here,
//! because they are compiled in and a page that read them from a table would
//! be printing the table rather than the bindings. The named actions are not:
//! they come from the host, so a plugin that registers one is on this page the
//! moment it loads — which is the only way a person finds out what to write in
//! `keymap.json`.

use std::collections::HashMap;

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::input_keys::Platform;
use crate::keymap::{self, Bound};
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};

/// The plugin that owns the Keys page.
pub struct Keys;

impl Plugin for Keys {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", "Keys", 30, |workspace, _| keys(workspace));
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/keys").expect("a literal that parses"),
        name: "Keys settings",
        description: "Every chord the window answers to, and every action reachable by name.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}

/// A binding as this platform spells it.
///
/// The two keymaps are not one chord with the modifier swapped. Off macOS a
/// bare ctrl-letter belongs to the program in the pane — ctrl-c interrupts it,
/// ctrl-d ends its input — so Crook's own chords take a Shift there and the
/// tabs move on Page Up and Page Down rather than on the arrows the field
/// selects with. A page that printed `cmd` or `ctrl` in front of one spelling
/// would name chords the window does not answer to, so each row spells both
/// out and this picks between them with the same [`Platform::current`] the
/// window delegate resolves a keystroke with.
fn chord(mac: &str, other: &str) -> String {
    match Platform::current() {
        Platform::Mac => mac,
        Platform::Other => other,
    }
    .to_owned()
}
/// Everything reachable by name, and the chord that reaches it.
///
/// The one part of this page that is not written down in this file. A named
/// action is registered by a plugin, so what is listed here depends on which
/// plugins are loaded — which is the whole point of a name: the page can print
/// a row for something it has never heard of, and a person can bind a chord to
/// it without anybody adding a variant to an enum.
fn named_actions(workspace: &Workspace) -> Category {
    let fonts = workspace.fonts();
    let keymap = workspace.keymap();

    // Which chord, if any, reaches each name. A name may be bound more than
    // once; all of them are printed, because a person looking for "why does
    // this fire" needs to see the second one.
    let mut bound: HashMap<String, Vec<String>> = HashMap::new();
    for (keystroke, action) in keymap::bindings(keymap) {
        if let Some(Bound::Named(name)) = action {
            bound
                .entry(name.as_str().to_owned())
                .or_default()
                .push(keymap::format_chord(&keystroke));
        }
    }
    // A stable order, since the map's is not one.
    for chords in bound.values_mut() {
        chords.sort();
    }

    let mut entries: Vec<Entry> = workspace
        .host()
        .actions()
        .names()
        .into_iter()
        .map(|(action, owner)| {
            let chords = match bound.get(action.as_str()) {
                Some(chords) => chords.join(", "),
                None => "not bound".to_owned(),
            };
            widgets::fact(
                Words::new(action.to_string())
                    .with_description(format!("From {owner}."))
                    .with_keywords(&["plugin", "action", "bind", "keymap"]),
                chords,
                true,
                fonts,
            )
        })
        .collect();

    entries.push(widgets::note(
        "Bind one by putting its name in keymap.json beside a chord — \
         \"cmd-shift-u\": \"crook/usage/refresh\". A name no loaded plugin answers to is \
         a chord that does nothing, so a keymap written for a plugin you have not installed \
         costs you nothing but that one chord.",
        fonts.ui,
    ));

    widgets::category("Named actions", entries)
}
/// The bindings, which are fixed.
fn keys(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    // The chord is the value, which is what makes it searchable: somebody who
    // remembers the key and not the name of what it does types the key.
    let binding = |words: Words, chord: String| widgets::fact(words, chord, true, fonts);
    let key = Words::new;

    vec![
        widgets::category(
            "Tabs and panes",
            vec![
                binding(
                    key("New agent tab").with_keywords(&["open", "create", "another"]),
                    chord("cmd-t", "ctrl-shift-t"),
                ),
                binding(
                    key("Close the focused pane").with_keywords(&["quit", "exit", "kill"]),
                    chord("cmd-w", "ctrl-shift-w"),
                ),
                binding(
                    key("Split to the right").with_keywords(&["vertical", "side", "beside"]),
                    chord("cmd-d", "ctrl-shift-d"),
                ),
                binding(
                    key("Split downwards").with_keywords(&["horizontal", "below", "under"]),
                    chord("cmd-shift-d", "ctrl-shift-e"),
                ),
                binding(
                    key("Previous / next tab").with_keywords(&["switch", "cycle", "between"]),
                    chord("cmd-alt-left / right", "ctrl-pageup / pagedown"),
                ),
                binding(
                    key("Move the active tab").with_keywords(&["reorder", "position"]),
                    chord("cmd-ctrl-left / right", "ctrl-shift-pageup / pagedown"),
                ),
                widgets::note(
                    "Every chord here stays off the ones the field needs. On macOS that is why \
                     the tabs are on cmd-alt-arrow rather than cmd-shift-arrow, which selects to \
                     the end of a line; everywhere else it is why Crook's own chords carry a \
                     Shift, since a bare ctrl-letter belongs to the program in the pane.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "Command blocks",
            vec![
                binding(
                    key("Copy a whole command and its output").with_keywords(&[
                        "clipboard",
                        "block",
                        "yank",
                    ]),
                    "hover it, then click".to_owned(),
                ),
                binding(
                    key("Move through the commands").with_keywords(&["scroll", "block", "wheel"]),
                    "wheel".to_owned(),
                ),
                widgets::note(
                    "A pane's output is a list of commands, and that needs the shell to say \
                     where each one starts and ends. Crook installs the marks that do it into \
                     zsh, bash and fish by itself, without writing to any dotfile — there is \
                     nothing to set up here. It cannot reach a shell it did not start, so on \
                     the far side of an ssh, in a container, or under a shell it has no \
                     snippet for, a pane is a plain terminal instead: one continuous stream, \
                     no blocks, and no field — every key goes straight to the shell.",
                    ui,
                ),
                widgets::note(
                    "There is no keyboard binding here yet. Selecting a block, walking between \
                     them and jumping to the bottom are all still to come; what a block can be \
                     asked for today is its own text, exactly, with one click.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "Selecting the output",
            vec![
                binding(
                    key("Select a run of text").with_keywords(&["mouse", "drag", "highlight"]),
                    "drag".to_owned(),
                ),
                binding(
                    key("Select a word / a whole line")
                        .with_keywords(&["mouse", "double", "triple", "click"]),
                    "double / triple click".to_owned(),
                ),
                binding(
                    key("Select a column of it").with_keywords(&[
                        "block",
                        "rectangular",
                        "alt",
                        "option",
                    ]),
                    "alt-drag".to_owned(),
                ),
                binding(
                    key("Copy what is selected").with_keywords(&["clipboard", "yank"]),
                    chord("cmd-c", "ctrl-c or ctrl-shift-c"),
                ),
                widgets::note(
                    "A selection in the output owns the copy chord for as long as it exists, \
                     and copying lets go of it — which is the only sign a copy happened. That \
                     is what settles ctrl-c off macOS, where the same key is also the \
                     interrupt: with nothing selected it interrupts exactly as it always has, \
                     and because copying releases the selection, the very next press does too.",
                    ui,
                ),
                widgets::note(
                    "The copy leaves the command field alone. A selection is also let go of by \
                     typing, by clicking into the field, and by clicking elsewhere in the \
                     output — but never by the shell printing, which is exactly when somebody \
                     is reading what is already on screen.",
                    ui,
                ),
                widgets::note(
                    "A selection lives in the command that is still running, and nowhere else. \
                     A finished command's rows have been copied out of the terminal — which is \
                     what makes them survive a clear, a resize and the scrollback filling up — \
                     and a selection has to stay in the terminal to stay anchored to its own \
                     text while output arrives. A press on a finished command lets go of \
                     whatever was selected rather than starting a new selection; its copy \
                     control takes the whole of it.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "The command field",
            vec![
                binding(
                    key("Send the line to the shell")
                        .with_keywords(&["run", "execute", "submit", "return"]),
                    "enter".to_owned(),
                ),
                binding(
                    key("Lengthen it by a line").with_keywords(&[
                        "multiline",
                        "newline",
                        "continue",
                    ]),
                    "shift-enter".to_owned(),
                ),
                binding(
                    key("Walk this pane's history").with_keywords(&["previous", "recall", "arrow"]),
                    "up / down".to_owned(),
                ),
                widgets::note(
                    "Everything else in the field is the text editing this platform already \
                     does. ctrl-c interrupts the shell and throws the half-written line away \
                     with it, ctrl-z suspends, and ctrl-d ends the input when the field is empty \
                     and deletes a character when it is not.",
                    ui,
                ),
                widgets::note(
                    "The field goes away, and every key reaches the program instead, in three \
                     cases: a full-screen program — vim, `top` — is up, the shell has reported \
                     a command running for longer than a blink, or the output is being drawn as \
                     a plain terminal because the shell reports no command boundaries at all.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "Window",
            vec![
                binding(
                    key("Move the tabs panel").with_keywords(&[
                        "sidebar",
                        "strip",
                        "placement",
                        "layout",
                    ]),
                    chord("cmd-b", "ctrl-shift-b"),
                ),
                binding(
                    key("Open these settings").with_keywords(&["preferences", "options", "config"]),
                    chord("cmd-,", "ctrl-,"),
                ),
                binding(
                    key("Make the text bigger / smaller")
                        .with_keywords(&["zoom", "font", "size", "scale"]),
                    chord("cmd-+ / cmd--", "ctrl-+ / ctrl--"),
                ),
                binding(
                    key("Put the text back to its size").with_keywords(&["zoom", "reset", "font"]),
                    chord("cmd-0", "ctrl-0"),
                ),
                widgets::note(
                    "These settings are a pane, like a session is, so they close the way every \
                     pane does and have no key of their own for it. Pressing the binding again \
                     brings this tab forward rather than closing it.",
                    ui,
                ),
                widgets::fact(
                    Words::new("Keymap file").with_keywords(&[
                        "bindings",
                        "shortcuts",
                        "chords",
                        "rebind",
                        "keymap",
                    ]),
                    crate::keymap::user_keymap_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| {
                            "nowhere — this machine has no configuration directory".to_owned()
                        }),
                    true,
                    fonts,
                ),
                widgets::note(
                    "A chord in that file wins over the one above it, and \"none\" takes a chord \
                     away — which is how one is given back to a shell or an editor that wants \
                     it. It is read when Crook starts. What a pane does with a key is not in it: \
                     ctrl-c interrupts and ctrl-d ends an input, and a keymap that could take \
                     one of those away would be one that breaks a terminal.",
                    ui,
                ),
                widgets::note(
                    "The bindings are fixed. Warp has editable keymaps with context predicates; \
                     Crook reads input directly, and the keyboard produces exactly the values the \
                     mouse produces — which is what lets a keymap be added later without touching \
                     a single handler.",
                    ui,
                ),
            ],
        ),
        named_actions(workspace),
    ]
}
