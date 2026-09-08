//! What the header can be asked to do.
//!
//! A view handles exactly one action type — dispatch keys on the action's
//! `TypeId` and stops at the first view registered for it — so the workspace's
//! own vocabulary has to be one enum. It is *this* enum rather than
//! [`TabAction`], and that is the point: [`TabAction`] stays the strip's
//! vocabulary, tested with no window, and `TabStrip::apply` never has to grow
//! an arm for a menu it does not know exists. Warp draws the same line in the
//! other direction, with one `WorkspaceAction` that carries tab actions and
//! vertical-tab display options side by side.

use crook_terminal::BlockId;

use crate::plugin::{ActionId, PageId, SectionId};
use crate::settings::{Density, Granularity, PrimaryInfo, Subtitle};
use crate::tab::{PaneId, TabAction, TabId};

/// Everything the header dispatches.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WorkspaceAction {
    /// Something happened to the strip. Applied by `TabStrip::apply`.
    Tab(TabAction),
    /// Something happened in the options menu, or on the settings page, which
    /// writes the same options through the same path.
    Options(OptionsAction),
    /// Something happened to the settings page itself.
    Settings(SettingsAction),
    /// Something happened in the Themes panel.
    Theme(ThemeAction),
    /// The header was used as what it is: the window's title bar.
    Window(WindowAction),
    /// Something happened to the context menu a tab's secondary press opens.
    TabMenu(TabMenuAction),
    /// Something happened in the worktree menu, which is one entry of that one.
    Worktree(WorktreeAction),
    /// Something happened in the menu a block opens, which is about that
    /// block.
    Block(BlockAction),
    /// The pointer entered a row, or left it.
    ///
    /// Carried as an action rather than written directly, because a hover
    /// handler runs while the element tree is being walked and holds no
    /// `&mut Workspace`. It names the row in both directions on purpose: an
    /// enter and a leave dispatched in one pass — the pointer crossing from one
    /// row to the next — then settle on the row the pointer is actually over,
    /// whichever order they arrive in.
    HoverRow {
        /// The row the pointer crossed.
        pane: PaneId,
        /// Whether it arrived, rather than left.
        entered: bool,
    },
    /// Let go of what is selected in a pane's output.
    ///
    /// Carried as an action for a reason the grid could not solve on its own.
    /// A pane's grid and the field under it are siblings, and a `Flex` hands
    /// the same keystroke to both of them — the grid first. Both route it
    /// through [`crate::input_keys::route`], and both ask the same question to
    /// do it: is anything selected in the output? A grid that released the
    /// selection while it copied would answer the field's question differently
    /// from its own a moment later, and the field would then take `ctrl-c` for
    /// the interrupt it is with nothing selected and throw away the
    /// half-written command line with it.
    ///
    /// Actions are applied once the whole tree has seen the event, so the two
    /// elements route against the same answer and the release lands after both
    /// of them. **Nothing may change what is selected in the output while a
    /// keystroke is being dispatched.**
    ReleaseSelection(PaneId),
    /// Something happened to the search box above the tabs.
    Search(SearchAction),
    /// Something happened to the find bar over a pane's output.
    Find {
        /// Which pane's bar. A pane, not "the focused one", because a button
        /// on the bar names the pane it is drawn over and the answer must not
        /// depend on which pane the keyboard was in when it was clicked.
        pane: PaneId,
        /// What happened to it.
        action: FindAction,
    },
    /// Show one section of the sidebar, or `None` for the tab list.
    ///
    /// The sections come from a slot, so this carries a
    /// [`SectionId`](crate::plugin::SectionId) — interned, because the enum is
    /// `Copy` and a section is named by an `owner/entry` string.
    ShowSection(Option<SectionId>),
    /// Run a named action, which is what every plugin's is.
    ///
    /// Carried as an id rather than an [`ActionName`](crate::plugin::ActionName)
    /// because this enum is `Copy` and a name is a `String`; the host resolves
    /// one to the other. See [`ActionId`](crate::plugin::ActionId).
    Run(ActionId),
    /// The first half of a chord sequence was pressed, and the window is
    /// waiting for the rest of it.
    ///
    /// Nothing happens; the point of the action is that *something* is
    /// returned, because a keystroke the window has no action for goes on to
    /// the element under it and from there to the shell. A person spelling
    /// `ctrl+k ctrl+s` must not have the `ctrl+k` typed into their command
    /// line while they reach for the second half. See
    /// [`crate::keybindings`].
    Chord,
    /// Ask this pane's shell what the word before the caret could become.
    ///
    /// An action rather than a call, for the reason `ReleaseSelection` is one:
    /// the element that saw Tab holds the field but not the terminal *model*,
    /// and it is the model that knows where this session's scratch directory
    /// is. See [`crate::completion`].
    Complete(PaneId),
}

/// What the search box above the tabs does.
///
/// Three, and the shape of them is the whole difference between this box and
/// the settings rail's. That one is the only thing on its screen that takes a
/// key, so it needs no way to be left; this one shares its screen with a
/// shell, so two of the three are ways out of it — and both of them empty the
/// box on the way, because a filter left in force over the list a person
/// navigates by is a panel that has quietly lost most of its tabs.
///
/// See [`search`](super::tabs_panel::search).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SearchAction {
    /// Put the keyboard in the box: a press on it, or the chord.
    Focus,
    /// Empty it and hand the keyboard back to the pane. Escape.
    Dismiss,
    /// Select the top match, then dismiss. Enter.
    Accept,
}

impl From<SearchAction> for WorkspaceAction {
    fn from(action: SearchAction) -> Self {
        Self::Search(action)
    }
}

/// What a keystroke or a press does to a pane's find bar.
///
/// The shape of the search box's actions, one member longer: this box shares
/// its screen with a shell too, so [`Self::Close`] is a way out that empties
/// nothing — a query kept is a search a person comes back to — and the extra
/// member is the one thing the tab search has no equivalent of, stepping
/// between the several matches a query in a screenful of output turns up.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FindAction {
    /// Open the bar and put the keyboard in it. The chord, and a press on a
    /// bar that is already open.
    Open,
    /// Take the bar down and give the keyboard back to the shell. Escape.
    Close,
    /// Go to the next match, or the previous one. Enter and Shift-Enter, and
    /// the two buttons on the bar.
    Step {
        /// Forwards, rather than back.
        forward: bool,
    },
}

/// What the header does as a title bar.
///
/// A window is not a view, so none of these ends in the workspace's own state:
/// they are forwarded to
/// [`WindowControls`](crate::window_controls::WindowControls) and the answer
/// comes back as a window that has moved. They are actions all the same, for
/// the reason every other control's gesture is one — a handler runs while the
/// element tree is being walked and holds no `&mut Workspace` — and because it
/// is the only way a test with no window can watch a press turn into a drag.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WindowAction {
    /// Pick the window up and move it with the pointer.
    Drag,
    /// Fill the work area, or go back to the size before that.
    ToggleMaximized,
    /// Put the window wherever this desktop keeps minimised ones.
    Minimize,
    /// Close it, which for a one-window application is to quit.
    Close,
}

impl From<WindowAction> for WorkspaceAction {
    fn from(action: WindowAction) -> Self {
        Self::Window(action)
    }
}

/// What the context menu a tab's secondary press opens does.
///
/// Two, and there is deliberately nothing else in here. Every *entry* of that
/// menu is a named action belonging to whichever plugin contributed it, which
/// is what stops this enum growing an arm every time somebody adds a row — the
/// arrangement `WorkspaceAction::Run` exists for. What is left is the menu
/// itself: it is up, or it is not.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TabMenuAction {
    /// Open it on this tab, over this row. A secondary press on a row.
    ///
    /// The pane as well as the tab, because a row under `Panes` granularity
    /// stands for a pane and half the entries are about that pane rather than
    /// about its tab. Pressing on the row whose menu is already up closes it,
    /// which is what anything opened by being pressed does.
    Open {
        /// The tab the row belongs to.
        tab: TabId,
        /// The pane the row draws.
        pane: PaneId,
    },
    /// Take it down, and any submenu with it. What a press outside it sends.
    Close,
}

impl From<TabMenuAction> for WorkspaceAction {
    fn from(action: TabMenuAction) -> Self {
        Self::TabMenu(action)
    }
}

/// What the menu on a tab does.
///
/// A worktree is named by **its index in the list the menu is showing** rather
/// than by its path, and that is not laziness. This enum is `Copy` — a
/// `WorkspaceAction` is compared by value in a dozen places — and a `PathBuf`
/// would end that. The index is sound because the list is not live: it is read
/// once, when the menu opens, and while the menu is up its own modal underlay
/// is the only thing that can be clicked.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorktreeAction {
    /// Open the menu on this tab. What the active tab's own row dispatches.
    OpenMenu(TabId),
    /// Take it down. What a press outside it sends.
    CloseMenu,
    /// Show the worktree at this index: bring forward the tab already in it,
    /// or open one there.
    Show(usize),
    /// Begin making one.
    StartCreating,
    /// Make it, from what has been typed into the branch field.
    Create,
    /// Ask about removing the worktree at this index.
    AskRemove(usize),
    /// Remove it. `force` is the second answer, offered only once git has
    /// refused the first over work that is in there.
    Remove {
        /// Whether to delete a checkout with local work in it.
        force: bool,
    },
    /// Ask about removing every checkout that is free: not the main one, not
    /// locked, and nothing in the window working in it.
    AskTidy,
    /// Remove those of them git lets go without being forced, and leave the
    /// rest standing. There is no second question: this one never forces.
    Tidy,
    /// Back to the list, from the creator or from the confirmation.
    Cancel,
}

/// What the menu on a block does.
///
/// Every entry but the two that open and close it acts on **the block the menu
/// is up on**, which is why none of them names one. The alternative — a block
/// id in every variant — would say that an entry could be run against a block
/// nothing is showing a menu for, and there is no gesture that does that: a
/// menu is opened from a block's own dots and is taken down by anything else
/// that happens in the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockAction {
    /// Open the menu on this block of this pane's list.
    OpenMenu {
        /// Whose list it is.
        pane: PaneId,
        /// Which block of it.
        block: BlockId,
    },
    /// Take it down. What a press outside it sends, and Escape.
    CloseMenu,
    /// Put one of the block's facts on the clipboard.
    Copy(BlockPart),
    /// Bring one of the block's own edges to the edge of the pane.
    ScrollTo(BlockEdge),
    /// Put the block's command line back in the composer, unsent.
    Rerun,
}

/// Which of a block's facts an entry copies.
///
/// Warp's list, minus the ones Crook has nothing behind: there is no session to
/// share and no workflow to save one as. What is left is what the block itself
/// knows, and each of them is a thing somebody would otherwise select with the
/// pointer and hope they had not caught the prompt with it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockPart {
    /// The prompt, the command and everything it printed: what the copy
    /// square beside the menu takes.
    Whole,
    /// The command line alone.
    Command,
    /// What the command printed, without the prompt or the line it was typed
    /// on. Only a block whose shell reported the `C` mark has one.
    Output,
    /// Where the shell was when the block opened.
    Directory,
    /// The branch that directory was on when the menu opened.
    Branch,
}

/// Which edge of a block an entry brings into view.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockEdge {
    /// Its first row, at the top of the pane.
    Top,
    /// Its last row, at the bottom of the pane.
    Bottom,
}

impl From<BlockAction> for WorkspaceAction {
    fn from(action: BlockAction) -> Self {
        Self::Block(action)
    }
}

impl From<TabAction> for WorkspaceAction {
    fn from(action: TabAction) -> Self {
        Self::Tab(action)
    }
}

impl From<OptionsAction> for WorkspaceAction {
    fn from(action: OptionsAction) -> Self {
        Self::Options(action)
    }
}

impl From<SettingsAction> for WorkspaceAction {
    fn from(action: SettingsAction) -> Self {
        Self::Settings(action)
    }
}

impl From<WorktreeAction> for WorkspaceAction {
    fn from(action: WorktreeAction) -> Self {
        Self::Worktree(action)
    }
}

impl From<ThemeAction> for WorkspaceAction {
    fn from(action: ThemeAction) -> Self {
        Self::Theme(action)
    }
}

/// What the Themes panel does.
///
/// Its own vocabulary rather than more [`SettingsAction`] variants, because
/// the panel is not the settings page: it opens beside a running shell, it
/// outlives the page being closed, and its keyboard belongs to it while it is
/// up. What the two share is the *write* path — both end at
/// `Workspace::set_theme` — which is where sharing matters.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ThemeAction {
    /// Show the panel, or bring it back to its list if it is already showing.
    OpenPanel,
    /// Take it down. What the × sends, and Escape.
    ClosePanel,
    /// Apply and remember the theme at this index of the workspace's list.
    Choose(usize),
    /// Move the keyboard's selection by this many rows, and apply what it
    /// lands on.
    ///
    /// Warp's arrow keys do exactly this: they move *and* choose, so browsing
    /// with the keyboard is browsing the real thing rather than a list of
    /// names.
    MoveSelection(isize),
    /// Start making a theme from the one in force.
    StartCreating,
    /// Put back the theme that was in force, and go back to the list.
    CancelCreating,
    /// Use this candidate as the draft's background.
    PickBackground(usize),
    /// Write the draft into the themes folder and choose it.
    Create,
}

/// What the settings page does that is not writing an option.
///
/// The split is deliberate and it is the page's whole design: every control
/// that changes a tab option dispatches the [`OptionsAction`] the tab options
/// menu already dispatches, so the two surfaces cannot drift apart. What is
/// left is this — and none of it is "open" or "close": the settings are a
/// section of the sidebar, so showing them is [`WorkspaceAction::ShowSection`]
/// and leaving them is showing another one.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SettingsAction {
    /// Show a different page of the settings in the section already holding
    /// them.
    Select(PageId),
    /// Move the keyboard to one of the fields on the page, or back to the
    /// rail's search box.
    ///
    /// By index rather than by name, because this enum is `Copy`; see
    /// [`Workspace::field`](crate::workspace::Workspace::field).
    FocusField(Option<usize>),
    /// Put every tab option back to the value a fresh install opens with.
    ResetTabOptions,
    /// Set the terminal's type size, in logical pixels.
    ///
    /// A size rather than a step, so that the zoom chords, the settings page's
    /// buttons and its reset all send the same action. Whoever sends it has
    /// already asked [`GeneralOptions`](crate::settings::GeneralOptions) what
    /// the next size is, which is the one place the bounds are applied.
    SetFontSize(f32),
    /// "Follow the desktop": whether the theme tracks the system's light or
    /// dark setting.
    ToggleFollowSystemTheme,
    /// "Bring the tabs back": whether a window opens holding the tabs the last
    /// one had.
    ToggleRestoreSession,
    /// "Start a login shell": whether a pane's shell reads the startup files
    /// that only a login shell reads.
    ToggleLoginShell,
    /// Start recording a chord for this command, on the Keyboard Shortcuts
    /// page.
    ///
    /// The command is carried as an [`ActionId`] for the reason
    /// [`WorkspaceAction::Run`] carries one: this enum is `Copy` and a name is
    /// a `String`.
    ///
    /// While a recording is up the keyboard belongs to it — see
    /// [`Workspace::action_for`](crate::workspace::Workspace::action_for) —
    /// which is what lets a chord a pane would otherwise eat be recorded at
    /// all.
    RecordBinding(ActionId),
    /// A chord went into the recording.
    ///
    /// Nothing is carried: the keystroke was written down where it was seen,
    /// exactly as a half-typed chord sequence is. What the action is for is
    /// the repaint — the row is showing what has been pressed, and it has just
    /// changed.
    RecordedKey,
    /// Keep what has been recorded, and write it to the keybindings file.
    KeepBinding,
    /// Throw away what has been recorded and leave the binding as it was.
    StopRecording,
    /// Take every chord away from this command, giving the keys back to the
    /// pane.
    UnbindCommand(ActionId),
    /// Put this command back to the chord this build ships with.
    ResetBinding(ActionId),
}

/// What the options menu writes.
///
/// One variant per control, exactly as Warp has one `WorkspaceAction` variant
/// per control. Collapsing the four check-list rows into one renderer is what a
/// pre-built action buys: Warp needs four byte-identical row functions only
/// because each closes over a different enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OptionsAction {
    /// Open the menu, or close it. A secondary press on the empty space around
    /// the tab list sends this, and so does the dismiss underlay; only one of
    /// the two can be reached at a time.
    TogglePopup,
    /// "View as".
    SetGranularity(Granularity),
    /// "Density".
    SetDensity(Density),
    /// "Pane title as".
    SetPrimaryInfo(PrimaryInfo),
    /// "Additional metadata".
    SetSubtitle(Subtitle),
    /// "Show: PR link".
    ToggleShowPrLink,
    /// "Show: Diff stats".
    ToggleShowDiffStats,
    /// "Show details on hover".
    ToggleShowDetailsOnHover,
}
