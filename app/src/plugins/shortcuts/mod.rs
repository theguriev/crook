//! The Keyboard Shortcuts page: every command, the chord that reaches it, and
//! the control that changes it.
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
//! # Any key on any action, from here
//!
//! VSCode's editor records a chord from the keyboard and writes the file for
//! you, and so does this one. A row's chord is a button: clicking it starts a
//! recording, every key pressed while one is up belongs to it —
//! [`Workspace::action_for`](crate::workspace::Workspace::action_for) hands
//! them over before anything else in the window can want them, which is what
//! makes a chord a pane would otherwise eat recordable at all — Enter keeps
//! what was spelled and Escape leaves the binding alone.
//!
//! Keeping a chord writes VSCode's own two lines into `keybindings.json`: the
//! command is taken off every chord it had, and then given the one recorded.
//! The button beside the chord is whichever of "Reset" and "Unbind" the row
//! can still be asked for. Nothing else in the file is touched — see
//! [`Document`](crate::keybindings::Document), which edits the *text* so that
//! a comment somebody wrote survives a button being clicked in a settings
//! page.
//!
//! Two keys this page cannot record, for the reason VSCode cannot record them
//! either: Enter and Escape are how a recording ends. The file can bind both,
//! and the path to it is on the page.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::keybindings::{self, Recording, Resolution, Source};
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};
use crate::workspace::settings_page::{SettingsState, keyed};
use crate::workspace::{Fonts, SettingsAction, Workspace, WorkspaceAction};

/// What the page is called, in the rail and to `--settings`.
pub const PAGE_TITLE: &str = "Keyboard Shortcuts";

/// The plugin that owns the Keyboard Shortcuts page.
pub struct Shortcuts;

impl Plugin for Shortcuts {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", PAGE_TITLE, 30, |workspace, _| shortcuts(workspace));

        // The same recording the page's own chord button starts, reachable by
        // name. It is here because the thing that most wants it is not this
        // page: a chip that says which chord starts an agent is the natural
        // place to offer "change that", and that chip may well be a plugin
        // outside the binary — which can hold `Capability::RunCommands` naming
        // this one command and nothing else. Which command it is about arrives
        // through `Host::said`, an action having no argument of its own.
        //
        // An action rather than a command: offered in the palette with nothing
        // said it would record a chord for a name nobody gave, which is worse
        // than not being offered there at all.
        host.register_action(action("rebind"), |workspace, ctx| {
            let said = workspace.host().said();
            let Ok(command) = ActionName::parse(said.trim()) else {
                log::warn!("crook/shortcuts/rebind was asked to rebind {said:?}");
                return;
            };
            if workspace.host().action(&command).is_none() {
                log::warn!("crook/shortcuts/rebind was asked about {command}, which is nothing");
                return;
            }
            // The recorder is a *row* of this page, so the page has to be the
            // thing on screen: a recording nobody can see is a keyboard that
            // has quietly stopped reaching the shell.
            let page = workspace.settings_page_named(PAGE_TITLE);
            workspace.open_settings_page(page, ctx);
            workspace.start_recording(command, ctx);
        });

        Ok(())
    }
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
        name: PAGE_TITLE,
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
    let state = workspace.settings_page();
    let recording = workspace.recording();

    let mut categories: Vec<Category> = Vec::new();
    let mut owners: Vec<&PluginId> = Vec::new();

    for (owner, command, title) in host.commands() {
        let recording = recording
            .as_ref()
            .filter(|recording| recording.command() == command);
        let row = binding_row(workspace, command, title, recording, state, fonts);

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

/// One command: what it is called, what reaches it, and how to change that.
///
/// The row is VSCode's row, in the three controls a settings page has room
/// for: the chord itself is the button that records a new one, and the button
/// beside it is whichever of "Reset" and "Unbind" this command can still be
/// asked for.
fn binding_row(
    workspace: &Workspace,
    command: &ActionName,
    title: &str,
    recording: Option<&Recording>,
    state: &SettingsState,
    fonts: Fonts,
) -> Entry {
    let keybindings = workspace.keybindings();
    let chords = keybindings.chords_for(command);
    let id = workspace.host().action(command);
    // A run with nowhere to write is a run where every one of these does
    // nothing, and a control that does nothing is drawn dead rather than
    // pretending: see `the_file`, which says where the file would have been.
    let editable = keybindings.is_editable();
    let editing =
        |action: SettingsAction| -> Option<WorkspaceAction> { editable.then_some(action.into()) };

    let description = match recording {
        Some(recording) => recorded(workspace, recording),
        None => match keybindings.source_of(command) {
            Some(source) => format!("{command} · {}", source.label()),
            None => command.to_string(),
        },
    };

    // Every chord, not the first one: somebody asking "why did this fire"
    // needs to see the second one.
    let value = match chords.is_empty() {
        true => "not bound".to_owned(),
        false => chords.join(", "),
    };
    let shown = match recording {
        // A recorder with nothing in it yet still has to be the widest thing
        // it will be, or the row jumps sideways on the first keystroke.
        Some(recording) if recording.is_empty() => "press a chord".to_owned(),
        Some(recording) => recording.chord(),
        None => value.clone(),
    };

    let words = Words::new(title.to_owned())
        .with_description(description)
        .with_keywords(&[
            &value, "command", "chord", "binding", "shortcut", "key", "rebind", "change",
        ]);

    let chord = widgets::chord_button(
        shown,
        recording.is_some(),
        !chords.is_empty(),
        match recording {
            // Clicking the box that is recording keeps what is in it, which is
            // what Enter does. Somebody who reached for the mouse instead of
            // reading the line under the row still gets the binding.
            Some(_) => Some(SettingsAction::KeepBinding.into()),
            None => id.and_then(|id| editing(SettingsAction::RecordBinding(id))),
        },
        state.control(keyed("chord", command)),
        fonts,
    );

    // One slot, and what is in it is whatever this command can still be asked
    // for: a recording is cancelled, a binding of the person's own is put
    // back, and anything else is taken away. Three labels rather than three
    // buttons, because a row with a live control for every state it is not in
    // is a row nobody can read at a glance.
    let (label, action) = match (recording.is_some(), keybindings.is_yours(command)) {
        (true, _) => ("Cancel", Some(SettingsAction::StopRecording.into())),
        (false, true) => (
            "Reset",
            id.and_then(|id| editing(SettingsAction::ResetBinding(id))),
        ),
        (false, false) => (
            "Unbind",
            match chords.is_empty() {
                true => None,
                false => id.and_then(|id| editing(SettingsAction::UnbindCommand(id))),
            },
        ),
    };
    let button = widgets::text_button(
        label,
        action,
        state.control(keyed("binding", command)),
        fonts.ui,
    );

    widgets::row(words, true, widgets::pair(chord, button), fonts.ui)
}

/// The line under a row that is recording: what to press, and what is in the
/// way.
///
/// The second half is the one that matters and it is the one VSCode has: a
/// chord that already reaches something else still works — the newer rule
/// wins — but it wins *silently*, and somebody who has just taken `ctrl+shift+d`
/// away from splitting a pane should find that out here rather than the next
/// time they reach for it.
fn recorded(workspace: &Workspace, recording: &Recording) -> String {
    let mut line = "Press the chord. Enter keeps it, Escape leaves it alone.".to_owned();

    if let Resolution::Command(taken) = workspace
        .keybindings()
        .resolve(recording.keys(), &workspace.key_context())
        && &taken != recording.command()
    {
        let name = workspace
            .host()
            .title_of(&taken)
            .map(str::to_owned)
            .unwrap_or_else(|| taken.to_string());
        line.push_str(&format!(" {} is {name}.", recording.chord()));
    }

    line
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
             the first, and while it is waiting every key belongs to the sequence. A file \
             edited by hand is read when Crook starts; a chord recorded on this page takes \
             effect at once.",
            ui,
        ),
        widgets::note(
            "This page writes to that file, and to nothing else in it: click a chord above to \
             record a new one, Enter to keep it and Escape to leave it alone. What is written \
             is the same two lines VSCode writes — the command taken off the chords it had, \
             and then the chord you pressed — so everything else in the file, comments \
             included, stays where you put it.",
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
