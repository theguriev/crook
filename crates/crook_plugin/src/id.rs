//! What a plugin, an action, a slot and an entry are called.
//!
//! Four named types rather than four `String`s, because every one of them is
//! compared, printed and used as a key, and three of them have a shape a
//! stranger will get wrong on their first try.

use std::fmt;

/// Why a name was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    /// A name with the wrong number of `/`-separated parts.
    Shape {
        /// What was given.
        given: String,
        /// What it should have looked like.
        expected: &'static str,
    },
    /// A part that is empty, or holds something other than lowercase ASCII
    /// letters, digits, `-` and `_`.
    Part {
        /// The offending part.
        part: String,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape { given, expected } => {
                write!(formatter, "{given:?} is not {expected}")
            }
            Self::Part { part } => write!(
                formatter,
                "{part:?} is not a name: lowercase letters, digits, `-` and `_`, and not empty"
            ),
        }
    }
}

impl std::error::Error for IdError {}

/// Whether one `/`-separated part is a name.
///
/// Lowercase only, and deliberately: a store whose index holds both
/// `Eugen/themes` and `eugen/themes` has two plugins that every person reading
/// it believes are one, and the difference is invisible on a case-insensitive
/// filesystem. The set is the one GitHub allows in a repository name minus the
/// dot, which is left out because a name is also a directory and a part called
/// `..` is not a thought worth having.
fn is_part(part: &str) -> bool {
    !part.is_empty()
        && part.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// Who wrote a plugin, and what they called it: `owner/name`.
///
/// The owner is the account the plugin's directory belongs to in the store's
/// repository, which is what makes ownership enforceable by the thing that
/// already enforces it — a `CODEOWNERS` line — rather than by a registry of
/// names somebody has to run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId(String);

impl PluginId {
    /// Parses `owner/name`.
    pub fn parse(given: &str) -> Result<Self, IdError> {
        let mut parts = given.split('/');
        let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(IdError::Shape {
                given: given.to_owned(),
                expected: "`owner/name`",
            });
        };

        for part in [owner, name] {
            if !is_part(part) {
                return Err(IdError::Part {
                    part: part.to_owned(),
                });
            }
        }

        Ok(Self(given.to_owned()))
    }

    /// The account that owns it.
    pub fn owner(&self) -> &str {
        self.0.split('/').next().unwrap_or_default()
    }

    /// What it is called, without its owner.
    pub fn name(&self) -> &str {
        self.0.split('/').nth(1).unwrap_or_default()
    }

    /// The whole of it, as it is written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Something a plugin can be asked to do: `owner/name/action`.
///
/// Named rather than enumerated, and that is the whole change from what Crook
/// has now. Today a chord names one of thirteen variants of a closed enum, so
/// nothing outside the binary can be bound to a key, put in a menu, or invoked
/// by another plugin. An action with a name can be all three, and the plugin
/// that owns it is written in the name — so two plugins cannot quietly claim
/// the same one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionName(String);

impl ActionName {
    /// Parses `owner/name/action`.
    pub fn parse(given: &str) -> Result<Self, IdError> {
        let mut parts = given.split('/');
        let (Some(owner), Some(name), Some(action), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(IdError::Shape {
                given: given.to_owned(),
                expected: "`owner/name/action`",
            });
        };

        for part in [owner, name, action] {
            if !is_part(part) {
                return Err(IdError::Part {
                    part: part.to_owned(),
                });
            }
        }

        Ok(Self(given.to_owned()))
    }

    /// The plugin that owns this action.
    pub fn plugin(&self) -> PluginId {
        let owner_and_name: String = self.0.rsplitn(2, '/').last().unwrap_or_default().to_owned();
        PluginId(owner_and_name)
    }

    /// The whole of it, as it is written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A place in the interface that renders what has been contributed to it.
///
/// A `&'static str` because a slot is *declared by the thing that draws it*,
/// and everything that draws is compiled in. A plugin that only contributes
/// names a slot it did not invent; when a sandboxed plugin names one, the host
/// resolves the string it was given against the slots that exist and refuses
/// the rest.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotId(&'static str);

impl SlotId {
    /// Names a slot. `const`, so slot ids are constants beside the code that
    /// declares them rather than strings spelled out at each call.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The name, as written.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for SlotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// One contribution's own name, unique within the plugin that made it.
///
/// Owned rather than static because a plugin may contribute a row per thing it
/// found — a worktree, a theme, a repository — and those names are not known
/// when it is compiled.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(String);

impl EntryId {
    /// Names an entry. Anything printable will do: it is a key within one
    /// plugin's contributions to one slot, never a public identity.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name, as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
