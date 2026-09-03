//! Command history: what Up and Down reach for past the ends of the text.
//!
//! The only part worth stating is the draft. Walking up out of a half-typed
//! line and then walking back down must return that line, character for
//! character — every shell and every browser does it, and its absence is
//! noticed the first time someone loses a command they were composing. So the
//! walk keeps the line it started from, and puts it back when the walk ends.

/// How many submitted lines one session keeps. A shell's default `HISTSIZE`,
/// and for the same reason: far more than anyone walks through by hand, and
/// bounded so a pane left open all week cannot grow without limit.
const CAPACITY: usize = 1000;

/// Every line submitted this session, plus where a walk through them is.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<String>,
    /// Where a walk currently sits. `None` means "at the line being typed",
    /// which is past the newest entry.
    cursor: Option<usize>,
    /// The line that was being typed when the walk started.
    draft: Option<String>,
}

impl History {
    /// The lines submitted this session, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Records a submitted line and ends any walk.
    ///
    /// Blank lines are not recorded: pressing Enter on an empty prompt is how
    /// a person asks for a fresh line, not something to recall later.
    pub fn push(&mut self, line: &str) {
        self.reset();
        if line.trim().is_empty() {
            return;
        }
        if self.entries.len() == CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(line.to_string());
    }

    /// The previous entry, or `None` when the walk is already at the oldest
    /// one — or when there is nothing to walk.
    ///
    /// `current` is the text on screen, which is kept as the draft if this
    /// call is what starts the walk.
    pub fn previous(&mut self, current: &str) -> Option<String> {
        let index = match self.cursor {
            Some(0) => return None,
            Some(index) => index - 1,
            None => {
                let newest = self.entries.len().checked_sub(1)?;
                self.draft = Some(current.to_string());
                newest
            }
        };
        self.cursor = Some(index);
        Some(self.entries[index].clone())
    }

    /// The next entry, or the draft when the walk steps back off the newest
    /// one. `None` when no walk is in progress.
    pub fn next(&mut self) -> Option<String> {
        let index = self.cursor? + 1;
        if index < self.entries.len() {
            self.cursor = Some(index);
            return Some(self.entries[index].clone());
        }

        self.cursor = None;
        Some(self.draft.take().unwrap_or_default())
    }

    /// Abandons a walk, and the draft it was holding, without producing a
    /// line. What submitting or clearing the input does.
    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft = None;
    }
}
