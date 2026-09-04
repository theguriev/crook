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
//! * the repository's worktrees, each of which opens a pane in it *inside the
//!   tab the menu was opened on* — or brings forward the pane already there;
//! * a way to make one, which asks for a branch name and nothing else;
//! * a way to remove one, offered only for a checkout nothing is working in.
//!
//! The branches of one repository stay together because of that second half.
//! A tab is the repository and its panes are its checkouts: the panel draws
//! them under one group header — which appears by itself the moment a tab
//! holds a second pane, so there is no group to make first and none to tidy
//! away when one of them closes — and the body shows them side by side. A tab
//! of its own for each checkout would file the branch away from the work it
//! came out of, with nothing left in the list to say the two were related.
//!
//! herdr's shape, which is the tool this was modelled on: there a worktree is
//! not a thing you administer but a workspace with a git checkout behind it,
//! and creating one *opens* it. What is deliberately not taken is its
//! `--base`: a worktree made from anything other than the head you are looking
//! at is a question a menu cannot ask well.
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
//! * **Enter** is Create, in the creator only. The name is selected the moment
//!   the creator opens, so making a worktree is a name and a press.
//!
//! Enter does nothing in the confirmation. Removing a checkout is the only
//! destructive thing here, and it is not a thing to hand to the key beside
//! the one that dismisses dialogs.
//!
//! # Reading git off the frame
//!
//! Every git call here happens on the background pool and lands through
//! `ctx.spawn`, never on the render path — the rule the whole git layer is
//! built on. The menu therefore opens *before* it knows what is in the
//! repository and says so, the way a list that is being read says so.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::worktree::{Local, Worktree};
use crate::tab::TabId;
use crate::text_input::TextInput;
use crate::theme::theme;

use super::action::{WorkspaceAction, WorktreeAction};
use super::view::Workspace;

/// How wide the popup is.
///
/// Wider than the options menu's 200, because a worktree row carries a path
/// and a path is the one thing here that cannot be shortened without losing
/// what it is for.
pub(super) const MENU_WIDTH: f32 = 260.;

/// The inset around every row.
const ROW_INSET: f32 = 12.;

/// The popup's corner radius, which is the options menu's.
const MENU_RADIUS: f32 = 6.;

/// The size of the label on a row, and of the line under it.
const LABEL_SIZE: f32 = 12.;
const PATH_SIZE: f32 = 10.5;

/// How many characters of a path a row shows before it is cut from the left.
///
/// Counted rather than measured, for the reason every other budget in this
/// application is: measuring needs the shaper, and this runs while the element
/// tree is being built.
const PATH_CHARS: usize = 34;

/// What the branch field says before anything is typed into it.
const BRANCH_PLACEHOLDER: &str = "branch";

/// What the menu is doing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    /// Listing the repository's worktrees.
    #[default]
    Listing,
    /// Making one.
    Creating,
    /// Asking about removing the one at this index, and saying what is in it.
    ///
    /// `local` is `None` until the count comes back, and `refused` is set once
    /// git has declined over local work — which is what turns the button into
    /// the one that deletes it anyway.
    Removing {
        /// Which worktree, by its index in [`TabMenuState::worktrees`].
        index: usize,
        /// What is in it, once counted.
        local: Option<Local>,
        /// Whether git has already refused once.
        refused: bool,
    },
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
    /// The creator's branch field.
    Branch,
    /// The creator's and the confirmation's "Cancel".
    Cancel,
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
    /// What the repository is called: the name of its main checkout's
    /// directory, which is what a person calls it.
    pub(super) repository: Option<String>,
    /// Where Crook keeps checkouts it made.
    pub(super) store: Option<PathBuf>,
    /// The branch name being typed, while one is.
    pub(super) branch: TextInput,
    /// What git said about the last thing that was asked of it, if it refused.
    pub(super) problem: Option<String>,
    /// Whether a git command is running for this menu right now.
    pub(super) working: bool,
    /// One mouse state per control, made on the control's first frame.
    controls: std::cell::RefCell<HashMap<Control, MouseStateHandle>>,
}

impl TabMenuState {
    /// Whether the menu is up.
    pub(super) fn is_open(&self) -> bool {
        self.tab.is_some()
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

/// The whole popup.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let ui = workspace.fonts().ui;

    let body = match state.mode {
        Mode::Listing => listing(workspace, ui),
        Mode::Creating => creator(workspace, ui),
        Mode::Removing {
            index,
            local,
            refused,
        } => confirmation(workspace, index, local.as_ref(), refused, ui),
    };

    ConstrainedBox::new(
        Container::new(body)
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
            .with_vertical_padding(6.)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish()
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
        Contents::Reading => column.add_child(note("Reading the repository…", ui)),
        Contents::Failed(problem) => column.add_child(note(problem.as_str(), ui)),
        Contents::Ready(worktrees) => {
            for (index, worktree) in worktrees.iter().enumerate() {
                column.add_child(worktree_row(workspace, index, worktree, ui));
            }
        }
    }

    column.add_child(divider());
    column.add_child(create_row(workspace, ui));
    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }

    column.finish()
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
    // Never the main checkout, never one somebody is working in, and never a
    // locked one. git refuses all three, and an × that always fails is worse
    // than no × at all — the last of them especially, because Crook's own
    // agent worktrees are locked by the session that holds them, and that lock
    // is exactly what stops one agent tidying away another's work.
    let removable = !worktree.is_main && !here && !elsewhere && worktree.locked.is_none();

    let label = branch_label(worktree);
    let path = crate::git::user_friendly_path(&worktree.path, workspace.home());
    let path = crate::git::truncate_start(&path, PATH_CHARS);
    let badge = if here {
        Some("this tab")
    } else if elsewhere {
        Some("open")
    } else if worktree.locked.is_some() {
        Some("locked")
    } else if worktree.prunable.is_some() {
        // Registered, and its directory is gone. Saying so is the difference
        // between a row that looks broken and one that explains itself.
        Some("missing")
    } else {
        None
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
    Hoverable::new(state.control(Control::Worktree(index)), move |mouse| {
        let hovered = mouse.is_hovered();

        let mut line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(label.clone(), ui, LABEL_SIZE)
                    .with_color(theme().text_primary)
                    .with_style(if here {
                        Properties {
                            weight: Weight::Semibold,
                            ..Properties::default()
                        }
                    } else {
                        Properties::default()
                    })
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish());

        // The badge and the × share the right edge, and only one of them can
        // ever be there: a checkout somebody is working in is exactly the one
        // that must not be removable.
        if let Some(badge) = badge {
            line.add_child(
                Text::new(badge, ui, PATH_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            );
        } else if removable && hovered {
            line.add_child(remove_button(remove.clone(), index));
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

/// Making one: a branch name, and where it would go.
///
/// One field, because there is one question. herdr asks the same one and
/// derives the directory rather than asking for it, which is right: a person
/// choosing a branch name has said everything that distinguishes one worktree
/// from another, and a directory they have to invent as well is a second
/// chance to get it wrong.
fn creator(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.tab_menu();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header("New worktree", ui))
        .with_child(
            Container::new(
                super::text_field::TextField::new(
                    state.branch.clone(),
                    workspace.clipboard().clone(),
                    workspace.fonts(),
                    state.control(Control::Branch),
                    BRANCH_PLACEHOLDER,
                )
                .finish(),
            )
            .with_horizontal_padding(ROW_INSET)
            .finish(),
        );

    // The path is shown rather than asked for, and it is shown *live*: it is
    // the answer to "where will this end up", which is a question about the
    // name being typed.
    if let Some(checkout) = checkout_for(state) {
        let path = crate::git::user_friendly_path(&checkout, workspace.home());
        column.add_child(
            Container::new(
                Text::new(crate::git::truncate_start(&path, PATH_CHARS), ui, PATH_SIZE)
                    .with_color(theme().text_muted)
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

    column.add_child(buttons(workspace, "Create", WorktreeAction::Create, ui));
    column.finish()
}

/// Removing one, which is the only destructive thing in the menu and is
/// therefore the only thing that asks twice.
fn confirmation(
    workspace: &Workspace,
    index: usize,
    local: Option<&Local>,
    refused: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let path = state
        .worktrees()
        .get(index)
        .map(|worktree| worktree.path.clone())
        .unwrap_or_default();

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
        // destructive button should not have to know git to know that.
        .with_child(note("The branch is kept. Only the checkout goes.", ui));

    if let Some(local) = local.filter(|local| !local.is_empty()) {
        column.add_child(note(local_summary(local), ui));
    }

    // The other two faces show what git said; this one showed nothing, so
    // every refusal but "there is work in there" — a locked checkout, a path
    // that is not a worktree, git timing out — took the button press and left
    // the dialog exactly as it was.
    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }

    let label = if refused { "Remove anyway" } else { "Remove" };
    column.add_child(buttons(
        workspace,
        label,
        WorktreeAction::Remove { force: refused },
        ui,
    ));
    column.finish()
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
fn buttons(
    workspace: &Workspace,
    label: &'static str,
    action: WorktreeAction,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let working = state.working;

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Expanded::new(
                    1.,
                    button(
                        state.control(Control::Cancel),
                        "Cancel",
                        false,
                        WorktreeAction::Cancel,
                        ui,
                    ),
                )
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.,
                    button(
                        state.control(Control::Confirm),
                        if working { "Working…" } else { label },
                        true,
                        action,
                        ui,
                    ),
                )
                .finish(),
            )
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_top(10.)
    .finish()
}

/// One of the two buttons.
fn button(
    state: MouseStateHandle,
    label: &str,
    primary: bool,
    action: WorktreeAction,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = label.to_owned();

    Hoverable::new(state, move |mouse| {
        let (background, border) = match (primary, mouse.is_hovered()) {
            (true, false) => (theme().accent, theme().accent),
            (true, true) => (theme().accent, theme().text_primary),
            (false, false) => (Color::TRANSPARENT, theme().border),
            (false, true) => (theme().overlay_2, theme().border),
        };

        Container::new(
            Align::new(
                Text::new(label.clone(), ui, 11.)
                    .with_color(theme().text_primary)
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
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Worktree(action)))
    .finish()
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
pub(super) fn checkout_for(state: &TabMenuState) -> Option<PathBuf> {
    let store = state.store.as_deref()?;
    let repository = state.repository.as_deref()?;
    let branch = state.branch.editor().text().trim().to_owned();

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
fn header(title: impl Into<std::borrow::Cow<'static, str>>, ui: FamilyId) -> Box<dyn Element> {
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

/// A line of explanation, or of apology, broken to the popup's width.
fn note(text: impl AsRef<str>, ui: FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start);
    for line in super::wrap(text.as_ref(), 36) {
        column.add_child(
            Text::new(line, ui, PATH_SIZE)
                .with_color(theme().text_muted)
                .with_line_height_ratio(1.4)
                .finish(),
        );
    }

    Container::new(column.finish())
        .with_horizontal_padding(ROW_INSET)
        .with_margin_bottom(4.)
        .finish()
}

/// The hairline between the worktrees and the way to another.
fn divider() -> Box<dyn Element> {
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
