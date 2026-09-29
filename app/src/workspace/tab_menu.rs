//! The menu a tab opens, which is about worktrees.
//!
//! # The gesture
//!
//! Right-clicking a tab's row in the panel. This is a context
//! menu — it is *about* the tab rather than a way to switch to it — and the
//! secondary button is the one every desktop opens a context menu with. It was
//! on the left button at first, on the row you were already in, where the
//! click was otherwise free; that made it something you opened by accident on
//! the way to the tab you were already in, and something no other tab could
//! open at all.
//!
//! It opens only when that tab's focused pane is inside a git repository.
//! A menu that opened everywhere and was empty half the time would teach
//! people not to reach for it; a gesture that does nothing where there is
//! nothing to say is the same promise the branch chip already makes, which
//! appears on a row exactly when there is a branch to name.
//!
//! # What it is for
//!
//! One agent, one worktree. Crook's unit of work is an agent and a pane is that
//! agent's workspace — its transcript, its directory, its state — and a git
//! worktree is the same statement made on the filesystem: one branch, one
//! checkout, one place to work that nothing else is standing in. Two agents in
//! one checkout overwrite each other's edits; two agents in two worktrees of
//! one repository do not.
//!
//! So this menu is not a worktree *manager*. It is a way to open one:
//!
//! * the repository's worktrees, each of which opens a tab in it *in the group
//!   the tab the menu was opened on belongs to* — or brings forward the pane
//!   already there;
//! * a way to make one, which asks for a branch name and where it starts, and
//!   locks the checkout it makes until no pane in the window is working in it
//!   or the window closes;
//! * a way to remove one, offered only for a checkout nothing is working in;
//! * a way to remove all of those at once, for the day a repository has eight
//!   of them and seven are finished;
//! * and a way to remove only the ones whose work has already landed, which is
//!   the question that day is really asking.
//!
//! # Tidying up
//!
//! The last two of those are the same offer as the ×, made about the list
//! rather than about a row, and deliberately the *weaker* one: neither ever
//! forces.
//! A confirmation about one checkout can say "there is work in there" and
//! offer to delete it anyway, because a person is looking at the one thing
//! they asked about. A confirmation about six cannot — "remove anyway" over a
//! list is a button whose consequences nobody can hold in their head — so a
//! checkout git would refuse is a checkout this leaves standing, and says it
//! is leaving standing, and the × is still there for the one somebody means.
//!
//! Which is why the face asks after looking rather than before: what a person
//! needs to agree to is the list of branches that will actually go, and that
//! list is not known until every candidate has been `git status`-ed. So the
//! question opens saying it is looking, the same way the menu itself opens
//! saying it is reading.
//!
//! # Work that has landed
//!
//! A finished task's checkout sits on a branch git calls unmerged for ever,
//! because the forge squashed it — so "which of these are finished" is not
//! something the list could say until [`crate::git::merged`] proved it from
//! the repository alone. The menu asks once the list is in, on the background
//! pool like every other read here, and marks a row whose branch is proved
//! with `merged`. The same proof is the second sweep, "Remove N merged
//! checkouts": the free checkouts, narrowed to the proved ones, and otherwise
//! the very same sweep — looked in first, never forced, the branches kept. A
//! branch the proof cannot reach is simply not marked; the first sweep and the
//! × are still there for it.
//!
//! # A wait is the pirate eating it
//!
//! Every face here that is waiting on git says so with the pirate chewing —
//! see [`busy_line`] — and where the wait is a list of checkouts he stands in
//! a row of pellets, one per checkout, having eaten the ones dealt with. The
//! sweep used to be one background task and one sentence, "Working…", for as
//! long as six `git worktree remove` calls took, and a removal deletes
//! whatever was built in the checkout: with a `target/` in each that is
//! minutes of a sentence that does not move, which is what a hang looks like.
//! Now each look and each removal is a step of its own that lands on its own,
//! so the face can say which one is taking the time — and Stop can mean what
//! it says. The one in flight finishes going, because a kill halfway through
//! deleting a directory leaves a checkout neither there nor gone; the ones
//! behind it are spared.
//!
//! The mouth moves on a chain of the workspace's own, which runs only while
//! [`TabMenuState::is_busy`] and ends when nothing is. A list nobody is
//! waiting on has no timer under it, which is the rule the whole window keeps.
//!
//! The branches of one repository stay together because of that second half.
//! A [group](crate::tab::TabGroup) is the repository and its tabs are its
//! checkouts: the panel folds them under one heading — which is made the
//! moment the second checkout arrives and pruned when its last member closes,
//! so there is no group to make first and none to tidy away. A loose tab for
//! each checkout would file the branch away from the work it came out of, with
//! nothing left in the list to say the two were related.
//!
//! It used to open a *pane* instead, splitting the tab, and that was a
//! different and wrong claim: two agents sharing a rectangle and a keyboard is
//! something a person asks for when they want to watch two things at once, not
//! what "give this branch a checkout of its own" means.
//!
//! herdr's shape, which is the tool this was modelled on: there a worktree is
//! not a thing you administer but a workspace with a git checkout behind it,
//! and creating one *opens* it.
//!
//! # Where a new branch starts
//!
//! herdr's `--base` was left out at first, on the grounds that a worktree made
//! from anything but the head you are looking at was a question a menu could
//! not ask well. Leaving it out answered it instead, and wrongly: a creator
//! opened from a tab on a feature branch cut every new branch from that
//! feature, and an agent's work stacked on another's without anybody having
//! said so. Most of them should start from the repository's default branch.
//!
//! So the creator asks, under the name, as a list of places to start from —
//! the tab's own `HEAD`, then the default branch (see
//! [`crate::git::worktree::default_branch`]), then every other local branch.
//! `HEAD` is picked when it opens, so a person who does not look gets a branch
//! from the commit they got one from before there was a question; the default
//! branch is one arrow away. Not quite the same branch: every new one is made
//! with no upstream now, where `branch.autoSetupMerge` set to `always` or
//! `inherit` used to give one made from `HEAD` an upstream (see
//! [`crate::git::worktree::add`]).
//!
//! The whole list rather than those two and a filter, because the filter
//! would be a second field with a caret of its own in a popup whose promise is
//! a name and a press, and every letter typed here belongs to the name: the
//! list is short in practice, the two rows that matter lead it, and it scrolls
//! past a few rows rather than growing the popup.
//!
//! # Starting an agent in it
//!
//! One agent, one worktree, and the creator is where the two meet: under the
//! places to start from it lists the coding agents it found installed, with
//! "Shell only" first and picked, and a prompt field that appears once an
//! agent is picked. What it does with them is type one line — `claude 'fix
//! the login bug'` — into the new tab's composer after the checkout opens,
//! and leave it there. The line is quoted by the host, never spliced (see
//! [`crate::agent::launch_line`]), and it is not sent: a line a person has
//! not seen is a line nobody chose to run, and a pane's composer is where a
//! person already reads a line before pressing Enter on it. The one way to
//! have it run straight away is the Start button, which is a press for
//! exactly that.
//!
//! The branch follows the prompt — `worktree/fix-the-login-bug` — for as long
//! as the field still holds a name the creator put there; the first letter
//! somebody types into it is the end of that. "New task…" in the palette is
//! this same creator opened with the first agent found already picked and the
//! keyboard in the prompt, which is what makes a task one sentence and a
//! press; the tab's own "New worktree…" opens it on "Shell only", which makes
//! exactly the worktree it always made and types nothing.
//!
//! Which agents are installed is a look for their programs on `PATH`, made on
//! the background pool with the repository's reads when the menu opens —
//! never a program started to ask, and never anything on the frame. A shell
//! the host's quoting is not proven in gets the agent's bare name and no
//! prompt field, rather than a line that means something else there.
//!
//! # The two keys
//!
//! Escape and Enter, claimed by
//! [`Workspace::action_for`](super::view::Workspace::action_for) before the
//! element tree sees them — the search box's arrangement, for the search box's
//! reason: [`TextField`](super::text_field::TextField) answers Escape by
//! emptying itself and Enter by doing nothing, so a creator whose branch field
//! holds the keyboard would have no way out that is not a pointer.
//!
//! * **Escape** is Cancel: back to the list from the creator and from the
//!   confirmation, and down from the list itself. One key, one step back.
//! * **Enter** is the button that face leads with: Create in the creator,
//!   where the name is selected the moment it opens so making a worktree is a
//!   name and a press, and Remove in the confirmation, which is a question
//!   already asked once and answered with the pointer that opened it. Create
//!   in the prompt field too, and never Start: Enter is the key a person
//!   presses at the end of a sentence, and that is not the moment they chose
//!   to run a line they have not read yet.
//!
//! Enter stops at the second question. When git refuses over local work the
//! button becomes "Remove anyway", and that one stays a click: a person who
//! pressed Enter and got a warning back should not be able to delete the work
//! it warns about by pressing the same key again.
//!
//! The creator claims the up and down arrows as well, and they move the check
//! down the list in the half that has the keyboard — the places to start from
//! with the name, the agents with the prompt — while the field keeps every
//! letter. In a one-line field those two only jump the caret to an end, which
//! Home and End still do. Tab, where there are agents, moves the keyboard to
//! the other half. The agents' half is a stop of its own while no prompt is
//! asked for — "Shell only", or a shell the quoting is not proven in — with
//! the picked row lit where a caret would otherwise say where the keyboard
//! is, so that every agent is a key away and not only a click.
//!
//! # Reading git off the frame
//!
//! Every git call here happens on the background pool and lands through
//! `ctx.spawn`, never on the render path — the rule the whole git layer is
//! built on. The menu therefore opens *before* it knows what is in the
//! repository and says so, the way a list that is being read says so.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crookui_core::elements::{
    MouseStateHandle, Padding, Paragraph, ScrollStateHandle, Scrollable, Shrinkable, WINDOW_INSET,
};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::worktree::{Local, Worktree};
use crate::tab::TabId;
use crate::theme::theme;

use super::action::{CreatorField, WorkspaceAction, WorktreeAction};
use super::view::Workspace;
use super::window_room::WindowRoom;

/// How wide the popup is.
///
/// Wider than the options menu's 200, because a worktree row carries a path
/// and a path is the one thing here that cannot be shortened without losing
/// what it is for.
pub(super) const MENU_WIDTH: f32 = 260.;

/// The inset around every row.
pub(super) const ROW_INSET: f32 = 12.;

/// The popup's corner radius, which is the options menu's.
const MENU_RADIUS: f32 = 6.;

/// The least the list face is shortened to for a window shorter than it: a
/// few rows' worth, under which a list that scrolls is a list nobody can read.
const MENU_LEAST_HEIGHT: f32 = 120.;

/// The size of the label on a row, and of the line under it.
pub(super) const LABEL_SIZE: f32 = 12.;
pub(super) const PATH_SIZE: f32 = 10.5;

/// The least room between a worktree row's label and whatever shares its
/// line — the badge, or the ×. A label cut with a mark is cut this far short
/// of them, so that the mark does not read as part of the badge.
const LABEL_GAP: f32 = 8.;

/// What the branch field says before anything is typed into it.
const BRANCH_PLACEHOLDER: &str = "branch";

/// What the prompt field says before anything is typed into it.
pub(super) const PROMPT_PLACEHOLDER: &str = "what it should do";

/// How many places to start from the creator shows before the list scrolls.
///
/// Six, for the reason the sweep names six branches: a face that stays one
/// screenful. The two rows that are right most of the time lead the list, so
/// a repository with forty branches costs a scroll — or a few presses of an
/// arrow — only to the person who wants one of the other thirty-eight.
const BASE_ROWS: usize = 6;

/// One place-to-start row's height: the 16px check slot and 2px above and
/// below it, which is the options menu's check row.
const BASE_ROW_HEIGHT: f32 = 20.;

/// The check slot on a place-to-start row, and the check inside it — the
/// options menu's sizes, because a row with a check on it here says what one
/// says there.
const CHECK_SLOT: f32 = 16.;
/// See [`CHECK_SLOT`].
const CHECK_SIZE: f32 = 13.;

/// What the menu is doing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    /// Listing the repository's worktrees.
    #[default]
    Listing,
    /// Making one.
    Creating,
    /// Asking about removing every free checkout at once, or every one whose
    /// work has landed.
    ///
    /// What it is asking about lives in [`TabMenuState::sweep`] rather than
    /// here: this enum is `Copy` and is read by value on the render path and
    /// in `Workspace::action_for`, and a `Vec` of paths in it would end that
    /// for the sake of one variant. The worktrees themselves are kept beside
    /// the mode for the same reason.
    Tidying,
    /// Asking about removing the one at this index, and saying what is in it.
    ///
    /// `refused` is set once git has declined over local work — which is what
    /// turns the button into the one that deletes it anyway.
    Removing {
        /// Which worktree, by its index in [`TabMenuState::worktrees`].
        index: usize,
        /// What is in it, once counted.
        local: Looked,
        /// Whether git has already refused once.
        refused: bool,
    },
}

/// What a look inside one checkout found, or that it has not come back yet.
///
/// Three states and not `Option<Local>`, because the face draws the pirate
/// chewing while the count is out and has to stop when it comes back — and
/// a count that *failed* comes back too. `None` for both would leave him
/// chewing at a checkout git could not be asked about until the person gave
/// up on him.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Looked {
    /// `git status` is still out.
    #[default]
    NotYet,
    /// What it said.
    Found(Local),
    /// It could not be asked, or refused; the log has why.
    Unknown,
}

/// What the menu knows about the repository it is open on.
#[derive(Clone, Debug, Default)]
pub(super) enum Contents {
    /// The list is being read. The menu is up and has nothing to show yet.
    #[default]
    Reading,
    /// This is what git said.
    Ready(Vec<Worktree>),
    /// git could not be asked, or refused. The message is its own.
    Failed(String),
}

/// What a tidy-up would do, once every free checkout has been looked in, and
/// then how far it has got doing it.
///
/// Two of the three states are progress rather than a verdict, and both carry
/// a count because both are drawn as one: the pirate at his place in a row of
/// pellets, one per checkout. There is no "failed". A checkout git could not
/// be asked about is kept with the ones holding work, because the only thing
/// this face may do with a checkout it does not understand is leave it alone;
/// and a checkout git would not remove is counted and logged rather than
/// stopping the ones after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Sweep {
    /// Each free checkout is being looked in, which is a `git status` apiece,
    /// one after another.
    Counting {
        /// How many have answered.
        looked: usize,
        /// How many there are.
        of: usize,
    },
    /// What the button would take, and what it would not.
    Ready {
        /// The checkouts that would go, in the order the list has them.
        going: Vec<Going>,
        /// The branches of the ones being left alone.
        kept: Vec<String>,
    },
    /// The button has been pressed and the checkouts are going, one at a time.
    Removing {
        /// How many `git worktree remove` has finished with, refused or not.
        done: usize,
        /// How many were going.
        of: usize,
        /// The branch of the one under the knife right now.
        current: Option<String>,
        /// Whether Stop has been pressed: the one in flight finishes, because
        /// a kill halfway through deleting a directory leaves a checkout that
        /// is neither there nor gone, and nothing after it starts.
        stopping: bool,
    },
}

impl Default for Sweep {
    fn default() -> Self {
        Self::Counting { looked: 0, of: 0 }
    }
}

impl Sweep {
    /// Whether git is being waited on.
    pub(super) fn is_working(&self) -> bool {
        !matches!(self, Self::Ready { .. })
    }
}

/// One place the creator can start a new branch from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Base {
    /// What its row says: a branch's name, or the head the tab is sitting on.
    pub(super) label: String,
    /// What [`crate::git::worktree::add`] is handed. `None` for the tab's own
    /// `HEAD`, which is what `add` starts from when it is handed nothing, and
    /// a full ref name — `refs/heads/main` — for everything else, so that a
    /// tag sharing a branch's name cannot be what git picks.
    pub(super) reference: Option<String>,
    /// The word on the right of the row, for the two rows that are more than
    /// a branch: the tab's own, and the default.
    pub(super) badge: Option<&'static str>,
}

/// One checkout a tidy-up would remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Going {
    /// Where it is, which is what `git worktree remove` is given.
    pub(super) path: PathBuf,
    /// What the list calls it: its branch, or the head it is sitting on.
    pub(super) label: String,
    /// What is loose in it — which for one of these can only be ignored
    /// files, since anything git would refuse over put it in `kept` instead.
    /// Nobody is warned about those by git, so this face is the only chance.
    pub(super) local: Local,
}

/// One clickable thing in the menu.
///
/// Keyed by identity rather than named one field at a time, for the reason the
/// Themes panel's controls are: the rows are however many worktrees the
/// repository has, and a struct with a field per control could not be written
/// down.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Control {
    /// One worktree's row.
    Worktree(usize),
    /// The × on one worktree's row.
    Remove(usize),
    /// "New worktree…".
    Create,
    /// "Remove N free checkouts…".
    Tidy,
    /// "Remove N merged checkouts…".
    TidyLanded,
    /// The creator's branch field.
    Branch,
    /// One of the creator's places to start from, by its index in
    /// [`TabMenuState::bases`].
    Base(usize),
    /// One of the creator's agents, by its row: `0` is "Shell only".
    Agent(usize),
    /// The creator's prompt field.
    Prompt,
    /// The creator's and the confirmation's "Cancel".
    Cancel,
    /// The creator's "Start", which runs the agent's line rather than
    /// leaving it in the composer.
    Start,
    /// The button that does the thing.
    Confirm,
}

/// Whether a menu is up, on which tab, and what it is showing.
#[derive(Default)]
pub(super) struct TabMenuState {
    /// The tab whose menu is up. `None` is closed — "which tab" *is* the open
    /// flag, because a bool beside it would be a second fact that can disagree
    /// with this one.
    pub(super) tab: Option<TabId>,
    /// The pane the menu was opened from, whose directory names the repository.
    pub(super) pane_directory: Option<PathBuf>,
    /// What the menu is doing.
    pub(super) mode: Mode,
    /// The repository's worktrees, as of when the menu opened.
    pub(super) contents: Contents,
    /// Every branch the repository has, checked out or not, as of when the
    /// menu opened.
    ///
    /// Kept beside the worktrees rather than derived from them, because it
    /// cannot be derived from them: a branch whose checkout has been removed is
    /// in this list and not in that one, and it is precisely the name the next
    /// suggestion must step over. Empty when the read failed, which costs a
    /// worse suggestion and nothing else.
    pub(super) branches: Vec<String>,
    /// The branch new work in the repository usually starts from, as a full
    /// ref, as of when the menu opened.
    ///
    /// `None` where the repository names none or the read failed, which costs
    /// the creator one row and nothing else — the same bargain the branches
    /// above strike.
    pub(super) default_branch: Option<String>,
    /// Where the creator's branch can start from, the tab's own `HEAD` first.
    ///
    /// Made when the creator opens, out of what the menu read when *it*
    /// opened, and kept rather than made again every frame because
    /// [`Self::base`] is an index into it.
    pub(super) bases: Vec<Base>,
    /// Which of [`Self::bases`] is picked: the tab's own `HEAD` until somebody
    /// picks another, which is where this creator started every branch before
    /// it had anything to ask.
    pub(super) base: usize,
    /// How far the places to start from are scrolled, in a repository with
    /// more branches than [`BASE_ROWS`].
    pub(super) base_scroll: ScrollStateHandle,
    /// The coding agents the creator can start, as of when the menu opened:
    /// the ones [`crate::agent::found_on`] found installed, in its order.
    pub(super) agents: Vec<&'static str>,
    /// Which to start in the new checkout, as a row of the creator's list:
    /// `0` is "Shell only", and every other row is one of [`Self::agents`].
    pub(super) agent: usize,
    /// Whether the window's shell reads a line with a quoted prompt in it the
    /// way it was written — see [`crate::plugins::wasm::quoting_holds_in`].
    /// Where it does not, the creator asks for no prompt.
    pub(super) prompt_holds: bool,
    /// Which half of the creator has the keyboard.
    pub(super) field: CreatorField,
    /// The branch name the creator itself last put in the field.
    ///
    /// While the field still says exactly this, nobody has typed a name of
    /// their own, and the name goes on following the prompt; the moment it
    /// says anything else, it is theirs and is left alone.
    pub(super) branch_offered: Option<String>,
    /// Whether the menu was opened to start a task, and goes into the
    /// creator the moment the repository has been read.
    pub(super) task: bool,
    /// What the repository is called: the name of its main checkout's
    /// directory, which is what a person calls it.
    pub(super) repository: Option<String>,
    /// The branches of the listed checkouts whose work is proved to have
    /// landed on the repository's base, once that has been read.
    ///
    /// Read after the list rather than with it, because it is the slow half —
    /// a pass over the base's history — and a list that waited for it would
    /// open a second late for a badge. `None` until it lands, and drawn
    /// exactly as an empty set is: no badge, no row. A proof that has not
    /// arrived and one that found nothing both offer nothing to tidy.
    pub(super) landed: Option<HashSet<String>>,
    /// Where Crook keeps checkouts it made.
    pub(super) store: Option<PathBuf>,
    /// What a tidy-up would take, while one is being asked about.
    ///
    /// Meaningless outside [`Mode::Tidying`], which is what reads it: the mode
    /// is the question and this is the answer being assembled for it.
    pub(super) sweep: Sweep,
    /// What git said about the last thing that was asked of it, if it refused.
    pub(super) problem: Option<String>,
    /// Whether a git command is running for the creator or the confirmation
    /// right now. The sweep keeps its own, inside [`Sweep`], because its
    /// running state has a count in it.
    pub(super) working: bool,
    /// Which question the menu is on, counted up every time it changes.
    ///
    /// Every answer git gives lands through `ctx.spawn` on a menu that has
    /// had time to move on: taken down, opened on another tab, walked back to
    /// the list, opened on the same question a second time. An answer carries
    /// the epoch it was asked under and is dropped on the floor unless the
    /// menu is still there — one number to compare rather than a tab, a mode
    /// and a sweep state to compare separately, which is what the sweep used
    /// to do and is what a stepwise chain cannot do, since every step of it
    /// lands into the same state the last one left.
    pub(super) epoch: u64,
    /// Which time the menu has been opened, counted up every time it is.
    ///
    /// Coarser than [`Self::epoch`], and for the two answers that must not be
    /// dropped when the question changes: a checkout made or removed. Walking
    /// back from the confirmation to the list while git is removing still
    /// wants the answer — `working` is only put down by it, and the list has to
    /// be read again once the checkout is gone — so those two cannot compare
    /// epochs. They used to compare tabs instead, which cannot tell the menu
    /// that asked from the same tab's menu taken down and opened again: that
    /// one was handed a failure it never asked about, a creator thrown back to
    /// the list, and "Remove anyway" armed on a confirmation git had not
    /// answered.
    pub(super) opening: u64,
    /// How many frames into the bite the pirate is.
    ///
    /// Counted up by the chain in `Workspace::keep_chomping` while
    /// [`Self::is_busy`], and put back to a shut mouth when it stops. Never
    /// read for anything but which frame to draw.
    pub(super) chomp: usize,
    /// Whether that chain is running, so that a second thing becoming busy
    /// while the first still is does not start a second one.
    pub(super) chomping: bool,
    /// The row the keyboard is standing on while the list is showing.
    ///
    /// `None` is the ordinary state — nothing is picked out, and the pointer
    /// is the only thing lighting a row. The first arrow key lands on an end
    /// of the list rather than on a row somebody would have to find, which is
    /// how every list in this window that can be walked behaves.
    ///
    /// Only [`Mode::Listing`] reads it. The creator's arrows move
    /// [`Self::base`] instead, and the other two modes are one question with
    /// two buttons, which Enter already answers.
    pub(super) selected: Option<usize>,
    /// How far the list of checkouts is scrolled, in a window too short for
    /// them all.
    pub(super) scroll: ScrollStateHandle,
    /// One mouse state per control, made on the control's first frame.
    controls: std::cell::RefCell<HashMap<Control, MouseStateHandle>>,
}

impl TabMenuState {
    /// Whether the menu is up.
    pub(super) fn is_open(&self) -> bool {
        self.tab.is_some()
    }

    /// Whether the face on screen is waiting on git, which is when the
    /// pirate chews.
    ///
    /// One predicate for all four faces, so that the chain that moves his
    /// mouth has one question to ask and the faces cannot disagree with it
    /// about whether he should be moving.
    pub(super) fn is_busy(&self) -> bool {
        if !self.is_open() {
            return false;
        }
        if self.working || matches!(self.contents, Contents::Reading) {
            return true;
        }
        match self.mode {
            Mode::Tidying => self.sweep.is_working(),
            Mode::Removing { local, .. } => local == Looked::NotYet,
            Mode::Listing | Mode::Creating => false,
        }
    }

    /// Moves on to the next question, and says which one that is.
    ///
    /// Called by everything that changes what the menu is asking, before it
    /// asks git anything for the new question; an answer to the old one that
    /// lands afterwards compares its epoch with this and goes nowhere.
    pub(super) fn next_epoch(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        self.epoch
    }

    /// The mouse state for one control.
    pub(super) fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }

    /// The worktrees the menu is showing, or nothing while it has none.
    pub(super) fn worktrees(&self) -> &[Worktree] {
        match &self.contents {
            Contents::Ready(worktrees) => worktrees,
            _ => &[],
        }
    }

    /// Steps the keyboard's row `by` places, or onto an end of the list from
    /// nothing, and reports whether it moved.
    ///
    /// Clamped rather than wrapped: a list of checkouts is short and read top
    /// to bottom, and an arrow that jumped from the last row back to the first
    /// would be one a person pressing it twice cannot predict.
    pub(super) fn move_selection(&mut self, by: isize) -> bool {
        let count = self.worktrees().len();
        if count == 0 {
            return false;
        }
        let last = count as isize - 1;
        let next = match self.selected {
            Some(at) => (at as isize + by).clamp(0, last),
            None if by < 0 => last,
            None => 0,
        };
        let next = Some(next as usize);
        let moved = self.selected != next;
        self.selected = next;
        if moved && let Some(at) = next {
            scroll_into_view(&self.scroll, at, count);
        }
        moved
    }

    /// Steps the creator's place to start from `by` places, and reports
    /// whether it moved.
    ///
    /// Clamped, for [`Self::move_selection`]'s reason. There is no "nothing
    /// picked" to land from: the tab's own `HEAD` is picked from the moment
    /// the creator opens, because a worktree has to start somewhere.
    pub(super) fn move_base(&mut self, by: isize) -> bool {
        let count = self.bases.len();
        if count == 0 {
            return false;
        }
        let next = (self.base as isize + by).clamp(0, count as isize - 1) as usize;
        let moved = self.base != next;
        self.base = next;
        if moved {
            scroll_into_view(&self.base_scroll, next, count);
        }
        moved
    }

    /// The agent the creator would start, or `None` for "Shell only".
    pub(super) fn picked_agent(&self) -> Option<&'static str> {
        let index = self.agent.checked_sub(1)?;
        self.agents.get(index).copied()
    }

    /// Whether the creator is asking for a prompt: an agent is picked, and
    /// the shell reads the line a prompt would be put on.
    pub(super) fn asks_for_a_prompt(&self) -> bool {
        self.prompt_holds && self.picked_agent().is_some()
    }

    /// Steps the creator's agent `by` rows, "Shell only" included, and
    /// reports whether it moved. Clamped, for [`Self::move_selection`]'s
    /// reason.
    pub(super) fn move_agent(&mut self, by: isize) -> bool {
        let last = self.agents.len() as isize;
        let next = (self.agent as isize + by).clamp(0, last) as usize;
        let moved = self.agent != next;
        self.agent = next;
        moved
    }

    /// Forgets every hover and press the menu was holding.
    ///
    /// Called when it closes and when it changes mode: every control is about
    /// to stop existing without seeing a hover-out, and the next opening would
    /// come back with a row lit under a pointer that is somewhere else.
    pub(super) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }
}

/// Brings row `at` of a scrolled list of `count` rows inside its viewport.
///
/// The rows are one height, so a row's top is arithmetic on what the last
/// layout measured: the content the scroll holds divided by the count. Before
/// any layout the measurement is zero and says nothing, and a list that fits
/// has nothing to move.
fn scroll_into_view(scroll: &ScrollStateHandle, at: usize, count: usize) {
    let mut scroll = scroll.lock();
    let viewport = scroll.viewport();
    if viewport <= 0. || !scroll.is_scrollable() {
        return;
    }
    let row = (scroll.max_offset() + viewport) / count as f32;
    let top = at as f32 * row;
    let offset = scroll.offset();
    if top < offset {
        scroll.scroll_to(top);
    } else if top + row > offset + viewport {
        scroll.scroll_to(top + row - viewport);
    }
}

/// Where a new branch can start, in the order the creator offers them.
///
/// The tab's own `HEAD` first, labelled the way its row in the list is, which
/// is where the creator started every branch before it asked. The default
/// branch second, one arrow away, because it is the answer that is right most
/// of the time. Then every other local branch in git's order, for the branch
/// that is meant to stack on another.
///
/// Nothing is offered twice. The tab's own branch is its first row and not
/// also a later one, and a default branch that *is* the tab's own branch —
/// the tab is on `main` — is that first row too. A remote default is not the
/// local branch of the same name: `origin/main` and a `main` somebody last
/// pulled a week ago are two different places to start, and both are offered.
///
/// Pure, and given the menu's answers rather than the workspace, so the rule
/// can be read in one place and is the same on every frame that draws it.
pub(super) fn bases(
    worktrees: &[Worktree],
    directory: Option<&Path>,
    branches: &[String],
    default_branch: Option<&str>,
) -> Vec<Base> {
    let head = holding(worktrees, directory).and_then(|index| worktrees.get(index));
    let head_branch = head.and_then(|worktree| worktree.branch.as_deref());
    let head_reference = head_branch.map(|branch| format!("refs/heads/{branch}"));

    let mut bases = vec![Base {
        label: head.map_or_else(|| "HEAD".to_owned(), branch_label),
        reference: None,
        badge: Some("this tab"),
    }];
    if let Some(default) =
        default_branch.filter(|default| Some(*default) != head_reference.as_deref())
    {
        bases.push(Base {
            label: short_ref(default).to_owned(),
            reference: Some(default.to_owned()),
            badge: Some("default"),
        });
    }
    for branch in branches {
        let reference = format!("refs/heads/{branch}");
        if Some(branch.as_str()) == head_branch || Some(reference.as_str()) == default_branch {
            continue;
        }
        bases.push(Base {
            label: branch.clone(),
            reference: Some(reference),
            badge: None,
        });
    }
    bases
}

/// A ref as a person names it: `main` for `refs/heads/main`, `origin/main`
/// for `refs/remotes/origin/main`, and anything else whole.
fn short_ref(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

/// The whole popup.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let ui = workspace.fonts().ui;

    let body = match state.mode {
        Mode::Listing => listing(workspace, ui),
        Mode::Creating => creator(workspace, ui),
        Mode::Tidying => tidying(workspace, ui),
        Mode::Removing {
            index,
            local,
            refused,
        } => confirmation(workspace, index, local, refused, ui),
    };

    let popup = ConstrainedBox::new(
        Container::new(body)
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
            .with_vertical_padding(6.)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish();

    // The list is the one face that grows with the repository, and the one
    // that scrolls: a window shorter than the list of checkouts is handed
    // the list its height, and the rows scroll behind the header and above
    // the row that makes another. The other three faces are a question with
    // two buttons, and are left their height.
    if state.mode == Mode::Listing {
        WindowRoom::new(popup)
            .with_height_inset(WINDOW_INSET, MENU_LEAST_HEIGHT)
            .finish()
    } else {
        popup
    }
}

/// The list: the repository's worktrees, and the way to another.
fn listing(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(
            state
                .repository
                .clone()
                .unwrap_or_else(|| "Worktrees".to_owned()),
            ui,
        ));

    match &state.contents {
        Contents::Reading => {
            column.add_child(busy_line(state, "Reading the repository…", None, ui));
        }
        Contents::Failed(problem) => column.add_child(note(problem.as_str(), ui)),
        Contents::Ready(worktrees) => {
            let mut rows = Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
            for (index, worktree) in worktrees.iter().enumerate() {
                rows.add_child(worktree_row(workspace, index, worktree, ui));
            }
            // The rows take what the header and the rows under them leave,
            // and scroll inside it: a loose flexible child, so a short list
            // is still its own height and only a long one gives way.
            column.add_child(
                Shrinkable::new(
                    1.,
                    Scrollable::new(state.scroll.clone(), rows.finish())
                        .with_scrollbar(theme().overlay_3)
                        .finish(),
                )
                .finish(),
            );
        }
    }

    column.add_child(divider());
    column.add_child(create_row(workspace, ui));
    // Absent rather than inert where there is nothing to tidy, which is the
    // promise this whole menu is built on: a repository with one checkout has
    // no free ones, and a row offering to remove none of them is a row that
    // teaches people the menu is full of things that do nothing.
    let free = free_checkouts(workspace).len();
    if free > 0 {
        column.add_child(sweep_row(
            workspace,
            Control::Tidy,
            match free {
                1 => "Remove 1 free checkout…".to_owned(),
                free => format!("Remove {free} free checkouts…"),
            },
            WorktreeAction::AskTidy,
            ui,
        ));
    }
    // The narrower offer, under the broader one and on the same terms: absent
    // until something has been proved, and never a checkout the row above
    // would not also take. Named by the word the rows it takes are marked
    // with, and short enough for one line of a menu this wide, which "whose
    // work has landed" is not.
    let landed = landed_checkouts(workspace).len();
    if landed > 0 {
        column.add_child(sweep_row(
            workspace,
            Control::TidyLanded,
            match landed {
                1 => "Remove 1 merged checkout…".to_owned(),
                landed => format!("Remove {landed} merged checkouts…"),
            },
            WorktreeAction::AskTidyLanded,
            ui,
        ));
    }
    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }

    column.finish()
}

/// Whether a checkout is one this menu may take away.
///
/// Never the main checkout, never one somebody is working in, never one
/// somebody else has locked, and never one git has already noticed is not
/// there. git refuses all four, and an × that always fails is worse than no ×
/// at all — the locked case especially, because a lock is somebody saying an
/// agent is working in there, which is exactly what stops one agent tidying
/// away another's work.
///
/// A lock Crook took is the exception, and the reason is the second
/// conjunct. A window takes its own lock off a checkout once none of its
/// panes is working in it, and the rest when it closes, so Crook's lock on a
/// checkout nothing in the window is working in is one a Crook that crashed
/// or was killed left behind — and holding the checkout against Crook's own
/// menu for ever would make the lock a trap rather than a statement. Removing
/// it takes that lock off first. The prefix cannot say which Crook took it,
/// so a second Crook window's checkout reads the same; that is the one thing
/// this cannot tell apart, and it is no more than the menu offered before
/// there was a lock.
///
/// One function rather than the same four conjuncts written twice, because
/// the × on a row and the row that sweeps all of them have to mean the same
/// thing by "free": a person who has read what the × is offered for should not
/// have to find out that the other one goes further.
fn removable(worktree: &Worktree, occupied: bool) -> bool {
    !worktree.is_main
        && !occupied
        && !worktree.is_locked_by_another()
        && worktree.prunable.is_none()
}

/// Every free checkout, as its path and the name the list calls it by.
///
/// The pane directories are read once for the whole list rather than once per
/// row, which is the only difference between this and asking [`removable`]
/// about each row in turn.
pub(super) fn free_checkouts(workspace: &Workspace) -> Vec<(PathBuf, String)> {
    free_checkouts_where(workspace, |_| true)
}

/// Every free checkout whose work is proved to have landed.
///
/// A narrowing of [`free_checkouts`] and never anything else, so that the
/// second sweep cannot reach a checkout the first would have left alone: a
/// proof says the work is safe on the base, not that nobody is in the
/// directory.
pub(super) fn landed_checkouts(workspace: &Workspace) -> Vec<(PathBuf, String)> {
    let state = workspace.tab_menu();
    free_checkouts_where(workspace, |worktree| has_landed(state, worktree))
}

/// The free checkouts `also` agrees to.
fn free_checkouts_where(
    workspace: &Workspace,
    also: impl Fn(&Worktree) -> bool,
) -> Vec<(PathBuf, String)> {
    let state = workspace.tab_menu();
    let worktrees = state.worktrees();
    let directories = workspace.pane_directories();

    worktrees
        .iter()
        .enumerate()
        .filter(|(index, worktree)| {
            let occupied = holding(worktrees, state.pane_directory.as_deref()) == Some(*index)
                || directories
                    .iter()
                    .any(|(_, directory)| holding(worktrees, Some(directory)) == Some(*index));
            removable(worktree, occupied) && also(worktree)
        })
        .map(|(_, worktree)| (worktree.path.clone(), branch_label(worktree)))
        .collect()
}

/// Whether a checkout's branch is proved to have landed on the base.
///
/// Never the main checkout: it is on the base, or on whatever the repository
/// was cloned onto, and "merged" on the row a person works from is a
/// statement about nothing they did.
pub(super) fn has_landed(state: &TabMenuState, worktree: &Worktree) -> bool {
    !worktree.is_main
        && worktree.branch.as_ref().is_some_and(|branch| {
            state
                .landed
                .as_ref()
                .is_some_and(|landed| landed.contains(branch))
        })
}

/// The branches of `worktrees` whose work has landed on the base of the
/// repository `directory` is in.
///
/// **Blocking**: the base, then the proof, which is a pass over the base's
/// history. Background pool only — and the one place the menu asks either
/// question, so that the list and a snapshot of it cannot disagree about
/// which checkouts are asked about. The main checkout is left out for the
/// reason [`has_landed`] gives, and a detached one has no branch to prove.
pub(super) fn landed_branches(directory: &Path, worktrees: &[Worktree]) -> HashSet<String> {
    let branches: Vec<String> = worktrees
        .iter()
        .filter(|worktree| !worktree.is_main)
        .filter_map(|worktree| worktree.branch.clone())
        .collect();
    if branches.is_empty() {
        return HashSet::new();
    }
    let Some(base) = crate::git::merged::base_of(directory) else {
        return HashSet::new();
    };
    crate::git::merged::merged(directory, &base, &branches)
        .into_keys()
        .collect()
}

/// One worktree.
fn worktree_row(
    workspace: &Workspace,
    index: usize,
    worktree: &Worktree,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let worktrees = state.worktrees();
    let here = holding(worktrees, state.pane_directory.as_deref()) == Some(index);
    let elsewhere = !here
        && workspace
            .pane_directories()
            .iter()
            .any(|(_, directory)| holding(worktrees, Some(directory)) == Some(index));
    let removable = removable(worktree, here || elsewhere);

    let label = branch_label(worktree);
    let path = crate::git::user_friendly_path(&worktree.path, workspace.home());
    let status = if here {
        Some("this tab")
    } else if elsewhere {
        Some("open")
    } else if worktree.is_locked_by_another() {
        // Crook's own lock says nothing here: with no pane in the checkout it
        // is one left behind, and the row offers the × instead.
        Some("locked")
    } else if worktree.prunable.is_some() {
        // Registered, and its directory is gone. Saying so is the difference
        // between a row that looks broken and one that explains itself.
        Some("missing")
    } else {
        None
    };
    // About the branch rather than the checkout, so it goes beside whatever
    // the checkout's own word is instead of replacing it: an agent's tab still
    // open on work that has landed is the tab most worth knowing about.
    let badge = match (status, has_landed(state, worktree)) {
        (Some(status), true) => Some(format!("{status} · merged")),
        (Some(status), false) => Some(status.to_owned()),
        (None, true) => Some("merged".to_owned()),
        (None, false) => None,
    };

    let remove = state.control(Control::Remove(index));
    // The row's own click has to decline the press the × claimed. A
    // `Hoverable` runs its handler whether or not a descendant already
    // handled the release, so without this the × dispatches `AskRemove` and
    // the row dispatches `Show` on top of it — the confirmation is unreachable
    // and a pane opens in the checkout somebody was asking to delete. The tab
    // strip's close button is guarded exactly this way, and for exactly this
    // reason.
    let guard = remove.clone();
    // The keyboard's row is lit exactly as the pointer's is, and carries the ×
    // on the same terms: a person walking this list with the arrows has to be
    // offered what a person hovering it is offered.
    let picked = state.selected == Some(index);
    Hoverable::new(state.control(Control::Worktree(index)), move |mouse| {
        let hovered = mouse.is_hovered() || picked;

        // The label takes what the badge or the × leaves, less a gap, and is
        // cut with a mark where it runs out: measured free as a plain row
        // child it was as wide as the branch, and a branch named at the
        // length people name them stopped at the popup's border on whatever
        // letter fell there. `Expanded` is what hands it the width; `Align`
        // is what keeps a short one at the left of it.
        let label = Text::new(label.clone(), ui, LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_ellipsis(Cut::End)
            .with_style(if here {
                Properties {
                    weight: Weight::Semibold,
                    ..Properties::default()
                }
            } else {
                Properties::default()
            })
            .finish();
        let mut line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Align::new(Container::new(label).with_margin_right(LABEL_GAP).finish())
                        .left()
                        .finish(),
                )
                .finish(),
            );

        // The badge and the × share the right edge. The checkout's own words
        // and the × never meet — a checkout somebody is working in is exactly
        // the one that must not be removable — so the only thing the × can
        // sit beside is `merged`, which is the row it is most likely to be
        // pressed on.
        if let Some(badge) = &badge {
            line.add_child(
                Text::new(badge.clone(), ui, PATH_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            );
        }
        if removable && hovered {
            line.add_child(
                Container::new(remove_button(remove.clone(), index))
                    .with_margin_left(if badge.is_some() { 6. } else { 0. })
                    .finish(),
            );
        }

        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(line.finish())
                .with_child(
                    Container::new(
                        Text::new(path.clone(), ui, PATH_SIZE)
                            .with_color(theme().text_muted)
                            .with_ellipsis(Cut::Start)
                            .finish(),
                    )
                    .with_margin_top(1.)
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(if hovered {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_horizontal_padding(ROW_INSET)
        .with_vertical_padding(5.)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        if guard.lock().is_hovered() {
            return;
        }
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::Show(index)));
    })
    .finish()
}

/// The × that offers to remove a checkout.
fn remove_button(state: MouseStateHandle, index: usize) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        ConstrainedBox::new(
            Align::new(
                Icon::new(Lucide::X, 11.)
                    .with_color(if mouse.is_hovered() {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .finish(),
        )
        .with_width(16.)
        .with_height(16.)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::AskRemove(index)));
    })
    .finish()
}

/// "New worktree…", which is the only row that is not a worktree.
fn create_row(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    // Inert until the repository has been read, because the name it offers has
    // to be one no existing checkout is using. A disabled control here has no
    // click handler at all rather than a handler that returns early, which is
    // the rule everywhere else in this application.
    let ready = matches!(state.contents, Contents::Ready(_));

    let row = Hoverable::new(state.control(Control::Create), move |mouse| {
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Icon::new(Lucide::Plus, 12.)
                            .with_color(theme().text_muted)
                            .finish(),
                    )
                    .with_margin_right(8.)
                    .finish(),
                )
                .with_child(
                    Text::new("New worktree…", ui, LABEL_SIZE)
                        .with_color(if ready {
                            theme().text_primary
                        } else {
                            theme().text_muted
                        })
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(if ready && mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_horizontal_padding(ROW_INSET)
        .with_vertical_padding(6.)
        .finish()
    });

    if ready {
        row.on_click(|_, ctx, _| {
            ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::StartCreating));
        })
        .finish()
    } else {
        row.finish()
    }
}

/// "Remove 3 free checkouts…", which is the × made about the whole list — or
/// about the part of it whose work has landed.
///
/// Drawn only where there is something to take, so it needs no inert state:
/// [`listing`] does not add it otherwise, and hands it the sentence it counted
/// to decide that.
fn sweep_row(
    workspace: &Workspace,
    control: Control,
    label: String,
    action: WorktreeAction,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    Hoverable::new(state.control(control), move |mouse| {
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        // The same × the rows offer, because this is the same
                        // offer: a mark of its own here would say it was a
                        // different kind of removal.
                        Icon::new(Lucide::X, 12.)
                            .with_color(theme().text_muted)
                            .finish(),
                    )
                    .with_margin_right(8.)
                    .finish(),
                )
                .with_child(
                    // Counted in the label, because how many is the whole of
                    // what a person needs to decide whether to look: "tidy
                    // up" on a repository with one stale checkout and on one
                    // with nine reads the same.
                    Text::new(label.clone(), ui, LABEL_SIZE)
                        .with_color(theme().text_primary)
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(if mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_horizontal_padding(ROW_INSET)
        .with_vertical_padding(6.)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(action));
    })
    .finish()
}

/// Making one: a branch name, where it would go, where it starts, and which
/// agent to start in it.
///
/// One field for the worktree, because there is one thing to type. herdr asks
/// the same question and derives the directory rather than asking for it,
/// which is right: a person choosing a branch name has said everything that
/// distinguishes one worktree from another, and a directory they have to
/// invent as well is a second chance to get it wrong. Where the branch starts
/// is a pick rather than a field — see the module's own account of it — and
/// it is already made when the creator opens. The second field is the agent's
/// prompt, and it is there only once an agent is picked.
fn creator(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header("New worktree", ui));

    // The field belongs to `crook/worktrees` rather than to this menu — see
    // that plugin — so it is looked up rather than held. `None` cannot happen
    // while this face is on screen: the plugin that claimed the field is the
    // one whose entry opens the menu this face is a mode of.
    if let Some(branch) = workspace.worktree_branch() {
        column.add_child(
            Container::new(
                super::text_field::TextField::new(
                    branch.clone(),
                    workspace.clipboard().clone(),
                    workspace.fonts(),
                    state.control(Control::Branch),
                    BRANCH_PLACEHOLDER,
                )
                .with_focus(WorkspaceAction::Worktree(WorktreeAction::Focus(
                    CreatorField::Branch,
                )))
                .finish(),
            )
            .with_horizontal_padding(ROW_INSET)
            .finish(),
        );
    }

    // The path is shown rather than asked for, and it is shown *live*: it is
    // the answer to "where will this end up", which is a question about the
    // name being typed.
    if let Some(checkout) = checkout_for(workspace) {
        let path = crate::git::user_friendly_path(&checkout, workspace.home());
        column.add_child(
            Container::new(
                Text::new(path, ui, PATH_SIZE)
                    .with_color(theme().text_muted)
                    .with_ellipsis(Cut::Start)
                    .finish(),
            )
            .with_horizontal_padding(ROW_INSET)
            .with_margin_top(6.)
            .finish(),
        );
    }

    // Under the path, because the name and where it goes are one answer and
    // this is a second question.
    if !state.bases.is_empty() {
        column.add_child(
            Container::new(header("Start from", ui))
                .with_margin_top(10.)
                .finish(),
        );
        let mut rows = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (index, base) in state.bases.iter().enumerate() {
            rows.add_child(check_row(
                state.control(Control::Base(index)),
                state.base == index,
                false,
                &base.label,
                base.badge,
                WorktreeAction::PickBase(index),
                ui,
            ));
        }
        // Its own height up to a few rows, and scrolled past that: a scroll
        // is handed a bounded height here and measures its rows unbounded,
        // so a short list is its own size and only a long one gives way.
        column.add_child(
            ConstrainedBox::new(
                Scrollable::new(state.base_scroll.clone(), rows.finish())
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .with_max_height(BASE_ROWS as f32 * BASE_ROW_HEIGHT)
            .finish(),
        );
    }

    // Absent where no agent was found, which leaves the creator exactly what
    // it was before it could start one.
    if !state.agents.is_empty() {
        column.add_child(
            Container::new(header("Agent", ui))
                .with_margin_top(10.)
                .finish(),
        );
        // The picked row is lit while the arrows walk this list and there is
        // no prompt field to hold a caret: nothing else on the face would say
        // where the keyboard is.
        let lit = state.field == CreatorField::Agent && !state.asks_for_a_prompt();
        column.add_child(check_row(
            state.control(Control::Agent(0)),
            state.agent == 0,
            lit && state.agent == 0,
            "Shell only",
            None,
            WorktreeAction::PickAgent(0),
            ui,
        ));
        for (index, agent) in state.agents.iter().enumerate() {
            let row = index + 1;
            // The program's own name, and the word that will be typed beside
            // it, since that word is what the composer is about to say.
            let program = crate::agent::program(agent).unwrap_or(agent);
            column.add_child(check_row(
                state.control(Control::Agent(row)),
                state.agent == row,
                lit && state.agent == row,
                program,
                (program != *agent).then_some(*agent),
                WorktreeAction::PickAgent(row),
                ui,
            ));
        }
    }

    // Under the agents, because it is what the picked one is asked; and only
    // once one is picked, since a prompt with nothing to hear it is a field
    // that does nothing.
    if state.asks_for_a_prompt()
        && let Some(prompt) = workspace.worktree_prompt()
    {
        column.add_child(
            Container::new(
                super::text_field::TextField::new(
                    prompt.clone(),
                    workspace.clipboard().clone(),
                    workspace.fonts(),
                    state.control(Control::Prompt),
                    PROMPT_PLACEHOLDER,
                )
                .with_focus(WorkspaceAction::Worktree(WorktreeAction::Focus(
                    CreatorField::Agent,
                )))
                .with_edit(WorkspaceAction::Worktree(WorktreeAction::PromptEdited))
                .finish(),
            )
            .with_horizontal_padding(ROW_INSET)
            .with_margin_top(6.)
            .finish(),
        );
    }

    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }
    // Under the path rather than in the button, which used to say "Working…"
    // in the accent as though it could still be pressed. What is being waited
    // on is a whole working tree being written, and on a large repository
    // that is genuinely tens of seconds.
    if state.working {
        column.add_child(busy_line(state, "Checking it out…", None, ui));
    }

    // Start only where there is an agent to start. Create stays the button
    // the face leads with — the one Enter presses — and leaves the agent's
    // line in the composer for a person to read first.
    let start = state
        .picked_agent()
        .map(|_| ("Start", Some(WorktreeAction::Start)));
    column.add_child(button_row(
        workspace,
        start,
        "Create",
        Some(WorktreeAction::Create),
        ui,
    ));
    column.finish()
}

/// One choice of several in the creator — a place to start from, or an
/// agent: a check slot, the name, and a badge for a row that is more than its
/// name.
///
/// The options menu's check row, because it is the same control — one choice
/// out of several, made by pressing it — and a person who has used one has
/// used this. The check is the only difference between the picked row and the
/// rest; the pointer lights whichever row it is over, as it does on every row
/// in this menu, and `lit` lights it as well for a keyboard standing on it,
/// the way the list's rows are lit for the keyboard's row. The badge sits
/// where the list's rows put theirs, and says "this tab" for the same
/// checkout the list says it about.
fn check_row(
    control: MouseStateHandle,
    picked: bool,
    lit: bool,
    label: &str,
    badge: Option<&'static str>,
    action: WorktreeAction,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = label.to_owned();

    Hoverable::new(control, move |mouse| {
        let check: Box<dyn Element> = if picked {
            Align::new(
                Icon::new(Lucide::Check, CHECK_SIZE)
                    .with_color(theme().text_primary)
                    .finish(),
            )
            .finish()
        } else {
            Empty::new().finish()
        };

        let mut line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    ConstrainedBox::new(check)
                        .with_width(CHECK_SLOT)
                        .with_height(CHECK_SLOT)
                        .finish(),
                )
                .with_margin_right(8.)
                .finish(),
            )
            // Cut with a mark where it runs out, for the reason a worktree
            // row's label is: branches are named at the length people name
            // them, and the popup is 260 wide.
            .with_child(
                Expanded::new(
                    1.,
                    Align::new(
                        Container::new(
                            Text::new(label.clone(), ui, LABEL_SIZE)
                                .with_color(theme().text_primary)
                                .with_ellipsis(Cut::End)
                                .finish(),
                        )
                        .with_margin_right(LABEL_GAP)
                        .finish(),
                    )
                    .left()
                    .finish(),
                )
                .finish(),
            );
        if let Some(badge) = badge {
            line.add_child(
                Text::new(badge, ui, PATH_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            );
        }

        Container::new(line.finish())
            .with_horizontal_padding(ROW_INSET)
            .with_vertical_padding((BASE_ROW_HEIGHT - CHECK_SLOT) / 2.)
            .with_background_color(if lit || mouse.is_hovered() {
                theme().overlay_1
            } else {
                Color::TRANSPARENT
            })
            .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(action));
    })
    .finish()
}

/// How many of the branches going are named before the list gives up counting.
///
/// Six, which is a face that stays one screenful. Past that the names have
/// stopped being a list somebody reads and become a wall they scroll, and the
/// count in the button is the fact they were after anyway.
const NAMED: usize = 6;

/// Removing every free checkout, which asks once and never forces.
///
/// The order is the order of the sentence a person is being asked to agree to:
/// what goes, what that costs, and what is being left where it is.
fn tidying(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    // Singular wherever exactly one checkout is in play, the way the button
    // below and the "Remove 1 free checkout" entry that opens this both count.
    let count = match &state.sweep {
        Sweep::Counting { of, .. } | Sweep::Removing { of, .. } => *of,
        Sweep::Ready { going, .. } => going.len(),
    };
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(
            if count == 1 {
                "Remove this checkout?"
            } else {
                "Remove these checkouts?"
            },
            ui,
        ));

    let (going, kept) = match &state.sweep {
        // Every candidate is a `git status`, so the question is on screen
        // before it can be answered — the same order the list itself opens in,
        // and for the same reason: a popup that appeared once git had finished
        // would be a gesture that does nothing for half a second. The pirate
        // stands at how far through the list the looking has got.
        Sweep::Counting { looked, of } => {
            column.add_child(busy_line(
                state,
                &format!("Looking in {} of {of}…", (looked + 1).min(*of)),
                Some((*looked, *of)),
                ui,
            ));
            column.add_child(buttons(workspace, "Remove", None, ui));
            return column.finish();
        }
        // Pressed, and going. Each checkout is a `git worktree remove` that
        // deletes a directory tree, and a tree with a build in it takes as
        // long as it takes — so the face says which one is going and how many
        // are behind it, and offers to stop after it rather than nothing.
        Sweep::Removing {
            done,
            of,
            current,
            stopping,
        } => {
            let nth = (done + 1).min(*of);
            let sentence = match (stopping, current) {
                (true, _) => "Stopping after this one…".to_owned(),
                (false, Some(branch)) => format!("Removing {nth} of {of}: {branch}…"),
                (false, None) => format!("Removing {nth} of {of}…"),
            };
            column.add_child(busy_line(state, &sentence, Some((*done, *of)), ui));
            column.add_child(buttons(workspace, &format!("Remove {of}"), None, ui));
            return column.finish();
        }
        Sweep::Ready { going, kept } => (going, kept),
    };

    for going in going.iter().take(NAMED) {
        column.add_child(note(&going.label, ui));
    }
    if going.len() > NAMED {
        column.add_child(note(format!("and {} more.", going.len() - NAMED), ui));
    }

    // The seam between the branches and the sentences about them. Without it
    // the list runs straight into "The branches are kept" in the same size and
    // the same tone, and a person skimming cannot see where the names stop.
    column.add_child(divider());

    if going.is_empty() {
        // Which is a real answer and not an error: every free checkout has
        // something in it. The face still opens, because "nothing to do here"
        // is what the person asked to be told.
        column.add_child(note(
            match kept.len() {
                1 => "There is work in it.",
                _ => "There is work in every one of them.",
            },
            ui,
        ));
    } else {
        column.add_child(note(
            match going.len() {
                1 => "The branch is kept. Only the checkout goes.",
                _ => "The branches are kept. Only the checkouts go.",
            },
            ui,
        ));
    }

    // What git will delete without ever mentioning it, summed over the lot:
    // the `target/` in each of six finished checkouts is the real cost of
    // this button, and nothing else on screen would say so.
    let ignored: usize = going.iter().map(|going| going.local.ignored).sum();
    if ignored > 0 {
        let here = if going.len() == 1 { "it" } else { "them" };
        column.add_child(note(
            format!("{ignored} ignored in {here} will be deleted."),
            ui,
        ));
    }

    if !kept.is_empty() {
        column.add_child(note(format!("Left alone: {}.", named(kept)), ui));
    }

    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }

    column.add_child(buttons(
        workspace,
        &match going.len() {
            0 => "Remove".to_owned(),
            count => format!("Remove {count}"),
        },
        (!going.is_empty()).then_some(WorktreeAction::Tidy),
        ui,
    ));
    column.finish()
}

/// Some branches in a line, with the ones past [`NAMED`] counted instead.
fn named(branches: &[String]) -> String {
    let mut sentence = branches
        .iter()
        .take(NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if branches.len() > NAMED {
        sentence.push_str(&format!(" and {} more", branches.len() - NAMED));
    }
    sentence
}

/// Removing one, which is the only destructive thing in the menu and is
/// therefore the only thing that asks twice.
fn confirmation(
    workspace: &Workspace,
    index: usize,
    local: Looked,
    refused: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let worktree = state.worktrees().get(index);
    let path = worktree
        .map(|worktree| worktree.path.clone())
        .unwrap_or_default();
    let on_a_branch = worktree.is_some_and(|worktree| worktree.branch.is_some());

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header("Remove this checkout?", ui))
        .with_child(note(
            crate::git::user_friendly_path(&path, workspace.home()),
            ui,
        ))
        // The sentence that stops this reading as "delete my branch". git does
        // not delete it and neither does this, and somebody about to press a
        // destructive button should not have to know git to know that. A
        // checkout with no branch has none to keep, and saying it did would be
        // the one reassurance here that was false.
        .with_child(note(
            match on_a_branch {
                true => "The branch is kept. Only the checkout goes.",
                false => "It is on no branch, so nothing keeps what it has committed.",
            },
            ui,
        ));

    match local {
        // The count is one `git status`, and on a checkout with a fat build
        // directory it is the first thing here that takes long enough to see.
        Looked::NotYet if !state.working => {
            column.add_child(busy_line(state, "Looking in it…", None, ui));
        }
        Looked::Found(local) if !local.is_empty() => {
            if local.stranded > 0 {
                column.add_child(note(stranded_summary(local.stranded), ui));
            }
            if local.modified + local.untracked + local.ignored > 0 {
                column.add_child(note(local_summary(&local), ui));
            }
        }
        Looked::NotYet | Looked::Found(_) | Looked::Unknown => {}
    }

    // The other two faces show what git said; this one showed nothing, so
    // every refusal but "there is work in there" — a locked checkout, a path
    // that is not a worktree, git timing out — took the button press and left
    // the dialog exactly as it was.
    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }
    if state.working {
        column.add_child(busy_line(state, "Removing it…", None, ui));
    }

    let label = if refused { "Remove anyway" } else { "Remove" };
    column.add_child(buttons(
        workspace,
        label,
        Some(WorktreeAction::Remove { force: refused }),
        ui,
    ));
    column.finish()
}

/// The commits a removal would lose, in one line.
///
/// git removes over them without refusing, so this line is the only warning
/// there is: after the press, nothing but `git fsck` can find them.
fn stranded_summary(stranded: usize) -> String {
    match stranded {
        1 => String::from("1 commit on no branch will be lost."),
        many => format!("{many} commits on no branch will be lost."),
    }
}

/// What is in a checkout, in one line.
fn local_summary(local: &Local) -> String {
    let mut parts = Vec::new();
    if local.modified > 0 {
        parts.push(format!("{} modified", local.modified));
    }
    if local.untracked > 0 {
        parts.push(format!("{} untracked", local.untracked));
    }
    if local.ignored > 0 {
        parts.push(format!("{} ignored", local.ignored));
    }
    format!("{} in it will be deleted.", parts.join(", "))
}

/// Cancel, and the button that does the thing.
///
/// `action` is `None` for a face whose button has nothing to do yet — one
/// still counting, or one that found nothing it may remove. It is drawn
/// without a click handler rather than with one that returns early, which is
/// the rule every other disabled control in this application follows — and
/// the rule this pair used to break while git was running: the button said
/// "Working…" in the accent, answered the pointer, and did nothing when
/// pressed. It is inert now, in the quiet ground, and the line above it is
/// what says something is happening.
///
/// Cancel is the one control that stays live through a wait, because it is
/// the way out of one. During a sweep it is *Stop*, which is Cancel with the
/// one difference the face cannot hide: the checkout under the knife
/// finishes going, and it is the ones after it that are spared.
fn buttons(
    workspace: &Workspace,
    label: &str,
    action: Option<WorktreeAction>,
    ui: FamilyId,
) -> Box<dyn Element> {
    button_row(workspace, None, label, action, ui)
}

/// [`buttons`], with a second way to do the thing between the two: the
/// creator's Start, which is Create and then the agent's line run rather than
/// left in the composer. A button of the quiet kind, since it is not the one
/// Enter presses; inert while git is working, like the one beside it.
fn button_row(
    workspace: &Workspace,
    middle: Option<(&str, Option<WorktreeAction>)>,
    label: &str,
    action: Option<WorktreeAction>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    let (cancel, cancels) = match &state.sweep {
        Sweep::Removing { stopping: true, .. } if state.mode == Mode::Tidying => {
            ("Stopping…", None)
        }
        Sweep::Removing { .. } if state.mode == Mode::Tidying => {
            ("Stop", Some(WorktreeAction::Cancel))
        }
        _ => ("Cancel", Some(WorktreeAction::Cancel)),
    };
    let action = action.filter(|_| !state.working);

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.)
        .with_child(
            Expanded::new(
                1.,
                button(
                    state.control(Control::Cancel),
                    cancel,
                    false,
                    cancels.map(WorkspaceAction::Worktree),
                    ui,
                ),
            )
            .finish(),
        );
    if let Some((middle, pressed)) = middle {
        let pressed = pressed.filter(|_| !state.working);
        row.add_child(
            Expanded::new(
                1.,
                button(
                    state.control(Control::Start),
                    middle,
                    false,
                    pressed.map(WorkspaceAction::Worktree),
                    ui,
                ),
            )
            .finish(),
        );
    }
    row.add_child(
        Expanded::new(
            1.,
            button(
                state.control(Control::Confirm),
                label,
                true,
                action.map(WorkspaceAction::Worktree),
                ui,
            ),
        )
        .finish(),
    );

    Container::new(row.finish())
        .with_horizontal_padding(ROW_INSET)
        .with_margin_top(10.)
        .finish()
}

/// One of a question's buttons.
///
/// Also the face of the question a close asks before it ends agents that
/// are still working — see [`closing`](super::closing) — which is why it
/// takes any action rather than the worktree menu's own: two questions that
/// looked different would be two things a person has to learn to read.
pub(super) fn button(
    state: MouseStateHandle,
    label: &str,
    primary: bool,
    action: Option<WorkspaceAction>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = label.to_owned();
    let inert = action.is_none();

    let control = Hoverable::new(state, move |mouse| {
        // An inert button takes the quiet ground rather than the accent, and
        // its own hover does nothing: a filled primary button that answers the
        // pointer and not the press is a control that looks broken.
        let (background, border) = match (primary && !inert, mouse.is_hovered() && !inert) {
            (true, false) => (theme().accent, theme().accent),
            (true, true) => (theme().accent, theme().text_primary),
            (false, false) => (Color::TRANSPARENT, theme().border),
            (false, true) => (theme().overlay_2, theme().border),
        };

        Container::new(
            Align::new(
                Text::new(label.clone(), ui, 11.)
                    .with_color(if inert {
                        theme().text_muted
                    } else {
                        theme().text_primary
                    })
                    .finish(),
            )
            .finish(),
        )
        .with_padding(Padding {
            top: 6.,
            bottom: 6.,
            left: 8.,
            right: 8.,
        })
        .with_background_color(background)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
        .finish()
    });

    match action {
        Some(action) => control
            .on_click(move |_, ctx, _| {
                ctx.dispatch_typed_action(action);
            })
            .finish(),
        None => control.finish(),
    }
}

/// Which of `worktrees` a directory sits in.
///
/// The *longest* path that contains it, which is the whole of the rule and is
/// not pedantry: a linked worktree is very often inside the main checkout —
/// Crook's own agent worktrees are — so a directory in it is under two of
/// these paths and only the longer one is true.
pub(super) fn holding(worktrees: &[Worktree], directory: Option<&Path>) -> Option<usize> {
    let directory = directory?;
    worktrees
        .iter()
        .enumerate()
        .filter(|(_, worktree)| directory.starts_with(&worktree.path))
        .max_by_key(|(_, worktree)| worktree.path.as_os_str().len())
        .map(|(index, _)| index)
}

/// Where the worktree being typed would go, once there is a name for it.
///
/// Takes the workspace rather than the menu's state, because the name is being
/// typed into a field that belongs to `crook/worktrees` and the workspace is
/// what can find it.
pub(super) fn checkout_for(workspace: &Workspace) -> Option<PathBuf> {
    let state = workspace.tab_menu();
    let store = state.store.as_deref()?;
    let repository = state.repository.as_deref()?;
    let branch = workspace
        .worktree_branch()?
        .editor()
        .text()
        .trim()
        .to_owned();

    (!branch.is_empty()).then(|| crate::git::worktree::checkout_path(store, repository, &branch))
}

/// What a worktree is called: its branch, or the head it is sitting on.
fn branch_label(worktree: &Worktree) -> String {
    match (&worktree.branch, &worktree.head) {
        (Some(branch), _) => branch.clone(),
        (None, Some(head)) => format!("detached at {head}"),
        (None, None) => "bare".to_owned(),
    }
}

/// The line at the top of whichever face the menu is showing.
pub(super) fn header(
    title: impl Into<std::borrow::Cow<'static, str>>,
    ui: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Text::new(title, ui, PATH_SIZE)
            .with_color(theme().text_muted)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Properties::default()
            })
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .with_margin_top(2.)
    .finish()
}

/// How big the pirate is drawn in the popup: a little over the 10.5pt line
/// beside him. The artwork is a disc that fills its box edge to edge, where a
/// glyph of the same nominal size sits inside its ascent and descent, so a
/// pirate at the label size would tower over the label.
const PIRATE_SIZE: f32 = 13.;

/// A pellet's diameter, and the distance from one to the next.
const PELLET: f32 = 4.;
/// See [`PELLET`].
const PELLET_PITCH: f32 = 9.;

/// How many pellets a trail draws at most.
///
/// Sixteen fit beside the pirate inside the popup's width with room to spare;
/// past that the trail is scaled, so that a sweep over forty checkouts is
/// still a pirate moving along a row rather than a row running off the edge.
const PELLETS: usize = 16;

/// A wait, drawn as the pirate chewing through it.
///
/// `trail` is `(done, of)` where the wait is a list of checkouts: he stands
/// after the ones dealt with, which are gone, and before the ones still to
/// come, which are pellets. Without it he chews beside the sentence, which is
/// the same sign a spinner would be with a face on it.
///
/// The frame is [`TabMenuState::chomp`], which the workspace's chain moves
/// while [`TabMenuState::is_busy`]; this only draws whichever frame that is.
fn busy_line(
    state: &TabMenuState,
    text: &str,
    trail: Option<(usize, usize)>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let pirate = crate::pirate::mark(crate::pirate::chomp_at(state.chomp), false, PIRATE_SIZE);
    // Wrapped to the popup's width the way a note is, because the sentence
    // names a branch and a branch can be long.
    let sentence = Paragraph::new(text.to_owned(), ui, PATH_SIZE)
        .with_color(theme().text_muted)
        .with_line_height_ratio(1.4)
        .finish();

    let body: Box<dyn Element> = match trail {
        // Beside the pirate, in the room he leaves: a row measures a plain
        // child free along its axis, and a paragraph measured free is one
        // line however long, so the sentence takes its share as a flexible
        // child and wraps inside it.
        None => Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.)
            .with_child(pirate)
            .with_child(Shrinkable::new(1., sentence).finish())
            .finish(),
        Some((done, of)) => Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(trail_row(pirate, done, of))
            .with_child(Container::new(sentence).with_margin_top(3.).finish())
            .finish(),
    };

    Container::new(body)
        .with_horizontal_padding(ROW_INSET)
        .with_margin_bottom(4.)
        .finish()
}

/// The pirate at his place along a row of `of` pellets, `done` of which he
/// has eaten.
///
/// The eaten ones are left as faint places rather than dropped, so he moves
/// right along a row that stays where it was — a row that shrank from the left
/// would have him standing still while the pellets came to him — and so the
/// row is as long as the list from the first frame to the last, which is what
/// lets "how far" be read off it at all.
fn trail_row(pirate: Box<dyn Element>, done: usize, of: usize) -> Box<dyn Element> {
    let shown = of.min(PELLETS);
    // Scaled when there are more than fit, and never shown as finished until
    // it is: `done * shown / of` rounds down, so the last pellet stays ahead
    // of him until the last checkout has gone.
    let eaten = if of == 0 { 0 } else { done * shown / of };

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);
    for _ in 0..eaten {
        row.add_child(pellet(theme().border));
    }
    row.add_child(pirate);
    for _ in eaten..shown {
        row.add_child(pellet(theme().text_muted));
    }
    row.finish()
}

/// One pellet, in its pitch.
fn pellet(color: Color) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PELLET / 2.)))
                .finish(),
        )
        .with_width(PELLET)
        .with_height(PELLET)
        .finish(),
    )
    .with_horizontal_padding((PELLET_PITCH - PELLET) / 2.)
    .finish()
}

/// A line of explanation, or of apology, wrapped to the popup's width.
pub(super) fn note(text: impl AsRef<str>, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text.as_ref().to_owned(), ui, PATH_SIZE)
            .with_color(theme().text_muted)
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}

/// The hairline between the worktrees and the way to another.
pub(super) fn divider() -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.)
            .finish(),
    )
    .with_background_color(theme().border)
    .with_margin_top(4.)
    .with_margin_bottom(4.)
    .finish()
}
