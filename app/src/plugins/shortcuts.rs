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
//! # Read-only, unlike VSCode's
//!
//! VSCode's editor records a chord from the keyboard and writes the file for
//! you. This one prints and does not edit, because recording a chord needs a
//! control that takes over the keyboard and the settings page has no text
//! input and no popup — see `settings_page::widgets`. The path to the file is
//! on the page for that reason, and the format is documented beside it.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::keybindings::{self, Source};
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};

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
        Ok(())
    }
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
        let row = widgets::fact(
            Words::new(title.clone())
                .with_description(described)
                .with_keywords(&["command", "chord", "binding", "shortcut", "key"]),
            match chords.is_empty() {
                true => "not bound".to_owned(),
                // Every chord, not the first one: somebody asking "why did
                // this fire" needs to see the second one.
                false => chords.join(", "),
            },
            true,
            fonts,
        );

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
