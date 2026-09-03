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

use crate::settings::{Density, Granularity, PrimaryInfo, Subtitle};
use crate::tab::{PaneId, TabAction};

/// Everything the header dispatches.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Something happened to the strip. Applied by `TabStrip::apply`.
    Tab(TabAction),
    /// Something happened in the options menu.
    Options(OptionsAction),
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
    /// A toggle rather than a `SetLayout(Layout)`, because there are exactly
    /// two layouts and the only thing that sends this is a keystroke that
    /// means "the other one". The menu has no control for it: Warp keeps
    /// `use_vertical_tabs` in its settings window rather than in this popup,
    /// and a popup that could move itself out from under the pointer is a
    /// worse place for it.
    ToggleLayout,
}
