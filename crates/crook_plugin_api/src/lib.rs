//! What a sandboxed plugin and its host say to each other.
//!
//! Shared by both sides: the host links this crate, and so does every plugin
//! compiled to wasm. That is the whole reason it exists — a wire format
//! written down twice is a wire format that will disagree with itself.
//!
//! # It describes, it does not paint
//!
//! Nothing here is a colour, a pixel or a font. A [`Node`] says *what a thing
//! is* — a label, a badge, a row — and the host decides what that looks like
//! in the theme that happens to be in force. That is the line between the two
//! tiers, and it is what makes a sandboxed plugin survive a theme it has never
//! heard of, a display scale it was not written for, and a version of Crook
//! that draws badges differently.
//!
//! A native plugin gets `&mut PaintContext` and can do anything. This tier
//! cannot, and the question "does it need a `PaintContext`?" is exactly how a
//! feature is sorted into one tier or the other. See `docs/plugins.md`.
//!
//! # Versioned by one number
//!
//! [`ABI_VERSION`] is the whole compatibility story. A plugin says which
//! version it was built against and the host refuses anything it does not
//! know, by name, with a line a person can act on — rather than decoding a
//! shape that means something else now and drawing nonsense.
//!
//! The rule for changing this vocabulary is the one `docs/plugins.md` states:
//! **add a slot, never widen the vocabulary**. A new [`Node`] variant is a new
//! ABI version and a migration for everybody; a new slot is neither.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// What version of this vocabulary a plugin was built against.
///
/// Bumped when a shape below changes in a way that would make an older plugin
/// decode to something other than what it meant. Adding a variant to an enum
/// counts: postcard encodes a variant by its index, so an older host reading a
/// newer plugin's `Node` would read the wrong variant rather than fail.
///
/// **2** is the version a plugin can *do* something in. One added
/// [`Capability`] ([`Capability::ReadFiles`]), the [`Request`]/[`Answer`] pair
/// that lets a plugin ask the host to reach the network or read a file on its
/// behalf, and the six [`Node`] variants a panel needs. Version 1 could
/// describe a badge and register an action, which is a plugin that can say
/// what it already knew.
pub const ABI_VERSION: u32 = 3;

/// What a sandboxed plugin says about itself, before any of it runs.
///
/// Read by the host *before* the plugin is built, which is what lets a store
/// list a plugin, a person read what it wants, and a host refuse one asking
/// for something it will not grant — none of which may require running it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The version of this vocabulary the plugin was built against.
    pub abi: u32,
    /// `owner/name`, checked by the host against the same rules a native
    /// plugin's id follows.
    pub id: String,
    /// What a person sees in a list.
    pub name: String,
    /// One line, for the row under the name.
    pub description: String,
    /// The plugin's own version, for the store to compare.
    pub version: String,
    /// What it needs to be allowed to do. Everything not asked for is denied,
    /// and asking is not being granted.
    pub capabilities: Vec<Capability>,
}

/// Something a plugin has to be allowed to do.
///
/// Deny by default and enumerated rather than open, because a capability a
/// person cannot read is a capability they cannot refuse. Each is phrased as
/// the sentence the permission dialog will say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// Read the settings — every option, not a subset.
    ReadSettings,
    /// Read what is in the tab strip: how many tabs, what they are called,
    /// which is active. Not what is *in* a pane.
    ReadTabs,
    /// Read the working directory and git facts of the active pane.
    ReadWorkingDirectory,
    /// Reach the network, and only these hosts.
    ///
    /// A list rather than a flag, because "this plugin talks to the internet"
    /// is not a thing anybody can meaningfully agree to, and
    /// "api.github.com" is.
    Network(Vec<String>),
    /// Read and write the system clipboard.
    Clipboard,
    /// Keep a little state of its own between runs, in a file the host owns.
    Storage,
    /// Read files on this machine, and only these paths.
    ///
    /// Exact paths rather than a flag, for the reason the network is a list of
    /// hosts: "this plugin reads your files" is not a thing anybody can
    /// meaningfully agree to, and `~/.claude/.credentials.json` is. A leading
    /// `~` is the person's home directory and is the only thing expanded; a
    /// path holding `..` is refused by the host rather than resolved, so a
    /// granted path cannot be walked out of.
    ReadFiles(Vec<String>),
    /// List the *names* in a directory, and only under these roots.
    ///
    /// Names and whether each is a directory, never a byte of what is in one:
    /// that is what makes this a weaker thing to grant than [`ReadFiles`] and
    /// what lets it name a root rather than an exact path. "Read every file
    /// under your home directory" is not a sentence anybody should agree to;
    /// "see the names of the folders under your home directory" is what a
    /// directory picker actually needs.
    ///
    /// A leading `~` is the person's home directory, as everywhere else, and a
    /// path holding `..` is refused rather than resolved — a granted root
    /// cannot be walked out of.
    ///
    /// [`ReadFiles`]: Self::ReadFiles
    ListDirectories(Vec<String>),
    /// Type a command into the shell, and only these commands.
    ///
    /// Templates rather than a flag, and this is the strongest thing on the
    /// list: what a plugin types, the shell runs, as the person. So it is a
    /// list of exact commands with one `{}` in each where the argument goes —
    /// `cd {}`, `git switch {}` — the host fills the hole and quotes what goes
    /// in it, and a plugin granted `cd {}` cannot type anything else. "This
    /// plugin can run commands" is not a thing anybody can meaningfully agree
    /// to; "this plugin can `cd` somewhere" is.
    TypeCommands(Vec<String>),
    /// Run one of Crook's own commands, and only these.
    ///
    /// By exact name, for the reason the network is a list of hosts. A plugin
    /// that may ask for `crook/shortcuts/rebind` is a plugin that can offer
    /// "change this keybinding" on its own chip; a plugin that may run any
    /// command by name is a plugin that can close the window.
    RunCommands(Vec<String>),
    /// See what Crook can be asked to do, and which keys reach it.
    ///
    /// The command list and the chords bound to it, which is what a plugin
    /// needs to print a hint — and nothing about what is in a pane.
    ReadCommands,
}

impl Capability {
    /// The sentence a permission dialog says.
    pub fn sentence(&self) -> String {
        match self {
            Self::ReadSettings => "Read your settings".into(),
            Self::ReadTabs => "See what your tabs are called".into(),
            Self::ReadWorkingDirectory => "See which project the active pane is in".into(),
            Self::Network(hosts) => list_sentence("Reach ", hosts),
            Self::Clipboard => "Read and change your clipboard".into(),
            Self::Storage => "Keep notes of its own between sessions".into(),
            Self::ReadFiles(paths) => list_sentence("Read ", paths),
            Self::ListDirectories(roots) => list_sentence("See the names of the files in ", roots),
            Self::TypeCommands(templates) => {
                let filled: Vec<String> = templates
                    .iter()
                    .map(|template| template.replace("{}", "\u{2026}"))
                    .collect();
                list_sentence("Type into your shell, and run: ", &filled)
            }
            Self::RunCommands(names) => list_sentence("Use Crook's own ", names),
            Self::ReadCommands => "See what Crook can be asked to do, and the keys for it".into(),
        }
    }

    /// What granting this is written down as, one string per thing granted.
    ///
    /// A grant is kept as text rather than as this enum, and that is the whole
    /// mechanism behind "re-prompted on escalation": a plugin that adds a host
    /// to its [`Network`] list in its next version asks for a key that is not
    /// in what a person allowed, so it is not granted and the Plugins page can
    /// say which line is new. Comparing the enums instead would make any
    /// change to the list a change to one value, and the only honest answer
    /// then would be to ask about all of it again.
    ///
    /// One key per *host* and per *path* for the same reason: allowing
    /// `api.anthropic.com` should not become allowing whatever a later version
    /// adds beside it.
    ///
    /// [`Network`]: Self::Network
    pub fn keys(&self) -> Vec<String> {
        match self {
            Self::ReadSettings => vec![String::from("settings.read")],
            Self::ReadTabs => vec![String::from("tabs.read")],
            Self::ReadWorkingDirectory => vec![String::from("cwd.read")],
            Self::Network(hosts) => hosts.iter().map(|host| format!("net:{host}")).collect(),
            Self::Clipboard => vec![String::from("clipboard")],
            Self::Storage => vec![String::from("storage")],
            Self::ReadFiles(paths) => paths.iter().map(|path| format!("file:{path}")).collect(),
            Self::ListDirectories(roots) => {
                roots.iter().map(|root| format!("list:{root}")).collect()
            }
            Self::TypeCommands(templates) => templates
                .iter()
                .map(|template| format!("type:{template}"))
                .collect(),
            Self::RunCommands(names) => names.iter().map(|name| format!("run:{name}")).collect(),
            Self::ReadCommands => vec![String::from("commands.read")],
        }
    }
}

/// "Read a, b and c", built from a lead-in and the things granted.
///
/// One function rather than three copies of the same loop, and it is here
/// rather than inline because every capability that is a *list* has to say the
/// same shape of sentence: a person comparing what two plugins ask for is
/// comparing two sentences, and two that are worded differently read as two
/// different kinds of request.
fn list_sentence(lead: &str, items: &[String]) -> String {
    let mut sentence = String::from(lead);
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            sentence.push_str(", ");
        }
        sentence.push_str(item);
    }
    sentence
}

/// How much a piece of text matters, rather than what colour it is.
///
/// The host resolves each of these against the theme in force, so a plugin
/// written before a theme existed is drawn correctly in it. A plugin that
/// could name a colour would be a plugin that looks wrong in half of them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    /// The ordinary weight of text on this surface.
    #[default]
    Primary,
    /// Secondary: a subtitle, a unit, a hint.
    Muted,
    /// The one thing on the surface that is being pointed at.
    Accent,
    /// Something is not right but nothing has failed.
    Warning,
    /// Something failed.
    Danger,
    /// Something worked.
    Success,
}

/// How big a piece of text is, relative to the interface.
///
/// Three sizes and no numbers, for the reason there are no colours: a plugin
/// that named 11.5 pixels would be a plugin that is the wrong size on a
/// display it was not written for.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Size {
    /// Smaller than the interface's default: a unit, a count, a caption.
    Small,
    /// The interface's default.
    #[default]
    Body,
    /// A heading.
    Large,
}

/// Something to draw, described rather than painted.
///
/// Deliberately small. Every variant here is something Crook's own chrome
/// already draws, which is the test a variant has to pass: the vocabulary
/// describes the interface Crook *has*, so that a plugin using it looks like
/// part of the application rather than like something dropped into it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// Nothing at all. What a contribution returns when it has nothing to say
    /// — which is most frames, for most plugins.
    Empty,
    /// A run of text.
    Text {
        /// What it says.
        text: String,
        /// How big.
        size: Size,
        /// How much it matters.
        tone: Tone,
    },
    /// Text inside a rounded pill, the way the usage chip is drawn.
    Badge {
        /// What it says.
        text: String,
        /// Which of the theme's tones the pill takes.
        tone: Tone,
    },
    /// One of Crook's icons, by the name in the Lucide set.
    ///
    /// By name rather than by drawing, because a plugin that shipped its own
    /// vector art would be a plugin whose icons are the wrong weight beside
    /// everything else. A name this build has no icon for draws nothing.
    Icon {
        /// The Lucide name, in kebab-case: `git-branch`, `circle-alert`.
        name: String,
        /// Which of the theme's tones it takes.
        tone: Tone,
    },
    /// Children left to right.
    Row(Vec<Node>),
    /// Children top to bottom.
    Column(Vec<Node>),
    /// A fixed gap, in the interface's own units rather than in pixels.
    Gap(Gap),
    /// Something to press, which runs one of the plugin's named actions.
    Button {
        /// What it says.
        label: String,
        /// The action to run, without the plugin's own prefix: the host puts
        /// that on, so a plugin cannot name somebody else's action.
        action: String,
        /// Which of the theme's tones it takes.
        tone: Tone,
    },
    /// A bar with part of it filled: how much of a limit is gone, how much of
    /// a whole something is.
    ///
    /// A *fraction*, not a width. The host decides how long a bar is and how
    /// thick it is drawn, so a plugin cannot produce one that is the wrong
    /// size in a window it never saw — the same reason there are three text
    /// sizes and no numbers.
    Meter {
        /// Between zero and one; anything outside is clamped by the host
        /// rather than refused, because a reading that briefly exceeds its own
        /// limit is a thing that happens and is not worth an empty frame.
        fraction: f32,
        /// Which of the theme's tones the filled part takes.
        tone: Tone,
    },
    /// A hairline across whatever holds it: the honest place to put the seam
    /// between two things that are not the same measurement.
    Rule,
    /// Space that takes whatever is left over.
    ///
    /// What puts a figure at the far end of a row from its label, which is the
    /// commonest shape in a panel and the one thing [`Gap`] cannot do: a gap
    /// is a number of pixels and a row's width is not known to the plugin.
    Fill,
    /// Prose, which wraps.
    ///
    /// Separate from [`Text`] because wrapping is the difference: a label that
    /// wraps is a label that was too long, and a note that does not is a note
    /// with its end cut off.
    ///
    /// [`Text`]: Self::Text
    Note {
        /// What it says.
        text: String,
        /// How much it matters.
        tone: Tone,
    },
    /// Anything at all, made to answer a click.
    ///
    /// [`Button`] is a control that looks like one; this is the other half of
    /// pressing — a chip, a row, a mark — for the times what should be clicked
    /// is the thing itself rather than a labelled control beside it.
    ///
    /// [`Button`]: Self::Button
    Pressable {
        /// What is drawn.
        content: Box<Node>,
        /// The action a click runs, without the plugin's own prefix.
        action: String,
    },
    /// Something with a panel hung under it.
    ///
    /// The one shape here that is not a box in a row, and it earns that: a
    /// plugin whose whole surface is a chip in the header has nowhere to say
    /// the rest of what it knows, and a plugin that could open a window would
    /// be a plugin that can cover the terminal. So the panel is *anchored to
    /// the contribution* — the host places it, sizes it, gives it its ground
    /// and its corner, and takes it away again when a click lands outside.
    ///
    /// Whether it is up is the plugin's state, not the host's: `panel` is
    /// `None` on every frame it is shut. Dismissing runs `dismiss`, which is
    /// how the plugin finds out that a click somewhere else closed it.
    Anchored {
        /// What sits in the slot.
        content: Box<Node>,
        /// What hangs under it, when anything does.
        panel: Option<Box<Node>>,
        /// The action a dismissal runs, without the plugin's own prefix.
        dismiss: String,
    },
    /// A list to choose from, with a field over it.
    ///
    /// The one node here the *host* drives rather than draws. A plugin
    /// supplies the rows and is told which one was chosen; the field, the
    /// filtering, the arrow keys, Enter, Escape, the hover and the scroll all
    /// belong to Crook. That is not a convenience — it is the only way a
    /// sandboxed plugin can have a search box at all. A plugin given the
    /// keyboard would be a plugin that reads what is typed in the window it
    /// is a chip in, and a plugin that filtered its own list would cost a
    /// call into the sandbox on every keystroke, on the thread that draws.
    ///
    /// So the plugin says what can be chosen and the host says what a person
    /// chose, which is the same bargain as [`Meter`]: describe the reading,
    /// not the pixels.
    ///
    /// **What is typed is never handed over.** The rows are what the plugin
    /// offered and the filtering is the host's, so a picker cannot be a way of
    /// reading what somebody is typing in the window it is a chip in. A picker
    /// whose rows change with the query — a search somebody else answers — is
    /// a thing this cannot describe, and it stays that way until something
    /// needs it: a variant nobody uses is a variant that has to keep working
    /// for ever.
    ///
    /// [`Meter`]: Self::Meter
    Picker {
        /// What the field says while nothing has been typed.
        placeholder: String,
        /// Everything that can be chosen, in the order it is offered. The host
        /// keeps that order and shows the ones matching what has been typed.
        rows: Vec<Row>,
        /// The action a chosen row runs, without the plugin's own prefix. The
        /// row's own [`key`](Row::key) arrives as the action's argument.
        choose: String,
    },
    /// A mark and a word in a quiet pill: what a chip beside a prompt is.
    ///
    /// Not a [`Badge`], which is a *reading* — a filled pill in a tone, for a
    /// percentage or a count, and loud on purpose. This is the other one: the
    /// theme's own raised ground and a hairline, for a fact about where you
    /// are. Warp draws both and so does Crook, and a plugin that had to build
    /// one out of a row and a container would be a plugin drawing a pill the
    /// wrong shape beside the application's own.
    ///
    /// [`Badge`]: Self::Badge
    Chip {
        /// A Lucide name in front of the text, or empty for none.
        icon: String,
        /// What it says.
        text: String,
        /// Which of the theme's tones the text and the mark take. The ground
        /// is the theme's, not the tone's: a chip is not a reading.
        tone: Tone,
    },
    /// Something with a menu on its secondary click.
    ///
    /// The gesture every desktop opens a context menu with, and the host draws
    /// the menu: a plugin that had to draw one would be a plugin whose menu is
    /// the wrong shape beside the one a tab opens.
    Menu {
        /// What is drawn, and what the secondary click lands on.
        content: Box<Node>,
        /// What the menu offers, in order.
        items: Vec<MenuItem>,
    },
}

/// One thing a [`Node::Picker`] offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// What the plugin is told when this row is chosen. Never shown.
    pub key: String,
    /// What the row says, and what the host filters on.
    pub label: String,
    /// A Lucide name drawn in front of the label, as [`Node::Icon`] resolves
    /// one; empty for a row with nothing in front of it.
    pub icon: String,
    /// Which of the theme's tones the label takes.
    pub tone: Tone,
}

/// One entry of a [`Node::Menu`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    /// What it says.
    pub label: String,
    /// The action it runs, without the plugin's own prefix.
    pub action: String,
    /// What that action is handed, or empty for an action with nothing to say
    /// to it. Here rather than in the name so that one action can serve every
    /// entry of a menu built out of something the plugin was told.
    pub argument: String,
}

/// Something a plugin asks the host to do on its behalf.
///
/// A sandboxed plugin has no network, no filesystem and no clock of its own —
/// that is what makes it sandboxed. What it has instead is this: it *asks*,
/// the host decides whether what it asked for is inside what a person granted,
/// and the work happens on the host's side of the boundary where it can be
/// refused, timed out and logged.
///
/// Asking never blocks. The call that raises a request gets an integer ticket
/// back and returns; the answer arrives later at `crook_deliver`, carrying the
/// same ticket. That is not a convenience — a guest call runs on the thread
/// that draws, so a request that waited for a socket would be a request that
/// cost a frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// Reach the network. Needs [`Capability::Network`] naming the host in the
    /// URL; anything else comes back [`Answer::Refused`].
    Fetch {
        /// Which verb.
        method: Method,
        /// The whole URL, scheme and all. Only `https` is performed.
        url: String,
        /// Headers to send, in order.
        headers: Vec<(String, String)>,
        /// The body, for the verbs that carry one.
        body: Option<Vec<u8>>,
    },
    /// Read a file. Needs [`Capability::ReadFiles`] naming exactly this path.
    ///
    /// A leading `~` is the person's home directory. There is no listing and
    /// no writing: a plugin reads the files it said it would read, and a
    /// capability that could name a directory would be one nobody could
    /// picture the contents of.
    ReadFile {
        /// The path, as it was written in the capability.
        path: String,
    },
    /// Where the active pane is, and what git says about it. Needs
    /// [`Capability::ReadWorkingDirectory`].
    Where,
    /// The names in one directory. Needs [`Capability::ListDirectories`]
    /// naming a root this path is inside.
    ///
    /// Names and kinds, never contents, and one directory rather than a walk:
    /// a plugin that could ask for a tree is a plugin that can be handed a
    /// hundred thousand names for asking once.
    List {
        /// The directory to read. A leading `~` is the person's home.
        path: String,
    },
    /// What the repository a directory is in has: its head, and its branches.
    /// Needs [`Capability::ReadWorkingDirectory`].
    ///
    /// Answered by the host's own git rather than by handing a plugin the
    /// repository to read for itself, which would be a grant worded "read
    /// everything you have ever written".
    Repository {
        /// A directory inside the repository, or the repository itself.
        path: String,
    },
    /// Type a command into the active pane's shell, and run it. Needs
    /// [`Capability::TypeCommands`] naming exactly this template.
    ///
    /// The template is the granted string, with one `{}` where the argument
    /// goes; the host fills it and quotes what it fills it with, so a branch
    /// called `;rm -rf ~` is a branch name and not a second command. What the
    /// shell then does with the line is the shell's own business, which is the
    /// point: `cd` is the shell's, and so are its aliases and its hooks.
    ///
    /// Only from an action a person ran. A request raised while describing or
    /// on a timer of the plugin's own is refused, because a plugin that can
    /// type without being clicked is a plugin that types while nobody is
    /// looking.
    Type {
        /// The template, exactly as it was granted.
        template: String,
        /// What goes in the hole.
        argument: String,
    },
    /// Run one of Crook's own commands. Needs [`Capability::RunCommands`]
    /// naming exactly this one.
    Run {
        /// The command's full name, as the Keyboard Shortcuts page prints it.
        name: String,
        /// What it is handed, or empty for a command that takes nothing.
        argument: String,
    },
    /// Everything Crook can be asked to do, and the chord that reaches each.
    /// Needs [`Capability::ReadCommands`].
    Commands,
}

/// Which HTTP verb a [`Request::Fetch`] is.
///
/// Two, because two is what a plugin that reads something needs. A verb that
/// changes somebody else's state is not something this tier should be able to
/// reach for without a capability of its own, and there is no such capability
/// yet.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Ask for something.
    Get,
    /// Send something and be told what came back.
    Post,
}

/// What became of a [`Request`].
///
/// Four answers and not one of them is silence: a plugin that asked for
/// something always finds out what happened to it, because a plugin left
/// waiting forever is a chip that says "reading…" until the window closes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    /// The request was made and the server answered.
    ///
    /// A status the plugin has to read for itself: a 401 is an answer, not a
    /// failure, and the host has no idea which of them this plugin considers
    /// one.
    Fetched {
        /// What the server said it was.
        status: u16,
        /// What it sent.
        body: Vec<u8>,
    },
    /// The file was read.
    Read {
        /// What was in it.
        bytes: Vec<u8>,
    },
    /// It was not granted. The sentence says what was asked for, in the same
    /// words the permission dialog used, so a plugin can tell a person what to
    /// allow rather than saying "something went wrong".
    Refused(String),
    /// It was granted and attempted, and did not work.
    Failed(String),
    /// Where the active pane is.
    Where(Facts),
    /// What is in a directory, in the order the host sorted it: directories
    /// first, then files, each by name.
    Listed(Vec<Entry>),
    /// What a repository has.
    Repository {
        /// The branch its head is on, or the short sha of a detached head, or
        /// `None` for a path that is not in a repository at all.
        head: Option<String>,
        /// Every branch it has, in the order git lists them.
        branches: Vec<String>,
    },
    /// What Crook can be asked to do.
    Commands(Vec<Command>),
    /// It was granted, attempted and did what it said.
    ///
    /// What a request that *changes* something answers with: there is nothing
    /// to hand back, and "nothing came back" and "it worked" have to be
    /// different answers or a plugin cannot tell them apart.
    Done,
}

/// Where a pane is, and what git says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// The working directory the shell last reported, or `None` for a pane
    /// that has not said.
    pub directory: Option<String>,
    /// The branch it is on, or the short sha of a detached head.
    pub branch: Option<String>,
    /// The person's home directory.
    ///
    /// Here because a chip prints `~/Work/crook` and a plugin cannot know
    /// which prefix that is — and because the alternative, handing over a
    /// path already shortened, would be handing over a path that cannot be
    /// `cd`-ed to. It reveals nothing the directory above it does not.
    pub home: Option<String>,
    /// Lines added in the working tree against `HEAD`.
    pub added: u32,
    /// Lines removed.
    pub removed: u32,
}

/// One name in a directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name alone, with no path in front of it.
    pub name: String,
    /// Whether it is a directory.
    pub directory: bool,
}

/// One thing Crook can be asked to do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// Its full name, which is what a keybindings file writes.
    pub name: String,
    /// What a person calls it.
    pub title: String,
    /// The chord that reaches it, as the Keyboard Shortcuts page prints it, or
    /// `None` for a command nothing is bound to.
    pub chord: Option<String>,
}

/// A gap, in units rather than pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gap {
    /// The gap between two words.
    Small,
    /// The gap between two controls.
    #[default]
    Medium,
    /// The gap between two groups.
    Large,
}

/// Everything a plugin registered while it built.
///
/// Collected by the host as the plugin calls the registration imports, and
/// handed back as one value — so that a plugin that traps halfway through
/// registers nothing rather than half of itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Registered {
    /// The slots it contributed to, with the entry name and the order.
    pub contributions: Vec<Contribution>,
    /// The actions it offers, by name and title.
    pub actions: Vec<Action>,
}

/// One contribution to one slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    /// The slot's name, checked by the host against the slots that exist.
    pub slot: String,
    /// This contribution's own name, unique within the plugin.
    pub entry: String,
    /// Where it goes among the others; lower is earlier.
    pub order: i32,
}

/// One named action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// The action's name within the plugin: the host puts `owner/name/` on the
    /// front, so a plugin cannot claim somebody else's.
    pub name: String,
    /// What a palette calls it, or `None` for an action that is reachable but
    /// not offered.
    pub title: Option<String>,
}

/// Encodes a value for the wire.
pub fn to_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(value)
}

/// Decodes one.
pub fn from_bytes<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
