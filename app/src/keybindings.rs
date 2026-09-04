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
//! # What a command is
//!
//! An [`ActionName`] — `owner/plugin/action` — and there is no second kind.
//! The window's own thirteen commands are `crook/window/*`, registered by the
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

pub mod when;

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
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let mut keybindings = Self::new();
        let Ok(text) = fs::read_to_string(path) else {
            // No file is the ordinary state, and not worth a line.
            return keybindings;
        };
        keybindings.user = read_rules(&text, path, Source::User);
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
        let mut chords: Vec<String> = Vec::new();
        for rule in self.effective() {
            let chord = rule.chord();
            // Two rules can reach the same command by the same chord — a
            // default and the person's own line restating it — and a row that
            // printed it twice would look like two ways to do one thing.
            if &rule.command == command && !chords.contains(&chord) {
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
];

#[cfg(test)]
#[path = "keybindings_tests.rs"]
mod tests;
