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
/// **7** is the version a plugin can act on **one command** in.
/// [`Subject::Block`], which is what a render of a slot in a block's menu is
/// about — the command line, how it ended and where it ran, each redacted
/// against what a person granted, exactly as a tab row's facts are.
/// [`Capability::ReadBlock`] is what un-redacts it, and
/// [`Request::Output`] is the half a subject cannot afford to carry: what the
/// command *printed* can be a megabyte, and a subject is built on every frame
/// a menu is open, so the output is asked for once when an entry runs rather
/// than copied sixty times a second at somebody who may never press anything.
/// [`Request::Copy`] is the other addition, and the shortest way for a plugin
/// to hand a person what it worked out.
///
/// **6** gave a plugin a field the keyboard can be in, and a menu on a
/// secondary click.
///
/// **5** is the version a plugin can be *noticed* in.
/// [`Capability::PlaySound`] and [`Capability::WatchCommands`], which together
/// are a plugin that can ring when a command finishes; the
/// [`Request::PlaySound`] that carries the audio itself rather than a path;
/// [`Event`], the first thing the host says to a plugin that the plugin did
/// not ask for; and [`Node::Explained`], which lets one hang what it would
/// otherwise have to say permanently off the thing it is about.
///
/// **4** is the version a plugin can be asked the same question twice in. A
/// render used to carry a slot name and nothing else, which is enough for a
/// slot there is one of — the header has one right-hand end — and nothing at
/// all for a slot that is drawn once per row of a list. So a render carries a
/// [`Render`]: the slot, and the [`Subject`] it is about when the slot has
/// one. What a subject *says* is [`redacted`](TabFacts) against what a person
/// granted, which is why a plugin that marks every tab with a picture of its
/// own can be a plugin allowed to know nothing about any of them.
///
/// **3** added [`Request::Tally`] and the `/**` a granted path may end in: a
/// plugin can have a directory of line-delimited JSON counted for it without
/// any of it crossing the boundary. A hundred megabytes of transcripts is a
/// hundred megabytes wherever it is read; what a sandbox cannot afford is
/// carrying it *through*, or looking at it a line at a time. So it does
/// neither, and every decision about what the counting *means* — which lines
/// are one line, what to group by, what to add up — stays with the plugin,
/// which supplies each of them as a field name.
///
/// **2** is the version a plugin can *do* something in. One added
/// [`Capability`] ([`Capability::ReadFiles`]), the [`Request`]/[`Answer`] pair
/// that lets a plugin ask the host to reach the network or read a file on its
/// behalf, and the six [`Node`] variants a panel needs. Version 1 could
/// describe a badge and register an action, which is a plugin that can say
/// what it already knew.
pub const ABI_VERSION: u32 = 7;

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
    /// which is active, and what each agent is doing. Not what is *in* a pane.
    ReadTabs,
    /// Read the working directory and git facts of a pane — the active one,
    /// and each of the rows a plugin is asked to draw a mark on.
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
    ///
    /// A path may end in `/**`, which is everything under a directory. That is
    /// a weaker thing to agree to than a named file and the sentence says so,
    /// but it is the only shape in which "the transcripts Claude Code writes"
    /// can be asked for at all: they are a directory of files whose names
    /// nobody knows in advance.
    ReadFiles(Vec<String>),
    /// Make a sound, by handing the host the audio to play.
    ///
    /// The guest never reaches an audio device: it sends bytes with
    /// [`Request::PlaySound`] and the host plays them, the same shape every
    /// other request has. What is being granted is *interrupting a person* —
    /// which is why it is a capability at all, and why the sentence says
    /// "sound" rather than naming a device nobody has an opinion about.
    PlaySound,
    /// Be told when a command in a pane finishes.
    ///
    /// Not a sight of what ran: the event carries which pane and how it
    /// exited, and no command line. Even so it is a capability, because when
    /// somebody's commands finish and whether they failed is a picture of how
    /// their day is going, and a plugin that could watch that unasked would be
    /// one nobody had the chance to refuse.
    WatchCommands,
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
    /// Read the command a menu of this plugin's is open on: what was run, how
    /// it ended, and what it printed.
    ///
    /// One command at a time, and only ever one a person pointed at: a plugin
    /// with this sees the block whose menu they opened, for as long as it is
    /// open, and has no way to name another or to ask about a session. What it
    /// does *not* cover is where that command ran — a directory is a directory
    /// wherever it is read, so that stays [`Capability::ReadWorkingDirectory`]
    /// and a plugin that wants both asks for both.
    ReadBlock,
}

impl Capability {
    /// The sentence a permission dialog says.
    pub fn sentence(&self) -> String {
        match self {
            Self::ReadSettings => "Read your settings".into(),
            Self::ReadTabs => "See what your tabs are called".into(),
            Self::ReadWorkingDirectory => "See which project each tab is in".into(),
            Self::Network(hosts) => {
                let mut sentence = String::from("Reach ");
                for (index, host) in hosts.iter().enumerate() {
                    if index > 0 {
                        sentence.push_str(", ");
                    }
                    sentence.push_str(host);
                }
                sentence
            }
            Self::Clipboard => "Read and change your clipboard".into(),
            Self::Storage => "Keep notes of its own between sessions".into(),
            Self::ReadFiles(paths) => {
                let mut sentence = String::from("Read ");
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        sentence.push_str(", ");
                    }
                    match path.strip_suffix("/**") {
                        // Said as what it is rather than as the pattern that
                        // spells it: a person reading a permission dialog
                        // should not have to know what two stars mean.
                        Some(directory) => {
                            sentence.push_str("everything under ");
                            sentence.push_str(directory);
                        }
                        None => sentence.push_str(path),
                    }
                }
                sentence
            }
            Self::PlaySound => "Play a sound".into(),
            Self::WatchCommands => "Know when a command finishes".into(),
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
            Self::ReadBlock => "Read the command you run it on, and what it printed".into(),
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
            Self::PlaySound => vec![String::from("sound.play")],
            Self::WatchCommands => vec![String::from("commands.watch")],
            Self::ListDirectories(roots) => {
                roots.iter().map(|root| format!("list:{root}")).collect()
            }
            Self::TypeCommands(templates) => templates
                .iter()
                .map(|template| format!("type:{template}"))
                .collect(),
            Self::RunCommands(names) => names.iter().map(|name| format!("run:{name}")).collect(),
            Self::ReadCommands => vec![String::from("commands.read")],
            Self::ReadBlock => vec![String::from("block.read")],
        }
    }
}

/// What the host is asking for, and what it is asking about.
///
/// A render used to be a slot name, which is enough for a slot there is one
/// of: the header has one right-hand end, and a plugin asked what goes in it
/// knows everything it needs from the question. It is nothing at all for a
/// slot drawn once per row of a list — a plugin asked "what is this tab's
/// mark" with no way to know *whose* mark can only answer the same thing for
/// every tab there is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Render {
    /// The slot, by the name the plugin contributed to.
    pub slot: String,
    /// This contribution's own name, as the plugin spelled it in `contribute`.
    ///
    /// A plugin may put four things in one list slot — a row of chips is
    /// exactly that — and a render told only which slot it was in would have
    /// to draw all four in each of them.
    pub entry: String,
    /// What this render is about, for a slot that is drawn per thing, and
    /// `None` for a slot that is drawn once.
    pub subject: Option<Subject>,
}

/// What one render is about.
///
/// A plugin that matches on a subject this build does not have draws nothing
/// rather than guessing, which is the rule an unknown slot name already
/// follows. The one still missing is a slot per *pane*.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subject {
    /// One row of the tab panel.
    Tab(TabFacts),
    /// The command whose menu this is being drawn in.
    Block(BlockFacts),
}

/// One command, as much of it as this plugin was allowed to see.
///
/// **Redacted rather than refused**, the way [`TabFacts`] is: a plugin granted
/// nothing still gets a [`key`](Self::key) and is still drawn, because an
/// entry that says "send this somewhere" is a thing somebody can want without
/// the plugin being told what they ran.
///
/// What it does *not* carry is the output. That is [`Request::Output`], and
/// the split is a cost rather than a policy: this is built on every frame a
/// menu is open, and what a command printed can be a megabyte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockFacts {
    /// Which command this is, as a number that says nothing else.
    ///
    /// Two renders of one block carry the same key and two blocks never carry
    /// one, which is what lets a plugin notice that the menu it is drawing in
    /// is the one it was drawing in a frame ago. Salted with the asking
    /// plugin's own id, as [`TabFacts::key`] is, and it says nothing about the
    /// command: a block's key is handed out fresh per session and means
    /// nothing tomorrow, because a command is not a place.
    pub key: u64,
    /// What was run and how it ended, or `None` without
    /// [`Capability::ReadBlock`].
    pub ran: Option<Ran>,
    /// Where it ran, or `None` without [`Capability::ReadWorkingDirectory`] —
    /// and `None` as well for a shell that never said.
    pub place: Option<Place>,
}

/// What a command was and what became of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    /// The command line, as it was submitted or as the shell echoed it, or
    /// `None` when neither happened.
    pub command: Option<String>,
    /// The status the shell reported, or `None` when it reported none.
    pub exit: Option<i32>,
}

/// One row of the tab panel, as much of it as this plugin was allowed to see.
///
/// **Redacted rather than refused.** A plugin granted nothing still gets a
/// [`key`](Self::key) and is still drawn, because a mark per tab is a thing
/// somebody can want without wanting to know what the tabs are. Everything
/// past that is a capability: what the tab is called is
/// [`Capability::ReadTabs`], where it is working is
/// [`Capability::ReadWorkingDirectory`], and a plugin that was not granted one
/// finds `None` where the answer would have been rather than a refusal it has
/// to handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabFacts {
    /// Which row this is, as a number that says nothing else.
    ///
    /// Two renders of one tab carry the same key, two tabs never carry one,
    /// and a project opened again tomorrow carries the key it carried today —
    /// which is the whole of what it takes to give a tab a mark of its own and
    /// have it still be that tab's mark next week. It is a hash of where the
    /// tab is working, salted with the asking plugin's own id, so two plugins
    /// cannot compare notes about which of their rows are the same row.
    ///
    /// Two tabs *in one directory* are told apart by which of them it is,
    /// counted down the panel, because a directory alone could not do it: a
    /// tab opened from the window starts where Crook started, so a hash of the
    /// place alone gave a whole window of new tabs one key. The consequence a
    /// plugin can see is that closing the first of several rows in a directory
    /// moves the keys of the ones under it — they each became the row above —
    /// while a restored session, which remembers its panes in order, brings
    /// every one of them back with the key it had.
    ///
    /// It is not a secret and is not offered as one: a hash can be checked
    /// against a guess, so a plugin that already knew a path could find out
    /// whether a tab is in it. That is exactly why the key is *all* that is
    /// ungated — everything a person would actually mind being read is a
    /// capability below, and none of it can be recovered from this number.
    pub key: u64,
    /// What the tab is, or `None` without [`Capability::ReadTabs`].
    pub tab: Option<TabInfo>,
    /// Where it is working, or `None` without
    /// [`Capability::ReadWorkingDirectory`].
    pub place: Option<Place>,
}

/// What a tab is, for a plugin granted [`Capability::ReadTabs`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    /// What the row's first line says.
    pub title: String,
    /// Whether this is the tab being looked at.
    pub active: bool,
    /// What the agent in it is doing.
    pub status: Status,
}

/// Where a tab is working, for a plugin granted
/// [`Capability::ReadWorkingDirectory`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The working directory, whole, as the host knows it.
    pub directory: String,
    /// The branch, or the short sha of a detached head, when the directory is
    /// in a repository at all.
    pub branch: Option<String>,
    /// Whether it is a linked git worktree rather than the checkout the
    /// repository was cloned into.
    ///
    /// A fact about the directory rather than about how the tab was opened: a
    /// worktree somebody made at a shell years ago is one, and so is the one
    /// Crook opened beside a tab this morning. There is no way to ask which,
    /// and no plugin should have to care.
    pub worktree: bool,
}

/// What the agent in a tab is doing.
///
/// The same four the panel draws as four colours of disc, said as words
/// because a plugin may not name a colour.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Waiting for a prompt.
    #[default]
    Idle,
    /// Working, with no attention needed.
    Running,
    /// Stopped, waiting on a person.
    NeedsInput,
    /// Stopped because something went wrong.
    Failed,
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
    /// A row of columns, each as tall as its share of the tallest.
    ///
    /// Shares of one another rather than of anything absolute, which is what a
    /// chart of "what happened on each of seven days" is: nobody reads the
    /// height, they read which day was the busy one. The host decides how tall
    /// the tallest is drawn, how wide a column is and what a column of nothing
    /// looks like — a day with no work still has to be a column of no height
    /// rather than a gap, or the chart says the week was shorter than it was.
    Bars {
        /// Between zero and one each; the host clamps rather than refuses.
        values: Vec<f32>,
        /// Which of the theme's tones they take.
        tone: Tone,
    },
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
    /// Something with a note that appears while the pointer is on it.
    ///
    /// The other half of [`Anchored`], and the difference is state. A panel is
    /// up because the plugin says so and shut because a click somewhere else
    /// told it; a note is up exactly while the pointer is on the thing, which
    /// is not a fact anybody has to remember. So this names no action in
    /// either direction: the host never asks whether to show it and never says
    /// that it did, and a plugin that has stopped answering cannot leave one
    /// on screen.
    ///
    /// Which is also why the note is described on every frame rather than
    /// fetched on the one the pointer arrives. A note whose words arrived only
    /// then would be a call into the guest at pointer speed, on the thread
    /// that draws, to say something the plugin already knew.
    ///
    /// [`Anchored`]: Self::Anchored
    Explained {
        /// What is drawn.
        content: Box<Node>,
        /// What appears above it while the pointer is on it.
        explanation: Box<Node>,
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
    /// A leading `~` is the person's home directory. There is no writing: a
    /// plugin reads the files it said it would read.
    ReadFile {
        /// The path, as it was written in the capability.
        path: String,
    },
    /// Walk a directory of line-delimited JSON and hand back what it adds up
    /// to. Needs [`Capability::ReadFiles`] granting a path this root is under.
    ///
    /// **The host reads and counts; the plugin decides what counting means.**
    /// A directory like the transcripts Claude Code writes is hundreds of
    /// megabytes across tens of thousands of lines, and the first shape this
    /// took — hand the plugin the fields and let it add them up — was measured
    /// at ninety thousand instructions a line, which for a week is forty
    /// seconds of interpreter on the thread that draws. No page size makes
    /// that affordable, because the cost is per line and there are too many
    /// lines.
    ///
    /// So the lines are counted where they are read, and what crosses is the
    /// answer: a few hundred rows instead of forty thousand. Every decision
    /// stays with the plugin — which lines are the same event written twice,
    /// what to group by, what to add up, and what any of it means — because
    /// each of those is a *field name* it supplies. The host knows none of it.
    Tally {
        /// The directory to walk, which must be inside a granted path.
        root: String,
        /// Only files whose name ends in this.
        extension: String,
        /// Only files modified at or after this, in milliseconds since the
        /// epoch — because a file nobody has touched cannot hold a line inside
        /// a window that ends now. Zero reads them all.
        touched_since: i64,
        /// Only lines holding this. A line without it is never parsed, which
        /// is what makes walking a hundred megabytes cheap.
        containing: String,
        /// Keep only lines whose field sorts at or after the value beside it.
        ///
        /// Compared as text, which is the whole of what the host knows about
        /// a value. That is enough for the thing it is for: an RFC 3339 stamp
        /// sorts the way time runs, so "at or after this instant" is a string
        /// comparison and the host needs no idea what a date is.
        ///
        /// Necessary rather than convenient. `touched_since` skips *files*,
        /// which is what makes the walk cheap, but a file touched an hour ago
        /// can hold lines from a month ago — so without this a total would
        /// quietly include them.
        at_least: Vec<Bound>,
        /// Lines agreeing on all of these are one line, counted once.
        ///
        /// Empty counts every line. A line missing any of them is counted
        /// rather than dropped: an identity that is not there cannot say a
        /// line is a repeat of anything.
        distinct_by: Vec<String>,
        /// The tables to build, each a way of looking at the same walk.
        ///
        /// Several rather than one because the walk is the expensive part: a
        /// plugin wanting totals by day *and* by author would otherwise pay
        /// for the hundred megabytes twice.
        tables: Vec<Table>,
    },
    /// Play a sound. Needs [`Capability::PlaySound`].
    ///
    /// The audio travels rather than a path, because a plugin's own chime is
    /// something it ships and not something on the machine: sending bytes
    /// costs it no [`Capability::ReadFiles`] over its own directory, and the
    /// host never resolves a name a guest chose into a file it then opens.
    ///
    /// One format, `wav`: the host does not decode audio and has no library
    /// that could, so what it can hand a player is what every player on every
    /// platform already reads. Anything else is [`Answer::Failed`].
    PlaySound {
        /// A RIFF/WAVE file, whole, as it would sit on disk.
        wav: Vec<u8>,
        /// How loud, 0 to 100. Clamped, and best effort: some players cannot
        /// be told, and the host would rather play at their volume than not
        /// play.
        volume: u8,
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
    /// What the command printed, for the block a menu of this plugin's is open
    /// on. Needs [`Capability::ReadBlock`].
    ///
    /// The half [`Subject::Block`] cannot afford to carry: a subject is built
    /// on every frame a menu is up, and output can be a megabyte. So it is
    /// asked for — once, by an entry somebody pressed, which is the only
    /// moment a plugin has any business with it.
    ///
    /// Only from an action a person ran, for the reason [`Request::Type`] is:
    /// a request raised while describing, or on a timer of the plugin's own,
    /// is about a block nobody pointed it at and comes back [`Answer::Failed`].
    Output,
    /// Put text on the system clipboard. Needs [`Capability::Clipboard`].
    ///
    /// A request rather than something a [`Node`] could describe, because what
    /// goes on a clipboard is a thing that *happens*, at a moment, because
    /// somebody asked — not a thing that is drawn.
    Copy {
        /// What to put there.
        text: String,
    },
}

/// A floor under one field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// The field, by dotted path.
    pub field: String,
    /// The value it must sort at or after. A line whose field is missing does
    /// not clear it.
    pub at_least: String,
}

/// One way of adding a walk up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// What makes a row: lines agreeing on all of these share one.
    pub by: Vec<Key>,
    /// The fields to add up, by dotted path. A line where one is missing or is
    /// not a number contributes nothing to that sum and still counts as a row.
    pub sum: Vec<String>,
}

/// One column of a table's key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    /// The field, by dotted path.
    pub field: String,
    /// How much of it to key on, in characters from the start.
    ///
    /// `None` is the whole value. A prefix is what makes "by the hour" or "by
    /// the day" expressible without the host knowing what a date is: a
    /// timestamp's first thirteen characters are its hour, and which hour
    /// belongs to which day is a question about time zones that only the
    /// plugin can answer.
    pub prefix: Option<u32>,
}

/// One row of a table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tallied {
    /// The values that make this row, in the order they were asked for.
    pub key: Vec<Cell>,
    /// What each of the summed fields came to, in the order they were asked
    /// for.
    pub sums: Vec<f64>,
    /// How many lines it is made of.
    pub lines: u64,
}

/// One field of one line, as it was found.
///
/// Three shapes and not one string, because a plugin that had to parse
/// `"1284"` back into a number would be paying twice for something the host
/// already knew — and a field a line does not carry has to be distinguishable
/// from one that carries an empty string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cell {
    /// The line does not carry this field.
    Nothing,
    /// A string, a boolean or anything else, as it reads.
    Text(String),
    /// A number.
    Number(f64),
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
/// Five answers and not one of them is silence: a plugin that asked for
/// something always finds out what happened to it, because a plugin left
/// waiting forever is a chip that says "reading…" until the window closes.
///
/// Not [`Eq`], because a [`Cell`] can be a number and a number is not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// What a [`Request::Tally`] came to.
    Counted {
        /// One per table asked for, in the order they were asked for. Each is
        /// a row per distinct key, in no order worth relying on.
        tables: Vec<Vec<Tallied>>,
        /// How many lines were counted altogether, after `distinct_by` has had
        /// its say.
        lines: u64,
    },
    /// It was not granted. The sentence says what was asked for, in the same
    /// words the permission dialog used, so a plugin can tell a person what to
    /// allow rather than saying "something went wrong".
    Refused(String),
    /// It was granted and attempted, and did not work.
    Failed(String),
    /// The sound was handed to a player, which is as far as this goes.
    ///
    /// Not "the person heard it": the host spawns a player and does not wait
    /// for it, so what it can honestly report is that something took the
    /// audio. A plugin that rings twice because it disbelieved this answer
    /// would be worse than one that trusts it.
    Played,
    /// Where the active pane is.
    ///
    /// `place` is `None` for a pane whose shell has not said where it is,
    /// which is the state every pane is in for a moment after it opens.
    Where {
        /// Where it is working, said the way a tab row's own place is.
        place: Option<Place>,
        /// The person's home directory.
        ///
        /// Here because a chip prints `~/Work/crook` and a plugin cannot know
        /// which prefix that is — and because the alternative, handing over a
        /// path already shortened, would be handing over one that cannot be
        /// `cd`-ed to. It reveals nothing the directory beside it does not.
        home: Option<String>,
        /// Lines added in the working tree against `HEAD`, and lines removed.
        ///
        /// Zero for a directory git has said nothing about, because the wire
        /// has no third answer and "nothing changed" is what a chip would
        /// print either way.
        added: u32,
        /// See [`added`](Self::Where::added).
        removed: u32,
    },
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
    /// What the command printed, without the prompt or the line it was typed
    /// on.
    ///
    /// Empty for a command that printed nothing, which is a different thing
    /// from a block whose shell never said where its output began — that is
    /// [`Answer::Failed`], because a plugin handed an empty string would put an
    /// empty fence on somebody's clipboard and never know why.
    Output {
        /// The rows, joined by newlines, with the blank ones at the end
        /// trimmed off.
        text: String,
    },
}

/// Something that happened, which a plugin asked to be told about.
///
/// The other direction from [`Request`], and the only one there is: a guest
/// still never reaches anything. The host calls `crook_event` off the frame
/// path with one of these, on the plugins whose capabilities cover it, and a
/// plugin that exports no `crook_event` is simply not called.
///
/// Deliberately small, and for the reason [`Node`] is: every variant has to be
/// something Crook already knows, described in terms a plugin can act on
/// without being told anything about the person using it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A command in a pane finished. Needs [`Capability::WatchCommands`].
    ///
    /// This is OSC 133 `D` — the shell saying it is done — so it arrives for
    /// every shell Crook's integration is installed in, and not at all in one
    /// where it is not. What ran is *not* here: a plugin that wanted to ring
    /// when something finished does not need the command line, and a plugin
    /// that had it would be reading somebody's history.
    CommandFinished {
        /// Which pane, stable for as long as the pane is open. A plugin that
        /// keeps state per pane can key on it; it means nothing across runs.
        pane: u64,
        /// How it exited, or `None` when the shell reported no status.
        exit: Option<u8>,
        /// How long it ran, in milliseconds, or `None` when the boundary that
        /// started it was never seen.
        ///
        /// Here because "ring only for the slow ones" is the first thing
        /// anybody wants from this and the host is the only side that can
        /// measure it.
        took_millis: Option<u64>,
    },
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
