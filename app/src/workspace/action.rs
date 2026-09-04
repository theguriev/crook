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

use crate::plugin::{ActionId, PageId};
use crate::settings::{Density, Granularity, Layout, PrimaryInfo, Subtitle};
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
    /// Something happened in the menu a tab opens, which is about worktrees.
    Worktree(WorktreeAction),
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
    /// Run a named action, which is what every plugin's is.
    ///
    /// Carried as an id rather than an [`ActionName`](crate::plugin::ActionName)
    /// because this enum is `Copy` and a name is a `String`; the host resolves
    /// one to the other. See [`ActionId`](crate::plugin::ActionId).
    Run(ActionId),
    /// Ask this pane's shell what the word before the caret could become.
    ///
    /// An action rather than a call, for the reason `ReleaseSelection` is one:
    /// the element that saw Tab holds the field but not the terminal *model*,
    /// and it is the model that knows where this session's scratch directory
    /// is. See [`crate::completion`].
    Complete(PaneId),
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
    /// Back to the list, from the creator or from the confirmation.
    Cancel,
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
/// that changes a tab option dispatches the [`OptionsAction`] the gear menu
/// already dispatches, so the two surfaces cannot drift apart. What is left is
/// this — three actions, none of which is "open" or "close": the page is a
/// pane, so opening it is [`TabAction::OpenSettings`] and closing it is
/// closing a pane, through the same close button, middle click and close
/// chord — `cmd-w`, `ctrl-shift-w` off macOS — as every other pane in the
/// window.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SettingsAction {
    /// Show a different page of the settings in the pane already holding
    /// them.
    Select(PageId),
    /// Move the keyboard to one of the fields on the page, or back to the
    /// rail's search box.
    ///
    /// By index rather than by name, because this enum is `Copy`; see
    /// [`SettingsState::field`](crate::workspace::settings_page::SettingsState::field).
    FocusField(Option<usize>),
    /// "Show the usage chip", which is also what starts and stops the poll.
    ToggleUsageChip,
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
}

/// What the options menu writes.
///
/// One variant per control, exactly as Warp has one `WorkspaceAction` variant
/// per control. Collapsing the four check-list rows into one renderer is what a
/// pre-built action buys: Warp needs four byte-identical row functions only
/// because each closes over a different enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OptionsAction {
    /// Open the menu, or close it. The gear button and the dismiss underlay
    /// both send this, and only one of them can be reached at a time.
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
    /// Move the tabs between the panel and the strip.
    ///
    /// A toggle rather than a `SetLayout`, because what sends it is a
    /// keystroke that means "the other one". The gear menu has no control for
    /// it — Warp keeps `use_vertical_tabs` in its settings window rather than
    /// in this popup, and a popup that could move itself out from under the
    /// pointer is a worse place for it.
    ToggleLayout,
    /// Put the tabs somewhere by name.
    ///
    /// What the settings page's segmented control sends. A named value rather
    /// than a toggle because a segmented control has two halves and clicking
    /// the selected one must do nothing: a toggle there would swap the layout
    /// under a pointer that had just been told it was already on the right
    /// half.
    SetLayout(Layout),
}
