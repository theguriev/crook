//! Every chord the window answers to: the ones compiled in, the ones a plugin
//! asked for, and the ones a person wrote down.
//!
//! # VSCode's arrangement, and why this is it
//!
//! There used to be a table of overrides here — one chord, one action, and a
//! rule that a chord in the file won over the built-in binding for the same
//! chord. It could say nothing about *when* a binding applies, it could not
//! see the built-in table it was overriding, and the built-in table was a
//! `match` in another file that nothing could enumerate. A settings page that
//! wanted to print "what does cmd-t do" had to be told, by hand, in prose.
//!
//! This is VSCode's model instead, whole:
//!
//! * **Everything is a rule**, and a rule is `{ key, command, when }`. The
//!   defaults are rules in [`DEFAULTS_MAC`] and [`DEFAULTS_OTHER`], a plugin's
//!   are rules it asked for through
//!   [`Host::suggest_binding`](crate::plugin::Host::suggest_binding), and a
//!   person's are rules in `keybindings.json`. One kind of thing, one table,
//!   one order.
//! * **The last rule that matches wins.** The layers are concatenated —
//!   plugin, then built-in, then the person's file — so a person's rule beats
//!   a default, a default beats a plugin's suggestion, and within a file the
//!   later line beats the earlier one.
//! * **A command is removed by name**, with a `-` in front of it:
//!   `{ "key": "cmd+t", "command": "-crook/window/new-tab" }` takes that chord
//!   away and gives the key back to the shell. This is the only way to unbind,
//!   exactly as it is in VSCode — with one addition: a removal that names no
//!   `"key"` takes *every* chord that command has, which VSCode has no
//!   spelling for and the zoom alone would otherwise cost eight lines of.
//! * **`when` says under what conditions a rule is in force** — see
//!   [`when`], which is VSCode's context-expression grammar.
//! * **A key is one chord or a sequence of them**, separated by a space:
//!   `"ctrl+k ctrl+s"`. The first chord of a sequence puts the window in chord
//!   mode, where the next keystroke completes the sequence or cancels it.
//!
//! # Where the file comes from
//!
//! A person writes it, or the Keyboard Shortcuts page does. Clicking a chord
//! on that page records one from the keyboard and [`Keybindings::bind`] writes
//! VSCode's own two lines for it — the command taken off every chord it had,
//! then the chord that was pressed — through [`document`], which edits the
//! *text* of the file rather than replacing it. That is what keeps somebody's
//! comments, their ordering and their line about a plugin they have not
//! installed yet: an edit made by clicking a button must not cost anything
//! that was written by hand.
//!
//! # What a command is
//!
//! An [`ActionName`] — `owner/plugin/action` — and there is no second kind.
//! The window's own commands are `crook/window/*`, registered by the
//! `crook/window` plugin like any other, which is what lets one table hold
//! both and what lets the settings page list every bindable thing without
//! being told any of them.
//!
//! A name is *not* resolved while the file is read. Which plugins are loaded
//! has a different answer at every moment of a window's life, so the name is
//! kept as written and looked up on every press: a name nothing answers to is
//! a chord that does nothing, which is right both for a typo and for a plugin
//! somebody has not installed.
//!
//! # What cannot be bound
//!
//! What a *pane* does with a key. `ctrl-c` interrupts, `ctrl-d` ends an input,
//! and a keybinding that could take one of those away would be one that breaks
//! a terminal. [`crate::input_keys::route`] is where that line is drawn and
//! nothing here reaches it.
//!
//! # Nothing here can cost a person their window
//!
//! A missing file, one that is not JSON, an entry that is not an object, a key
//! that is not a chord, a clause that does not parse, a command this build has
//! never heard of: each is one line in the log and one rule that is not
//! installed. A keybindings file that refused to load would be a file that
//! stops Crook opening.

use std::fs;
use std::path::{Path, PathBuf};

use crook_plugin::ActionName;
use crookui_core::event::{Keystroke, Modifiers};
use serde_json::Value;

use crate::input_keys::Platform;
use crate::settings::config_directory;

pub mod document;
pub mod when;

pub use document::Document;
pub use when::{Context, When};

/// The file a person writes their bindings in.
///
/// VSCode's name for VSCode's file, because it is VSCode's format.
const KEYBINDINGS_FILE: &str = "keybindings.json";

/// Where a rule came from — VSCode's "Source" column.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A plugin asked for it. The weakest layer: a plugin cannot take a chord
    /// from the window or from the person.
    Plugin,
    /// Compiled in.
    Default,
    /// The person's own `keybindings.json`.
    User,
}

impl Source {
    /// What the settings page prints in the source column.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plugin => "Plugin",
            Self::Default => "Default",
            Self::User => "User",
        }
    }
}

/// One line of a keybindings file: a chord sequence, a command, a condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The chords, in the order they are pressed. One for an ordinary
    /// binding; two or more for a sequence like `ctrl+k ctrl+s`.
    pub keys: Vec<Keystroke>,
    /// What it runs, or what it takes away when [`Rule::remove`] is set.
    pub command: ActionName,
    /// Whether this rule *removes* earlier rules rather than adding one: the
    /// `-` in front of a command name.
    pub remove: bool,
    /// The condition it is in force under, or `None` for always.
    pub when: Option<When>,
    /// Which layer it came from.
    pub source: Source,
}

impl Rule {
    /// A rule with no condition.
    fn new(keys: Vec<Keystroke>, command: ActionName, source: Source) -> Self {
        Self {
            keys,
            command,
            remove: false,
            when: None,
            source,
        }
    }

    /// Whether this rule's condition holds.
    fn applies(&self, context: &Context) -> bool {
        self.when
            .as_ref()
            .is_none_or(|clause| clause.evaluate(context))
    }

    /// The chords as a person would write them: `ctrl+k ctrl+s`.
    pub fn chord(&self) -> String {
        format_keys(&self.keys)
    }
}

/// What a keystroke turned out to mean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Run this. The name is looked up now, not when the file was read.
    Command(ActionName),
    /// The keys pressed so far begin a sequence: the window waits for the
    /// next keystroke rather than acting or passing it on.
    Chord,
    /// Nothing is bound to this. The keystroke belongs to whatever is under
    /// the window's own handling — which, in a pane, is the shell.
    Nothing,
}

/// Every rule in force, in the order they are consulted.
#[derive(Clone, Debug, PartialEq)]
pub struct Keybindings {
    /// What plugins asked for. Replaced once, after the plugins have built.
    plugin: Vec<Rule>,
    /// The table compiled in, for this platform.
    default: Vec<Rule>,
    /// What the person wrote down.
    user: Vec<Rule>,
    /// The person's file as it is written, which is what an edit made from
    /// the settings page edits. [`Self::user`] is what that text *means*, and
    /// it is derived from this every time it changes — one direction, so the
    /// rules in force and the file on disk cannot come apart.
    document: Document,
    /// Where that file is, or `None` for a run with nowhere to write: a test,
    /// the headless snapshot, a machine with no configuration directory.
    /// Nothing can be edited without one, and the page says so.
    path: Option<PathBuf>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self::new()
    }
}

impl Keybindings {
    /// The bindings this build ships with, and nothing else.
    ///
    /// What a test and the headless snapshot open with: a run with no settings
    /// file to write must not read the keybindings of whoever is running it.
    pub fn new() -> Self {
        Self::for_platform(Platform::current())
    }

    /// The shipped bindings as `platform` spells them.
    ///
    /// The platform is a parameter for the reason it is one in
    /// [`crate::input_keys`]: a table that can only be tested on the machine
    /// it was written on is a table with one half untested.
    pub fn for_platform(platform: Platform) -> Self {
        let table = match platform {
            Platform::Mac => DEFAULTS_MAC,
            Platform::Other => DEFAULTS_OTHER,
        };

        let default = table
            .iter()
            .map(|(keys, command)| {
                Rule::new(
                    parse_keys(keys).expect("a default chord that parses"),
                    ActionName::parse(command).expect("a default command that parses"),
                    Source::Default,
                )
            })
            .collect();

        Self {
            plugin: Vec::new(),
            default,
            user: Vec::new(),
            document: Document::default(),
            path: None,
        }
    }

    /// The shipped bindings with the person's file on top.
    pub fn for_user() -> Self {
        match user_keybindings_path() {
            Some(path) => Self::load(path),
            None => Self::new(),
        }
    }

    /// Reads the file at `path` over the shipped bindings.
    ///
    /// Tests point this at a scratch directory; nothing else should need to.
    ///
    /// A file that is not there is not an error and not a line in the log: it
    /// is what every install starts as, and the path is remembered all the
    /// same so that the first binding changed from the settings page has
    /// somewhere to be written.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let mut keybindings = Self::new();
        keybindings.path = Some(path.to_owned());
        let Ok(text) = fs::read_to_string(path) else {
            return keybindings;
        };
        keybindings.document = Document::new(text);
        keybindings.reread();
        keybindings
    }

    /// Puts the chords plugins asked for underneath everything else.
    ///
    /// Written after the fact rather than read with the rest because the
    /// plugins do not exist yet when the file is read: they are built by the
    /// workspace, and what they ask for is known only once they have.
    pub fn set_plugin_rules(&mut self, rules: Vec<Rule>) {
        self.plugin = rules;
    }

    /// Whether the person's file added nothing.
    pub fn is_empty(&self) -> bool {
        self.user.is_empty()
    }

    /// Every rule, weakest layer first, before removals are applied.
    fn layers(&self) -> impl Iterator<Item = &Rule> {
        self.plugin
            .iter()
            .chain(self.default.iter())
            .chain(self.user.iter())
    }

    /// Every rule that is still standing, in the order they are consulted.
    ///
    /// A `-command` rule takes out every rule *before* it with the same
    /// command and the same chords, and then goes away itself. Order matters
    /// twice over: a removal cannot reach a rule written after it, which is
    /// what lets a file remove a default and then bind the chord to something
    /// else in the next line.
    pub fn effective(&self) -> Vec<&Rule> {
        let mut kept: Vec<&Rule> = Vec::new();

        for rule in self.layers() {
            if !rule.remove {
                kept.push(rule);
                continue;
            }
            kept.retain(|standing| {
                standing.command != rule.command
                    // A removal with no chords at all takes every binding for
                    // the command, which VSCode has no spelling for and a
                    // person moving one command off the keyboard entirely
                    // otherwise has to write out one line per chord.
                    || (!rule.keys.is_empty() && standing.keys != rule.keys)
            });
        }

        kept
    }

    /// What this sequence of keystrokes means right now.
    ///
    /// VSCode's rule: of every rule whose condition holds, the **last** one
    /// that either matches the sequence exactly or begins with it decides. A
    /// rule that matches exactly runs its command; one that merely begins with
    /// it puts the window in chord mode.
    pub fn resolve(&self, keys: &[Keystroke], context: &Context) -> Resolution {
        if keys.is_empty() {
            return Resolution::Nothing;
        }

        let mut winner = None;
        for rule in self.effective() {
            if !rule.applies(context) {
                continue;
            }
            if rule.keys == keys || rule.keys.starts_with(keys) {
                winner = Some(rule);
            }
        }

        match winner {
            Some(rule) if rule.keys.len() == keys.len() => {
                Resolution::Command(rule.command.clone())
            }
            Some(_) => Resolution::Chord,
            None => Resolution::Nothing,
        }
    }

    /// Every chord that reaches `command`, as a person would write them.
    ///
    /// For the settings page, which lists commands rather than chords. The
    /// conditional ones are included: a row that hid a binding because its
    /// clause does not hold *while the settings page is open* would hide most
    /// of them.
    pub fn chords_for(&self, command: &ActionName) -> Vec<String> {
        let effective = self.effective();
        let mut chords: Vec<String> = Vec::new();

        for (at, rule) in effective.iter().enumerate() {
            if &rule.command != command {
                continue;
            }
            // A chord a later rule takes unconditionally is a chord this
            // command does not answer to any more, whatever the rule that
            // wanted it says. Somebody who has just put `ctrl+shift+d` on
            // another command would otherwise find it printed on two rows, one
            // of which is a lie — and it is the page's job to say which.
            //
            // Unconditionally, because a rule with a `when` takes the chord
            // only sometimes, and a row that went blank because of a clause
            // that does not hold right now would be a different lie.
            let taken = effective[at + 1..].iter().any(|later| {
                later.keys == rule.keys && later.when.is_none() && later.command != rule.command
            });
            let chord = rule.chord();
            // Two rules can reach the same command by the same chord — a
            // default and the person's own line restating it — and a row that
            // printed it twice would look like two ways to do one thing.
            if !taken && !chords.contains(&chord) {
                chords.push(chord);
            }
        }

        chords
    }

    /// Which layer the binding that reaches `command` came from.
    ///
    /// The strongest one, since that is the rule that is actually in force.
    pub fn source_of(&self, command: &ActionName) -> Option<Source> {
        self.effective()
            .into_iter()
            .filter(|rule| &rule.command == command)
            .map(|rule| rule.source)
            .next_back()
    }

    /// Whether there is a file to write, which is what makes a binding
    /// changeable from the settings page at all.
    ///
    /// False for a run with no configuration directory — a test, the headless
    /// snapshot, a machine that has none — where the page draws its controls
    /// dead rather than pretending an edit was kept.
    pub fn is_editable(&self) -> bool {
        self.path.is_some()
    }

    /// Whether the person's own file says anything about `command`.
    ///
    /// What the settings page offers "Reset" for: a command nobody has
    /// written a line about has nothing to put back.
    pub fn is_yours(&self, command: &ActionName) -> bool {
        self.user.iter().any(|rule| &rule.command == command)
    }

    /// Binds `command` to `keys`, and to nothing else.
    ///
    /// VSCode's own edit, in VSCode's own two lines: a removal that takes the
    /// command off every chord it had, and then the chord it is being given.
    /// The removal is what makes this "instead of" rather than "as well as" —
    /// without it, changing `ctrl+shift+t` to `ctrl+alt+n` would leave a
    /// window that opens a tab on both.
    ///
    /// The person's earlier lines about this command go first, so that
    /// changing one binding four times leaves two lines in the file rather
    /// than eight.
    ///
    /// `None` when there is nothing to write to, or when the file is one no
    /// edit may guess at — see [`Document::append`].
    pub fn bind(&mut self, command: &ActionName, keys: &[Keystroke]) -> Option<PendingSave> {
        if keys.is_empty() {
            return None;
        }

        self.forget(command);
        // Asked *after* the person's own lines have gone, so that what is
        // being removed is what the rest of the layers say — the shipped
        // chord, or a plugin's — rather than a line this edit is replacing.
        if self.is_bound(command) {
            self.document.append(&removal_entry(command));
        }
        if !self.document.append(&binding_entry(command, keys)) {
            log::warn!(
                "{} is not a list of bindings, so it was left alone",
                self.where_()
            );
            return None;
        }

        self.reread();
        self.pending()
    }

    /// Takes every chord away from `command`, and gives the keys back.
    ///
    /// The `-` line on its own. A command with nothing bound to it is not an
    /// error: it is how a chord is handed back to a shell or to an editor
    /// running in a pane.
    pub fn unbind(&mut self, command: &ActionName) -> Option<PendingSave> {
        self.forget(command);
        if self.is_bound(command) && !self.document.append(&removal_entry(command)) {
            log::warn!(
                "{} is not a list of bindings, so it was left alone",
                self.where_()
            );
            return None;
        }

        self.reread();
        self.pending()
    }

    /// Puts `command` back to what this build ships with.
    ///
    /// Every line the person wrote about it goes, and nothing replaces them,
    /// which is the whole of what "reset" can mean in a file where the
    /// defaults are not written down.
    pub fn reset(&mut self, command: &ActionName) -> Option<PendingSave> {
        self.forget(command);
        self.reread();
        self.pending()
    }

    /// Whether anything reaches `command` right now, under any condition.
    fn is_bound(&self, command: &ActionName) -> bool {
        self.effective().iter().any(|rule| &rule.command == command)
    }

    /// Takes every line the person wrote about `command` out of the document.
    ///
    /// Both kinds: the ones that bind it and the ones that remove it. What is
    /// left is a file that says nothing about the command at all, which is
    /// where each of the three edits starts.
    fn forget(&mut self, command: &ActionName) {
        let wanted = command.to_string();
        self.document.remove(|entry| {
            let named = entry.get("command").and_then(Value::as_str).unwrap_or("");
            named.strip_prefix('-').unwrap_or(named).trim() == wanted
        });
    }

    /// Reads the rules back out of the document.
    ///
    /// The one place [`Self::user`] is written after the file is loaded, so
    /// that a binding changed from the page means exactly what the same file
    /// would mean if Crook were started again on it — including a line that
    /// turns out not to parse, which is dropped here rather than believed.
    fn reread(&mut self) {
        let path = self.path.clone().unwrap_or_default();
        self.user = read_rules(self.document.text(), &path, Source::User);
    }

    /// The write this edit is asking for, for whoever has a thread to do it on.
    fn pending(&self) -> Option<PendingSave> {
        Some(PendingSave {
            path: self.path.clone()?,
            text: self.document.text().to_owned(),
        })
    }

    /// The file's path, for a line in the log.
    fn where_(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => KEYBINDINGS_FILE.to_owned(),
        }
    }
}

/// A chord being recorded on the Keyboard Shortcuts page.
///
/// VSCode's recorder, which is a small state machine and nothing else: it
/// holds the command whose chord is being spelled and the chords spelled so
/// far, and the page draws it. What it does *not* hold is the keyboard —
/// that is [`Workspace::action_for`](crate::workspace::Workspace::action_for),
/// which hands every keystroke here while a recording is up so that a chord a
/// pane would otherwise eat can still be recorded.
///
/// A recording is not a binding. Nothing is written until it is kept, so a
/// person who reaches for a chord, sees it is the wrong one and presses Escape
/// has changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recording {
    /// The command the chords are being recorded for.
    command: ActionName,
    /// The chords pressed so far, in order.
    keys: Vec<Keystroke>,
}

impl Recording {
    /// The most chords one binding may be spelled over here.
    ///
    /// VSCode's limit, for VSCode's reason: a recorder that went on
    /// accumulating would turn one fumbled keystroke into a binding nobody can
    /// press again, and there is no third chord in any editor's keymap. The
    /// *file* has no such limit — [`parse_keys`] reads as many as are written
    /// — because a file is read rather than fumbled.
    const MOST_CHORDS: usize = 2;

    /// A recording of nothing yet, for `command`.
    pub fn new(command: ActionName) -> Self {
        Self {
            command,
            keys: Vec::new(),
        }
    }

    /// Which command is being recorded for.
    pub fn command(&self) -> &ActionName {
        &self.command
    }

    /// The chords so far.
    pub fn keys(&self) -> &[Keystroke] {
        &self.keys
    }

    /// Whether anything has been pressed yet.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The chords so far, as a person would write them.
    pub fn chord(&self) -> String {
        format_keys(&self.keys)
    }

    /// Writes one keystroke down.
    ///
    /// Past [`Self::MOST_CHORDS`] the next press starts again rather than
    /// being dropped, which is VSCode's behaviour and the only one that lets
    /// somebody who has spelled the wrong sequence fix it without reaching for
    /// the mouse.
    pub fn press(&mut self, keystroke: &Keystroke) {
        if self.keys.len() >= Self::MOST_CHORDS {
            self.keys.clear();
        }
        self.keys.push(keystroke.clone());
    }

    /// Whether this keystroke is a modifier being held rather than a key being
    /// pressed.
    ///
    /// The platform reports a press for Shift itself on the way to
    /// `shift+cmd+k`, and a recorder that wrote those down would record
    /// "shift" every time somebody reached for a chord.
    pub fn is_a_modifier(keystroke: &Keystroke) -> bool {
        matches!(
            keystroke.key.as_str(),
            "shift" | "control" | "ctrl" | "alt" | "option" | "super" | "meta" | "cmd" | "command"
        )
    }
}

/// A keybindings file that has been changed and not yet written.
///
/// The edit lands in memory the moment it is made — the next keystroke is
/// resolved against it — and the file follows on a background thread, for the
/// reason [`Settings::save_blocking`](crate::settings::Settings::save_blocking)
/// does: writing one is a directory created, a file written, a flush waited
/// for and a rename, which is milliseconds on a good day and a stalled frame
/// on a bad one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSave {
    /// Where it goes.
    path: PathBuf,
    /// What goes there: the whole file, as the edit left it.
    text: String,
}

impl PendingSave {
    /// Writes it, atomically. Blocking; this belongs on the background pool.
    ///
    /// Atomically for the reason the settings are written that way: the bytes
    /// go to a temporary beside the target and a rename puts them in place, so
    /// a crash half-way through leaves the bindings that were working there
    /// rather than half a file that parses as nothing.
    pub fn write_blocking(&self) -> anyhow::Result<()> {
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        crate::settings::atomic_write(&self.path, self.text.as_bytes())
    }

    /// Where it would be written.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The line that binds a command to a chord sequence.
///
/// Written the way the documentation writes one and the way a person would:
/// one line, key first, which is the order VSCode's file uses.
fn binding_entry(command: &ActionName, keys: &[Keystroke]) -> String {
    format!(
        "{{ \"key\": {}, \"command\": {} }}",
        quoted(&format_keys(keys)),
        quoted(&command.to_string())
    )
}

/// The line that takes a command off every chord it has.
fn removal_entry(command: &ActionName) -> String {
    format!("{{ \"command\": {} }}", quoted(&format!("-{command}")))
}

/// `text` as a JSON string, escapes and all.
fn quoted(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// Where the per-user keybindings live.
pub fn user_keybindings_path() -> Option<PathBuf> {
    Some(config_directory()?.join(KEYBINDINGS_FILE))
}

/// Reads a keybindings document, dropping what cannot be read.
fn read_rules(text: &str, path: &Path, source: Source) -> Vec<Rule> {
    let document: Value = match serde_json::from_str(&strip_comments(text)) {
        Ok(document) => document,
        Err(error) => {
            log::warn!("could not read {}: {error}", path.display());
            return Vec::new();
        }
    };
    let Value::Array(entries) = document else {
        log::warn!("{} is not a list of bindings", path.display());
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| read_rule(entry, path, source))
        .collect()
}

/// One entry of the list, or `None` with a line in the log.
fn read_rule(entry: &Value, path: &Path, source: Source) -> Option<Rule> {
    let where_ = path.display();
    let Some(entry) = entry.as_object() else {
        log::warn!("{where_}: {entry} is not a binding");
        return None;
    };

    let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
    let (command, remove) = match command.strip_prefix('-') {
        Some(command) => (command, true),
        None => (command, false),
    };
    let command = match ActionName::parse(command.trim()) {
        Ok(command) => command,
        Err(_) => {
            log::warn!("{where_}: {command:?} is not a command name");
            return None;
        }
    };

    // A removal is allowed to name no chord, and then it takes every one of
    // that command's. Anything else without a chord binds nothing.
    let written = entry.get("key").and_then(Value::as_str).unwrap_or("");
    let keys = match (parse_keys(written), remove && written.trim().is_empty()) {
        (Some(keys), _) => keys,
        (None, true) => Vec::new(),
        (None, false) => {
            log::warn!("{where_}: {written:?} is not a chord");
            return None;
        }
    };

    let when = match entry.get("when").and_then(Value::as_str) {
        Some(clause) => match When::parse(clause) {
            Some(clause) => Some(clause),
            None => {
                log::warn!("{where_}: {clause:?} is not a condition");
                return None;
            }
        },
        None => None,
    };

    Some(Rule {
        keys,
        command,
        remove,
        when,
        source,
    })
}

/// The rules a plugin asked for, read the same way a file's are.
///
/// One entry point for both, so a plugin's default chord and a person's line
/// cannot mean different things by the same spelling.
pub fn rule_from(chord: &str, command: ActionName, source: Source) -> Option<Rule> {
    Some(Rule::new(parse_keys(chord)?, command, source))
}

/// JSON with `//` and `/* */` taken out, which is what VSCode writes.
///
/// The file VSCode creates for a person is entirely comments, and a person who
/// keeps one commented would otherwise lose every binding in it to a parse
/// error. Quoting is respected, so a `//` inside a string survives; escapes
/// are respected, so a `\"` does not end one.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(character) = characters.next() {
        if in_string {
            out.push(character);
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match character {
            '"' => {
                in_string = true;
                out.push(character);
            }
            '/' if characters.peek() == Some(&'/') => {
                for character in characters.by_ref() {
                    if character == '\n' {
                        // Kept, so that line numbers in a parse error still
                        // point at the line a person is looking at.
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if characters.peek() == Some(&'*') => {
                characters.next();
                let mut star = false;
                for character in characters.by_ref() {
                    if star && character == '/' {
                        break;
                    }
                    star = character == '*';
                }
            }
            _ => out.push(character),
        }
    }

    out
}

/// A sequence like `ctrl+k ctrl+s`, or `None` when any chord in it is not one.
pub fn parse_keys(written: &str) -> Option<Vec<Keystroke>> {
    let keys: Option<Vec<Keystroke>> = written.split_whitespace().map(parse_chord).collect();
    keys.filter(|keys| !keys.is_empty())
}

/// A chord like `cmd+shift+d`, or `None` when it names no key.
///
/// The modifiers are taken off the front one at a time and whatever is left is
/// the key, which is what makes `ctrl++` and `cmd+-` parse: the key is
/// everything after the last modifier, however many pluses are in it.
pub fn parse_chord(chord: &str) -> Option<Keystroke> {
    let mut rest = chord.trim();
    let mut modifiers = Modifiers::default();

    loop {
        let Some((head, tail)) = rest.split_once('+') else {
            break;
        };
        match head.to_ascii_lowercase().as_str() {
            // `win` and `super` are VSCode's spellings of the same key on the
            // two platforms where it is not called Command.
            "cmd" | "super" | "win" | "meta" => modifiers.cmd = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "opt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            // Not a modifier, so the rest of the string — pluses and all — is
            // the key. This is what makes `ctrl++` parse: the second plus
            // splits into an empty head, which is not a modifier, and what is
            // left is the `+` key.
            _ => break,
        }
        rest = tail;
    }

    let key = key_name(rest);
    // A chord with no key is a string of modifiers, which names nothing that
    // can be pressed.
    (!key.is_empty()).then(|| Keystroke::new(key, modifiers))
}

/// The name the window reports this key under.
///
/// A window reports what the platform gave it — `escape`, `pagedown`, `left` —
/// and a person writes what their editor taught them, which for half of these
/// is something else. The aliases are VSCode's own spellings plus the two
/// shortenings everybody types.
fn key_name(written: &str) -> String {
    let key = written.trim().to_ascii_lowercase();
    match key.as_str() {
        "esc" => "escape",
        "return" => "enter",
        "del" => "delete",
        "ins" => "insert",
        "pgup" | "pageup" => "pageup",
        "pgdn" | "pagedown" => "pagedown",
        "arrowleft" => "left",
        "arrowright" => "right",
        "arrowup" => "up",
        "arrowdown" => "down",
        _ => return key,
    }
    .to_owned()
}

/// Writes a sequence the way [`parse_keys`] reads one.
pub fn format_keys(keys: &[Keystroke]) -> String {
    keys.iter()
        .map(format_chord)
        .collect::<Vec<String>>()
        .join(" ")
}

/// Writes a chord the way [`parse_chord`] reads one.
///
/// The inverse, and tested as one: the settings page prints what a person
/// wrote in their own file, and printing it in a different notation from the
/// one the file uses would be the page teaching a syntax that does not parse.
/// The modifier order is VSCode's, so two chords with the same modifiers
/// always read the same however they were typed.
pub fn format_chord(keystroke: &Keystroke) -> String {
    let mut chord = String::new();
    for (present, name) in [
        (keystroke.modifiers.ctrl, "ctrl"),
        (keystroke.modifiers.shift, "shift"),
        (keystroke.modifiers.alt, "alt"),
        (keystroke.modifiers.cmd, "cmd"),
    ] {
        if present {
            chord.push_str(name);
            chord.push('+');
        }
    }
    chord.push_str(&keystroke.key);
    chord
}

/// The chords macOS opens with.
///
/// Command's, which is where a macOS application's chords live and where
/// nothing the input field wants can be. The pairs that look redundant are
/// not: which of `=` and `+` the platform reports for one physical key
/// depends on the layout and on whether Shift was held, and a chord table
/// matches the modifiers exactly.
pub const DEFAULTS_MAC: &[(&str, &str)] = &[
    ("cmd+t", "crook/window/new-tab"),
    ("cmd+w", "crook/window/close-pane"),
    ("cmd+d", "crook/window/split-right"),
    ("cmd+shift+d", "crook/window/split-down"),
    // Telegram's chord, which is what the search box's placeholder says. Free
    // on macOS: the field's own emacs bindings are ctrl-keys, and cmd-k is
    // nobody's here.
    ("cmd+k", "crook/window/search-tabs"),
    // The chord every browser and editor opens a find bar with. On macOS it
    // is free: the field spends ctrl on its emacs bindings and cmd on the
    // clipboard, and cmd-f is nobody's here.
    ("cmd+f", "crook/window/find"),
    // Stepping through the blocks. Not cmd+arrow: macOS text fields spend
    // that on document-start and document-end, and the composer is one. The
    // Option beside it is free there, and nothing in a terminal wants it.
    ("cmd+alt+up", "crook/window/select-block-up"),
    ("cmd+alt+down", "crook/window/select-block-down"),
    ("cmd+,", "crook/window/open-settings"),
    // Not cmd+shift+arrow, which every macOS text field spends on selecting to
    // the end of a line — the field needs it more than the tabs do, and
    // cmd+alt+arrow is where a Mac browser keeps its tabs anyway.
    ("cmd+alt+left", "crook/window/previous-tab"),
    ("cmd+alt+right", "crook/window/next-tab"),
    ("ctrl+cmd+left", "crook/window/move-tab-left"),
    ("ctrl+cmd+right", "crook/window/move-tab-right"),
    ("cmd+=", "crook/window/zoom-in"),
    ("ctrl+shift+cmd+=", "crook/window/zoom-in"),
    ("cmd++", "crook/window/zoom-in"),
    ("shift+cmd++", "crook/window/zoom-in"),
    ("shift+cmd+=", "crook/window/zoom-in"),
    ("cmd+-", "crook/window/zoom-out"),
    ("shift+cmd+-", "crook/window/zoom-out"),
    ("cmd+_", "crook/window/zoom-out"),
    ("shift+cmd+_", "crook/window/zoom-out"),
    ("cmd+0", "crook/window/zoom-reset"),
    // The tabs by position, which is every browser's arrangement and the one
    // Terminal.app and iTerm2 took from them — the ninth key is the *last*
    // tab rather than the ninth, because somebody with twenty tabs pressing it
    // means the one at the end. Free of the field: the cmd branch of
    // `chord_intent` names five letters and no digits.
    ("cmd+1", "crook/window/select-tab-1"),
    ("cmd+2", "crook/window/select-tab-2"),
    ("cmd+3", "crook/window/select-tab-3"),
    ("cmd+4", "crook/window/select-tab-4"),
    ("cmd+5", "crook/window/select-tab-5"),
    ("cmd+6", "crook/window/select-tab-6"),
    ("cmd+7", "crook/window/select-tab-7"),
    ("cmd+8", "crook/window/select-tab-8"),
    ("cmd+9", "crook/window/select-last-tab"),
    // Moving between the panes of a split. Not cmd+alt+arrow, which is
    // iTerm2's and is spent here on the tabs and the block selection; ctrl is
    // free on macOS *with a Shift*, because the field's emacs bindings are
    // bare-ctrl and Mission Control's are bare-ctrl-arrow.
    ("ctrl+shift+left", "crook/window/focus-pane-left"),
    ("ctrl+shift+right", "crook/window/focus-pane-right"),
    ("ctrl+shift+up", "crook/window/focus-pane-up"),
    ("ctrl+shift+down", "crook/window/focus-pane-down"),
    // iTerm2's own pair for the same gesture, and free: the cmd branch of
    // `chord_intent` names letters, so the brackets are nobody's.
    ("cmd+]", "crook/window/focus-next-pane"),
    ("cmd+[", "crook/window/focus-previous-pane"),
    // The scrollback, on the chord every terminal ever written uses for it.
    // Shift has always been what takes the wheel and the keyboard back from
    // whatever is running in the pane, which is why the bare page keys are
    // left to the program.
    ("shift+pageup", "crook/window/page-up"),
    ("shift+pagedown", "crook/window/page-down"),
    ("shift+cmd+pageup", "crook/window/scroll-to-top"),
    ("shift+cmd+pagedown", "crook/window/scroll-to-bottom"),
];

/// The chords Linux and Windows open with.
///
/// Ctrl-*Shift*, because plain ctrl-letter belongs to the tty: ctrl-c
/// interrupts, ctrl-d ends input and ctrl-w erases a word, and an application
/// that took them would be an application nobody could run a program in.
/// Every terminal emulator on Linux arrived at the same arrangement.
pub const DEFAULTS_OTHER: &[(&str, &str)] = &[
    ("ctrl+shift+t", "crook/window/new-tab"),
    ("ctrl+shift+w", "crook/window/close-pane"),
    ("ctrl+shift+d", "crook/window/split-right"),
    // Not ctrl+shift+D with a Shift already spent: the split pair takes the
    // two keys next to each other instead.
    ("ctrl+shift+e", "crook/window/split-down"),
    // With the Shift every one of Crook's own chords takes off macOS, and here
    // it is load-bearing twice over: bare ctrl-k is the field's "delete to the
    // end of the line", which a window binding would take from every pane.
    ("ctrl+shift+k", "crook/window/search-tabs"),
    // With the Shift, because bare ctrl-f is readline\'s "forward one
    // character" and the field needs it — the same reason every other chord
    // here carries a Shift the macOS one does not.
    ("ctrl+shift+f", "crook/window/find"),
    // Not ctrl+shift+arrow: that is the field's own selection-by-word off
    // macOS. Alt beside the ctrl is free there, and mirrors the cmd+alt the
    // Mac uses for the same step.
    ("ctrl+alt+up", "crook/window/select-block-up"),
    ("ctrl+alt+down", "crook/window/select-block-down"),
    // ctrl+comma without a Shift: the settings chord is the same on every
    // platform, and unlike the tab bindings it has no field gesture to stay
    // out of the way of.
    ("ctrl+,", "crook/window/open-settings"),
    // The tab chord of every browser and every terminal on Linux and Windows,
    // and it leaves ctrl+shift+arrow to the field, where it selects by word.
    ("ctrl+pageup", "crook/window/previous-tab"),
    ("ctrl+pagedown", "crook/window/next-tab"),
    ("ctrl+shift+pageup", "crook/window/move-tab-left"),
    ("ctrl+shift+pagedown", "crook/window/move-tab-right"),
    // ctrl+minus does not collide with anything a shell wants: the C0 range
    // has no code for it, so it was already sending a bare `-`.
    ("ctrl+=", "crook/window/zoom-in"),
    ("ctrl+shift+=", "crook/window/zoom-in"),
    ("ctrl++", "crook/window/zoom-in"),
    ("ctrl+shift++", "crook/window/zoom-in"),
    ("ctrl+-", "crook/window/zoom-out"),
    ("ctrl+shift+-", "crook/window/zoom-out"),
    ("ctrl+_", "crook/window/zoom-out"),
    ("ctrl+shift+_", "crook/window/zoom-out"),
    ("ctrl+0", "crook/window/zoom-reset"),
    // Alt rather than the Ctrl-Shift the rest of this table takes, and that is
    // not a lapse. `control_code` folds ctrl+shift+3, +4 and +8 to ESC, FS and
    // DEL, so a ctrl+shift+digit family would take Escape away from vim; Alt
    // is free of the field off macOS — `typed_intent` refuses to compose with
    // it — and alt+digit is what GNOME Terminal, xfce4-terminal and Tilix
    // already bind this to. GNOME Terminal mixes the two families exactly this
    // way, ctrl+shift+t to open a tab and alt+2 to reach one.
    ("alt+1", "crook/window/select-tab-1"),
    ("alt+2", "crook/window/select-tab-2"),
    ("alt+3", "crook/window/select-tab-3"),
    ("alt+4", "crook/window/select-tab-4"),
    ("alt+5", "crook/window/select-tab-5"),
    ("alt+6", "crook/window/select-tab-6"),
    ("alt+7", "crook/window/select-tab-7"),
    ("alt+8", "crook/window/select-tab-8"),
    ("alt+9", "crook/window/select-last-tab"),
    // Windows Terminal's and Tilix's pane-focus chord. Not ctrl+shift+arrow,
    // which is the field's own select-by-word off macOS.
    ("alt+left", "crook/window/focus-pane-left"),
    ("alt+right", "crook/window/focus-pane-right"),
    ("alt+up", "crook/window/focus-pane-up"),
    ("alt+down", "crook/window/focus-pane-down"),
    ("ctrl+shift+]", "crook/window/focus-next-pane"),
    ("ctrl+shift+[", "crook/window/focus-previous-pane"),
    // The same two scrollback chords the Mac table takes, which is the one
    // gesture xterm, GNOME Terminal, konsole, kitty and Windows Terminal all
    // spell identically. The ends of the scrollback are on Alt because
    // ctrl+home and ctrl+shift+home are both the field's off macOS, where
    // `buffer_chord` is Ctrl.
    ("shift+pageup", "crook/window/page-up"),
    ("shift+pagedown", "crook/window/page-down"),
    ("alt+home", "crook/window/scroll-to-top"),
    ("alt+end", "crook/window/scroll-to-bottom"),
];

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;
