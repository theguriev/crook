//! OSC 133, the four escape sequences a shell uses to say where a command
//! begins and ends.
//!
//! This module is only the vocabulary and the parser. What a mark *means* —
//! which block it opens, which it closes, which ones are ignored — belongs to
//! [`crate::blocks`], because that decision needs the block that is already
//! open and this one does not.
//!
//! # The standard
//!
//! FinalTerm's semantic prompt marks, as specified by terminal-wg and
//! implemented by iTerm2, VS Code, WezTerm, kitty, Ghostty, foot and Konsole.
//! Four sequences, each terminated by BEL or ST:
//!
//! ```text
//! ESC ] 133 ; A ST            the shell is about to draw a prompt
//! ESC ] 133 ; B ST            the prompt is drawn; the cursor is where you type
//! ESC ] 133 ; C ST            the line was accepted and is about to run
//! ESC ] 133 ; D ; <exit> ST   it finished, with this status
//! ```
//!
//! `A` and `B` have to be embedded *in the prompt string* — `%{…%}` in zsh,
//! `\[…\]` in bash — because the prompt is redrawn on every reflow and the
//! marks must be redrawn with it. `C` and `D` come from `preexec`/`precmd`
//! hooks, and `$?` must be the first thing the `precmd` hook captures or every
//! command reports success.
//!
//! # What is deliberately not read
//!
//! `aid=` (an application id), `cl=` (a fresh-line hint) and kitty's `L` are
//! parsed past and dropped. A conforming consumer must ignore parameters it
//! does not know rather than reject the mark for carrying them, so an emitter
//! that adds one does not silently stop producing blocks here.
//!
//! # Trust
//!
//! None. Anything that can print to the terminal can print these bytes —
//! `cat` a log file containing them and blocks appear. So a mark may only ever
//! move the block model between states, and every string that reaches this
//! crate from the stream is display-only. The private per-session token Warp
//! uses to defend its own protocol has no equivalent in the public standard;
//! the `aid=` parameter is advisory at best.

/// Which of a shell's prompts a [`ShellMark::PromptStart`] belongs to.
///
/// Only [`Self::Initial`] opens a block. A right prompt is drawn on the same
/// row as the primary one, and a continuation prompt appears in the middle of
/// a command that is still being typed, so treating either as a boundary would
/// cut a block in half.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PromptKind {
    /// `k=i`, or no `k=` at all: the primary prompt.
    Initial,
    /// `k=r`: the right-hand prompt.
    Right,
    /// `k=c`: a continuation prompt, drawn when a command spans lines.
    Continuation,
    /// `k=s`: a secondary prompt, drawn by a program reading a line of its own.
    Secondary,
}

impl PromptKind {
    /// Reads the `k=` parameter, which every other kind of parameter is passed
    /// over. An unknown kind reads as [`Self::Initial`], because a mark that
    /// says "a prompt starts here" in a dialect this does not know is still a
    /// prompt start.
    fn parse(parameters: &[&[u8]]) -> Self {
        for parameter in parameters {
            match parameter.strip_prefix(b"k=") {
                Some(b"r") => return Self::Right,
                Some(b"c") => return Self::Continuation,
                Some(b"s") => return Self::Secondary,
                _ => {}
            }
        }
        Self::Initial
    }
}

/// One OSC 133 mark, as it arrived in the byte stream.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShellMark {
    /// `A` (or iTerm2's `P`): a prompt is about to be drawn. The top edge of a
    /// block.
    PromptStart(PromptKind),
    /// `B`: the prompt is finished and the cursor sits where the user types.
    /// Everything from here to [`Self::OutputStart`] is the command line as
    /// the shell echoed it.
    PromptEnd,
    /// `C`: the line has been accepted and is about to run. The top edge of
    /// the output.
    OutputStart,
    /// `D`: the command finished. The status is `None` when the shell sent a
    /// bare `D`, which is what an empty line or a cancelled one produces.
    CommandFinished(Option<i32>),
}

impl ShellMark {
    /// Reads a mark out of the parameters of an OSC sequence, or returns
    /// `None` when they are not an OSC 133 this crate acts on.
    ///
    /// `parameters` is what `vte` split on `;`, so `ESC ] 133 ; D ; 130 BEL`
    /// arrives as `[b"133", b"D", b"130"]`.
    pub(crate) fn parse(parameters: &[&[u8]]) -> Option<Self> {
        let [b"133", command, rest @ ..] = parameters else {
            return None;
        };

        match *command.first()? {
            // `P` is iTerm2's (and Warp's) spelling of a parameterised prompt
            // start. It carries the same `k=` and means the same thing.
            b'A' | b'P' if command.len() == 1 => Some(Self::PromptStart(PromptKind::parse(rest))),
            b'B' if command.len() == 1 => Some(Self::PromptEnd),
            b'C' if command.len() == 1 => Some(Self::OutputStart),
            b'D' if command.len() == 1 => Some(Self::CommandFinished(exit_status(rest))),
            _ => None,
        }
    }
}

/// The exit status carried by a `D` mark: decimal ASCII in the first parameter
/// after the letter.
///
/// `None` for a bare `D`, for an empty parameter, and for anything that is not
/// a number — all of which mean the same thing to a block, which is "it
/// finished and nobody said how".
fn exit_status(parameters: &[&[u8]]) -> Option<i32> {
    let status = parameters.first()?;
    std::str::from_utf8(status).ok()?.parse().ok()
}

#[cfg(test)]
#[path = "marks_tests.rs"]
mod tests;
