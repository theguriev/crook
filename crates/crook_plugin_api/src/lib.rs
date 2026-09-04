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

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// What version of this vocabulary a plugin was built against.
///
/// Bumped when a shape below changes in a way that would make an older plugin
/// decode to something other than what it meant. Adding a variant to an enum
/// counts: postcard encodes a variant by its index, so an older host reading a
/// newer plugin's `Node` would read the wrong variant rather than fail.
pub const ABI_VERSION: u32 = 1;

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
}

impl Capability {
    /// The sentence a permission dialog says.
    pub fn sentence(&self) -> String {
        match self {
            Self::ReadSettings => "Read your settings".into(),
            Self::ReadTabs => "See what your tabs are called".into(),
            Self::ReadWorkingDirectory => "See which project the active pane is in".into(),
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
        }
    }
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
