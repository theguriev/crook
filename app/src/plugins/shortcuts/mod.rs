//! The Keyboard Shortcuts page: every command, and the chord that reaches it.
//!
//! # Nothing on this page is written down here
//!
//! It used to be. The page was a hand-written list of the chords the window
//! answered to, in prose, next to a `match` in another file that decided what
//! those chords actually did — two lists that had to be kept in step by
//! somebody remembering to. This is the list VSCode's Keyboard Shortcuts
//! editor is: the *commands* come from the host, so a plugin that registers
//! one appears here the moment it loads, and the *chords* come from
//! [`crate::keybindings`], so a person who rebound one sees their own binding
//! rather than the one this build shipped with.
//!
//! What that leaves written down here is the one thing that is not a
//! keybinding at all: what a pane does with a key. Those cannot be bound —
//! `ctrl-c` interrupts and `ctrl-d` ends an input — and a page that left them
//! out would be a page that answers "what does this key do" with silence for
//! the keys somebody presses most.
//!
//! # It can change one, now
//!
//! It could not, and its own doc explained why: recording a chord needs a
//! control that takes over the keyboard, and the settings page has no text
//! input and no popup. It has one now — not on the page, but over the window,
//! where `crook/window`'s overlay slot puts anything that floats. Every row
//! carries a **Change** button, `crook/shortcuts/rebind` is the same thing by
//! name for anything that would rather ask than draw a button, and
//! [`recorder`] is the surface both of them put up. The path to the file is
//! still on the page, and the format is still documented beside it, because
//! this control writes one line of that format and a person's own file is
//! still the authority.
//!
//! # Why the rebinding is a command with an argument
//!
//! Because the thing that most wants it is not this page. A chip that says
//! which chord starts an agent is the natural place to offer "change that",
//! and that chip may well be a plugin outside the binary — which can hold
//! [`Capability::RunCommands`](crook_plugin_api::Capability::RunCommands)
//! naming this one command and nothing else. So the flow is reachable by name
//! and told *which* command through `Host::said`, and the button on this page
//! is one caller of it rather than the only way in.

use crookui_core::prelude::*;

use std::rc::Rc;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::keybindings::{self, Keybindings, Source};
use crate::plugin::{BuildError, Host, Plugin};
use crate::theme::theme;
use crate::workspace::settings_page::named;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};
use crate::workspace::{Workspace, WorkspaceAction};

use super::window::WINDOW_OVERLAY;

mod recorder;

use recorder::Recorder;

/// The plugin that owns the Keyboard Shortcuts page.
pub struct Shortcuts;

impl Plugin for Shortcuts {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", "Keyboard Shortcuts", 30, |workspace, _| {
            shortcuts(workspace)
        });

        let recorder = Rc::new(Recorder::new(
            recorder::Names {
                record: action("record"),
                cancel: action("cancel"),
                ignore: action("ignore"),
            },
            host.fonts(),
        ));
        // Every keystroke, while it is up: the chord being bound is very
        // likely one the window itself answers to, and a recorder that let
        // those through could not record them. See `recorder`.
        let showing = host.claim_surface({
            let recorder = recorder.clone();
            move |keystroke| recorder.claims(keystroke)
        });
        recorder.armed_by(showing);

        // A command rather than an action, so a person can find it: it is
        // offered with no argument from the palette and does nothing there,
        // which is worse than not offering it. So: an action, reachable by
        // name and by anything holding the capability, and a button on every
        // row of the page for the way a person finds it.
        host.register_action(action("rebind"), {
            let recorder = recorder.clone();
            move |workspace, ctx| {
                // Which command is what the caller said before it ran this.
                let said = workspace.host().said();
                let Ok(command) = ActionName::parse(said.trim()) else {
                    log::warn!("crook/shortcuts/rebind was asked to rebind {said:?}");
                    return;
                };
                let title = workspace
                    .host()
                    .title_of(&command)
                    .unwrap_or(command.as_str())
                    .to_owned();
                recorder.open(command, title);
                workspace.sync_input_keys();
                ctx.notify();
            }
        });

        host.register_action(action("record"), {
            let recorder = recorder.clone();
            move |workspace, ctx| {
                let Some((command, chord)) = recorder.recorded() else {
                    return;
                };
                match recorder::bind(&command, &chord) {
                    Ok(path) => {
                        log::info!("bound {chord} to {command} in {}", path.display());
                        // Read back rather than added to the table in memory:
                        // the file is the authority, and a binding that only
                        // existed in this process would disappear at the next
                        // launch without anybody being told.
                        workspace.set_keybindings(Keybindings::load(&path));
                        recorder.close();
                        workspace.sync_input_keys();
                    }
                    // Left up, saying what went wrong: a person who has just
                    // pressed three keys deserves to be told they landed
                    // nowhere, and to be able to try again.
                    Err(why) => recorder.failed(why),
                }
                ctx.notify();
            }
        });

        for name in ["cancel", "ignore"] {
            let recorder = recorder.clone();
            // Two actions, one handler, and the difference is the name: a
            // modifier on its way to a chord is claimed and does nothing,
            // Escape is claimed and takes the recorder down.
            host.register_action(action(name), move |workspace, ctx| {
                if name == "cancel" {
                    recorder.close();
                    workspace.sync_input_keys();
                    ctx.notify();
                }
            });
        }

        host.contribute(WINDOW_OVERLAY, "recorder", 10, {
            let recorder = recorder.clone();
            move |_, _| recorder::render(&recorder)
        });

        Ok(())
    }
}

/// One command's row: what it is called, what reaches it, and a way to change
/// that.
///
/// The chord is printed in the monospace family the page prints every other
/// chord in, and the button beside it stands for *this* command — which is
/// what the voice carries, an action having no argument of its own.
fn rebindable(
    workspace: &Workspace,
    command: &ActionName,
    title: &str,
    described: String,
    printed: String,
    fonts: crate::workspace::Fonts,
) -> Entry {
    let words = Words::new(title.to_owned())
        .with_description(described)
        .with_keywords(&[
            "command", "chord", "binding", "shortcut", "key", "rebind", "change",
        ]);

    let control = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(printed.clone(), fonts.monospace, 10.5)
                .with_color(theme().text_primary)
                .finish(),
        )
        .with_child(
            Container::new(widgets::text_button_about(
                "Change",
                Some((workspace.host().voice(), command.to_string())),
                workspace
                    .host()
                    .action(&action("rebind"))
                    .map(WorkspaceAction::Run),
                workspace
                    .settings_page()
                    .control(named(&format!("rebind.{command}"))),
                fonts.ui,
            ))
            .with_margin_left(10.)
            .finish(),
        )
        .finish();

    // What the page's search matches on the right of a row. `widgets::row`
    // does not fill it in — a switch has no text to find — and this row does:
    // typing a chord into the box should find the command it runs.
    widgets::row(words, true, control, fonts.ui).saying(printed)
}

/// `crook/shortcuts/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/shortcuts/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/shortcuts").expect("a literal that parses"),
        name: "Keyboard Shortcuts",
        description: "Every command the window answers to, and the chord that reaches it.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// The page.
fn shortcuts(workspace: &Workspace) -> Vec<Category> {
    let mut categories = commands(workspace);
    if let Some(category) = your_bindings(workspace) {
        categories.push(category);
    }
    categories.push(the_file(workspace));
    categories.push(in_a_pane(workspace));
    categories
}

/// Every command, grouped by the plugin that registered it.
///
/// In registration order, which is load order, which is the order the sidebar
/// and the settings rail are already in — the window's own commands first.
/// Nothing here enumerates a command: a plugin that adds one adds a row.
fn commands(workspace: &Workspace) -> Vec<Category> {
    let fonts = workspace.fonts();
    let host = workspace.host();
    let keybindings = workspace.keybindings();

    let mut categories: Vec<Category> = Vec::new();
    let mut owners: Vec<&PluginId> = Vec::new();

    for (owner, command, title) in host.commands() {
        let chords = keybindings.chords_for(command);
        let source = keybindings.source_of(command);

        let described = match source {
            Some(source) => format!("{command} · {}", source.label()),
            None => command.to_string(),
        };
        // Every chord, not the first one: somebody asking "why did this fire"
        // needs to see the second one.
        let printed = match chords.is_empty() {
            true => "not bound".to_owned(),
            false => chords.join(", "),
        };
        let row = rebindable(workspace, command, title, described, printed, fonts);

        match owners.iter().position(|known| *known == owner) {
            Some(at) => categories[at].entries.push(row),
            None => {
                owners.push(owner);
                categories.push(widgets::category(plugin_name(workspace, owner), vec![row]));
            }
        }
    }

    categories
}

/// What a plugin calls itself, for a category heading.
fn plugin_name(workspace: &Workspace, owner: &PluginId) -> String {
    workspace
        .host()
        .loaded()
        .iter()
        .find(|manifest| &manifest.id == owner)
        .map(|manifest| manifest.name.to_owned())
        .unwrap_or_else(|| owner.to_string())
}

/// The rules out of the person's own file, printed back to them.
///
/// `None` when there are none, so an untouched install is not given an empty
/// heading. This is the half of the page that answers "why is my file not
/// doing anything": a line that could not be read is not here, because it was
/// dropped when the file was read, and a line that names a command nothing
/// answers to says so.
fn your_bindings(workspace: &Workspace) -> Option<Category> {
    let fonts = workspace.fonts();
    let keybindings = workspace.keybindings();

    let entries: Vec<Entry> = keybindings
        .effective()
        .into_iter()
        .filter(|rule| rule.source == Source::User)
        .map(|rule| {
            let mut description = match workspace.host().action(&rule.command) {
                Some(_) => "From your keybindings.json.".to_owned(),
                // The one thing a file cannot tell a person, and the ordinary
                // state of any file copied from somebody else: the line is
                // fine and the plugin it names is not installed.
                None => "Nothing answers to this name.".to_owned(),
            };
            if let Some(clause) = &rule.when {
                description.push_str(&format!(" While {}.", clause_text(clause)));
            }
            widgets::fact(
                Words::new(rule.command.to_string())
                    .with_description(description)
                    .with_keywords(&["yours", "keybindings.json", "override", "custom"]),
                rule.chord(),
                true,
                fonts,
            )
        })
        .collect();

    (!entries.is_empty()).then(|| widgets::category("Your keybindings", entries))
}

/// The context keys a clause turns on, for a row that has no room for the
/// clause itself.
fn clause_text(clause: &keybindings::When) -> String {
    let mut names = clause.names();
    names.dedup();
    format!("it depends on {}", names.join(", "))
}

/// Where the file is, what goes in it, and what a `when` clause may name.
fn the_file(workspace: &Workspace) -> Category {
    let fonts = workspace.fonts();
    let ui = fonts.ui;

    let mut entries = vec![
        widgets::fact(
            Words::new("Keybindings file").with_keywords(&[
                "bindings",
                "shortcuts",
                "chords",
                "rebind",
                "keybindings.json",
                "customise",
            ]),
            keybindings::user_keybindings_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    "nowhere — this machine has no configuration directory".to_owned()
                }),
            true,
            fonts,
        ),
        widgets::note(
            "A list of bindings, in VSCode's format and with VSCode's rules: \
             { \"key\": \"ctrl+shift+t\", \"command\": \"crook/window/new-tab\" }. The last rule \
             that matches a chord wins, so a line of yours beats the one this build ships \
             with, and a later line of yours beats an earlier one. Comments are allowed.",
            ui,
        ),
        widgets::note(
            "A `-` in front of a command takes it off that chord — \
             { \"key\": \"ctrl+shift+t\", \"command\": \"-crook/window/new-tab\" } — which is how \
             a chord is given back to a shell or an editor that wants it. A removal with no \
             \"key\" at all takes every chord that command has.",
            ui,
        ),
        widgets::note(
            "A \"key\" may be a sequence: \"ctrl+k ctrl+s\" waits for the second chord after \
             the first, and while it is waiting every key belongs to the sequence. The file is \
             read when Crook starts.",
            ui,
        ),
        widgets::note(
            "A \"when\" clause limits a binding to a condition: \"!searchFocused && \
             paneFocused\". The grammar is VSCode's — &&, ||, !, ==, != and brackets — over \
             the keys below.",
            ui,
        ),
    ];

    for (key, value) in workspace.key_context().keys() {
        entries.push(widgets::fact(
            Words::new(key.to_owned())
                .with_description("A key a \"when\" clause may name.")
                .with_keywords(&["when", "condition", "context", "clause"]),
            format!("now: {value}"),
            true,
            fonts,
        ));
    }

    widgets::category("Your own bindings", entries)
}

/// The keys that are the pane's, and cannot be bound to anything else.
fn in_a_pane(workspace: &Workspace) -> Category {
    let fonts = workspace.fonts();
    let ui = fonts.ui;
    let key = |label: &str, keywords: &[&str], value: &str| {
        widgets::fact(
            Words::new(label.to_owned()).with_keywords(keywords),
            value.to_owned(),
            true,
            fonts,
        )
    };

    widgets::category(
        "In a pane",
        vec![
            key(
                "Send the line to the shell",
                &["run", "execute", "submit", "return"],
                "enter",
            ),
            key(
                "Lengthen it by a line",
                &["multiline", "newline", "continue"],
                "shift+enter",
            ),
            key(
                "Walk this pane's history",
                &["previous", "recall", "arrow"],
                "up / down",
            ),
            key(
                "Complete the word, and step through what the shell offered",
                &["completion", "complete", "tab", "suggest"],
                "tab / shift+tab",
            ),
            key(
                "Take the suggestion standing after the caret",
                &["autosuggest", "ghost", "history", "completion", "accept"],
                match crate::input_keys::Platform::current() {
                    crate::input_keys::Platform::Mac => "right / alt+right for one word",
                    crate::input_keys::Platform::Other => "right / ctrl+right for one word",
                },
            ),
            key(
                "Interrupt, suspend, end the input",
                &["signal", "sigint", "ctrl-c", "ctrl-d", "ctrl-z"],
                "ctrl+c / ctrl+z / ctrl+d",
            ),
            key(
                "Select a run of the output",
                &["mouse", "drag", "highlight", "word", "line", "column"],
                "drag / double / triple click / alt+drag",
            ),
            key(
                "Copy what is selected",
                &["clipboard", "yank"],
                match crate::input_keys::Platform::current() {
                    crate::input_keys::Platform::Mac => "cmd+c",
                    crate::input_keys::Platform::Other => "ctrl+c or ctrl+shift+c",
                },
            ),
            key(
                "Copy a whole command and its output",
                &["clipboard", "block", "yank"],
                "hover it, then click",
            ),
            widgets::note(
                "None of these can be rebound, and that is the line the keybindings stop at: \
                 what a pane does with a key belongs to the program in it. ctrl-c interrupts, \
                 ctrl-d ends an input and ctrl-z suspends, and a keybinding that could take one \
                 of those away would be one that breaks a terminal.",
                ui,
            ),
            widgets::note(
                "A selection in the output owns the copy chord for as long as it exists, and \
                 copying lets go of it — which is the only sign a copy happened. That is what \
                 settles ctrl-c off macOS, where the same key is also the interrupt: with \
                 nothing selected it interrupts exactly as it always has, and because copying \
                 releases the selection, the very next press does too.",
                ui,
            ),
            widgets::note(
                "The field goes away, and every key reaches the program instead, in three \
                 cases: a full-screen program — vim, `top` — is up, the shell has reported a \
                 command running for longer than a blink, or the output is being drawn as a \
                 plain terminal because the shell reports no command boundaries at all.",
                ui,
            ),
        ],
    )
}
