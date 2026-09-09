//! The menu a block opens: everything that can be done to one command.
//!
//! # The gesture
//!
//! Three dots at the top right of the block the pointer is on, beside the
//! square that copies it. Warp's arrangement, and the reason for two controls
//! rather than one is Warp's too: copying a block is the thing people do over
//! and over, so it keeps a click of its own, and everything else is a list
//! they read.
//!
//! It is not the pane's context menu. A secondary click in the output belongs
//! to the shell — a program that reads the mouse is drawn on the grid, where
//! the press is forwarded to it — and a menu that opened there would take a
//! button away from every full-screen program a person runs. The dots are a
//! control of Crook's own, on a surface Crook owns, and they appear on exactly
//! the block they are about.
//!
//! # What is in it comes from a slot
//!
//! `block.menu`, declared by [`crook/blocks`](crate::plugins::blocks) and
//! contributed to by anything — which is what makes this menu the place a
//! plugin can put something that is *about one command*. Nothing else in the
//! window is: the header is one chip, a settings page is a page, and neither
//! of them knows what a person is looking at.
//!
//! **The unit of the slot is a group, not a row.** One contribution builds a
//! column of entries and the menu draws a hairline between contributions, so a
//! plugin's entries arrive together, under a rule, in the order the slot puts
//! them — rather than interleaved with Crook's own, where they would read as
//! things the terminal does. Warp's menu has the same shape and so does every
//! menu VS Code renders out of `menus` groups.
//!
//! Crook's own four groups are `crook/blocks`' contributions to its own slot,
//! which is the arrangement `crook/header` and `crook/usage` proved: the plugin
//! that owns a surface declares the slot, and its own content goes in through
//! the same door a stranger's would. Eight entries in all — what the block's
//! text can be copied as, what is known *about* the block, running it again,
//! and the two ways of moving the list to it. Warp's menu is longer, and what
//! is missing is missing because there is nothing behind it yet: no session to
//! share, no workflow to save a command as, no bookmark, no per-block filter
//! and no find-within-block. Adding a row that opened a dialogue saying "not
//! yet" would be worse than the row not being there.
//!
//! A menu with nothing in it never opens, and the dots that would open it are
//! not drawn: disabling `crook/blocks` takes the control off the block with the
//! menu behind it, which is what a plugin being switched off has to mean.
//!
//! **A row that cannot act is drawn and disabled rather than dropped.** A
//! block that ran in a directory nothing reported has no working directory to
//! copy, and a block whose shell never sent the `C` mark cannot say which of
//! its rows are output. Both are common enough — an `ssh`, a container, a
//! shell with no integration — that a menu which changed length between blocks
//! would be a menu whose rows move under the pointer. Warp greys the same rows
//! for the same reason.
//!
//! # The two rules a popup only half-works without
//!
//! They are [`tab_options_menu`](super::tab_options_menu)'s, and they are
//! written out there: the root is a [`Container`] with a background, because a
//! container is the only element that records a hit rect; and the popup is
//! painted one layer above its own [`Dismiss`](crookui_core::elements::Dismiss)
//! underlay, so a press on its padding is covered rather than a dismissal.
//!
//! Where this menu differs is what a click does: **every entry closes it.**
//! The options menu is a panel of preferences somebody changes three of at a
//! time; this is a list of things to do, each done once.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::theme::theme;

use crate::plugins::blocks::BLOCK_MENU;

use super::action::{BlockAction, BlockEdge, BlockPart, WorkspaceAction};
use super::block_list::{CONTROL_INSET, CONTROL_OFFSET, CONTROL_SIZE};
use super::view::Workspace;

/// How wide the popup is.
///
/// The options menu's 200 would cut "Scroll to bottom of block" in half, and a
/// menu whose longest row is its whole width has no margin for a theme whose
/// interface font runs wider. This is the worktree menu's 260, which is the
/// other width the application already uses.
const MENU_WIDTH: f32 = 260.;

/// The popup's corner radius, which is every other popup's.
const MENU_RADIUS: f32 = 6.;

/// The size a row's chord is printed at.
///
/// Smaller than the label, for the reason the tab menu's is: the label is what
/// somebody came to read and the chord is the answer to a question they have
/// not asked yet.
const CHORD_SIZE: f32 = 10.5;

/// The inset around every row. Dividers are full-bleed and are not inset,
/// which is the one thing easiest to get backwards.
const ROW_INSET: f32 = 12.;

/// The size of a row's label.
const LABEL_SIZE: f32 = 12.;

/// The space above and below a row's label.
const ROW_PADDING: f32 = 5.;

/// The whole popup: every group in the slot, with a hairline between them.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for (index, group) in workspace
        .host()
        .slots()
        .map(BLOCK_MENU, |build| build(workspace, app))
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            column.add_child(divider());
        }
        column.add_child(group);
    }

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_vertical_padding(6.)
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish()
}

/// The gap between the dots and the menu hanging off them.
///
/// The same four pixels the menu on a tab leaves under its row.
const MENU_GAP: f32 = 4.;

/// Where the popup hangs: its top-right corner on the bottom-right corner of
/// the dots that opened it.
///
/// The corner comes from where the control was last *painted*, because there
/// is nothing else in a frame that knows: the button is drawn by the list
/// rather than built as an element, so its rectangle exists in the scene and
/// not in the tree. That is a frame behind, which cannot show — the block a
/// menu is up on keeps its controls painted, and a modal underlay means
/// nothing under it can scroll while it is up.
///
/// Without a corner to hang off — a menu opened by
/// [`--block-menu`](crate::Overrides) before anything has been painted — it
/// hangs from the list's own top-right, where the topmost block's controls
/// are, so an unattended picture is still a picture of a menu on a block.
pub(super) fn anchor(at: Option<Vector2F>) -> AnchorTo {
    match at {
        Some(corner) => AnchorTo {
            parent: Corner::TopLeft,
            child: Corner::TopRight,
            offset: corner + vec2f(0., MENU_GAP),
            keep_on_screen: true,
            keep_clear_of_parent: false,
        },
        None => AnchorTo {
            parent: Corner::TopRight,
            child: Corner::TopRight,
            offset: vec2f(-CONTROL_INSET, CONTROL_OFFSET + CONTROL_SIZE + MENU_GAP),
            keep_on_screen: true,
            keep_clear_of_parent: false,
        },
    }
}

/// What the block's text can be copied as.
///
/// The first entry is what the square beside the dots does, spelled out — a
/// menu that left it out would be one missing the thing everything else in the
/// group is a variation of.
pub(crate) fn copy_group(workspace: &Workspace) -> Box<dyn Element> {
    let menu = workspace.block_menu();
    let ui = workspace.fonts().ui;
    group([
        entry(
            "Copy",
            chord_for(workspace, "crook/window/copy-block"),
            Some(dispatching(BlockAction::Copy(BlockPart::Whole))),
            menu.copy.clone(),
            ui,
        ),
        entry(
            "Copy command",
            chord_for(workspace, "crook/window/copy-block-command"),
            menu.command
                .is_some()
                .then(|| dispatching(BlockAction::Copy(BlockPart::Command))),
            menu.copy_command.clone(),
            ui,
        ),
        entry(
            "Copy output",
            chord_for(workspace, "crook/window/copy-block-output"),
            menu.output_from
                .is_some()
                .then(|| dispatching(BlockAction::Copy(BlockPart::Output))),
            menu.copy_output.clone(),
            ui,
        ),
    ])
}

/// What is known about the block rather than printed by it.
///
/// Both come from where the shell said it was when the command started, which
/// is why they are a group of their own rather than three copies and two.
pub(crate) fn facts_group(workspace: &Workspace) -> Box<dyn Element> {
    let menu = workspace.block_menu();
    let ui = workspace.fonts().ui;
    group([
        entry(
            "Copy working directory",
            chord_for(workspace, "crook/window/copy-block-directory"),
            menu.directory
                .is_some()
                .then(|| dispatching(BlockAction::Copy(BlockPart::Directory))),
            menu.copy_directory.clone(),
            ui,
        ),
        entry(
            "Copy git branch",
            chord_for(workspace, "crook/window/copy-block-branch"),
            menu.branch
                .is_some()
                .then(|| dispatching(BlockAction::Copy(BlockPart::Branch))),
            menu.copy_branch.clone(),
            ui,
        ),
    ])
}

/// Put the command back in the field, unsent.
///
/// "Run again" rather than "Re-run", because it does not run anything: the
/// shell is handed nothing and the person presses Enter, which is the only
/// honest way for a menu to offer a command a second time.
pub(crate) fn run_group(workspace: &Workspace) -> Box<dyn Element> {
    let menu = workspace.block_menu();
    group([entry(
        "Run again",
        chord_for(workspace, "crook/window/rerun-block"),
        menu.command
            .is_some()
            .then(|| dispatching(BlockAction::Rerun)),
        menu.rerun.clone(),
        workspace.fonts().ui,
    )])
}

/// The two ways of moving the list to the block, which are what a block taller
/// than the window is read with.
pub(crate) fn scroll_group(workspace: &Workspace) -> Box<dyn Element> {
    let menu = workspace.block_menu();
    let ui = workspace.fonts().ui;
    group([
        entry(
            "Scroll to top of block",
            chord_for(workspace, "crook/window/scroll-to-block-top"),
            Some(dispatching(BlockAction::ScrollTo(BlockEdge::Top))),
            menu.scroll_top.clone(),
            ui,
        ),
        entry(
            "Scroll to bottom of block",
            chord_for(workspace, "crook/window/scroll-to-block-bottom"),
            Some(dispatching(BlockAction::ScrollTo(BlockEdge::Bottom))),
            menu.scroll_bottom.clone(),
            ui,
        ),
    ])
}

/// One contribution's entries, as the column the menu puts a hairline under.
pub(crate) fn group(entries: impl IntoIterator<Item = Box<dyn Element>>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for entry in entries {
        column.add_child(entry);
    }
    column.finish()
}

/// What pressing an entry does, or `None` for a row that cannot be pressed:
/// being pressable *is* having something to run.
///
/// A closure rather than an action, because two kinds of entry end up in this
/// menu and they act differently. Crook's own dispatch a
/// [`BlockAction`]; a plugin's says which entry it was and then runs the
/// plugin's named action, which is the arrangement every other guest control
/// already uses.
pub(crate) type Press = Box<dyn Fn(&mut EventContext)>;

/// A press that dispatches one of the window's own actions.
pub(crate) fn dispatching(action: impl Into<WorkspaceAction> + Copy + 'static) -> Press {
    Box::new(move |ctx: &mut EventContext| ctx.dispatch_typed_action(action.into()))
}

/// One entry: a label, the chord that also reaches it, and what pressing it
/// does.
///
/// A row that cannot be pressed takes the muted role and no hover of its own,
/// and its press does nothing — rather than being a `Hoverable` with an empty
/// handler, which would still light up under the pointer and still claim the
/// press. Nothing about it invites a click.
///
/// `chord` is what [`chord_for`] found, or `None` for a row nothing is bound
/// to — which is most of them, since almost every command about a block ships
/// without one. The row is the place that answers "is there a faster way to do
/// this again", so it prints whatever is in force rather than what was shipped.
pub(crate) fn entry(
    label: impl Into<std::borrow::Cow<'static, str>>,
    chord: Option<String>,
    press: Option<Press>,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = label.into();
    let Some(press) = press else {
        // The hover state a disabled row is not using is dropped, so that a
        // row which becomes pressable again on the next block does not come
        // back lit under a pointer that has moved away.
        state.lock().reset_interaction_state();
        // No chord on a row that cannot be pressed: a key that is bound and
        // would decline is worse than a row that says nothing.
        return plate(
            Text::new(label, ui, LABEL_SIZE)
                .with_color(theme().text_muted)
                .finish(),
            Color::TRANSPARENT,
        );
    };

    Hoverable::new(state, move |mouse| {
        let background = if mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        };
        plate(line(label.clone(), chord.clone(), ui), background)
    })
    .on_click(move |_, ctx, _| press(ctx))
    .finish()
}

/// A row's label, and its chord against the far edge.
fn line(
    label: std::borrow::Cow<'static, str>,
    chord: Option<String>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let text = Text::new(label, ui, LABEL_SIZE)
        .with_color(theme().text_primary)
        .finish();
    let Some(chord) = chord else {
        return text;
    };

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(text)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(
            Text::new(chord, ui, CHORD_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        )
        .finish()
}

/// The chord that reaches one of the window's own commands, if one does.
///
/// The first of them, because a row is one line. Nothing is cached: the
/// keybindings are read on the frame the menu is drawn, so a chord recorded on
/// the Keyboard Shortcuts page is on this row the next time it opens.
pub(crate) fn chord_for(workspace: &Workspace, command: &str) -> Option<String> {
    let name = crook_plugin::ActionName::parse(command).ok()?;
    workspace.keybindings().chords_for(&name).into_iter().next()
}

/// The box a plugin's own element goes in when it is not an entry.
///
/// A group that describes something other than rows — a badge, a meter, a line
/// of prose — is still drawn, in the padding every row in this menu has, so it
/// lines up with the labels above and below it rather than sitting against the
/// popup's edge.
pub(crate) fn framed(content: Box<dyn Element>) -> Box<dyn Element> {
    plate(content, Color::TRANSPARENT)
}

/// The box a row's label sits in, which is the same box whether the row can be
/// pressed or not — so a disabled row is a row of the same height in the same
/// place, and the menu does not change shape between two blocks.
fn plate(label: Box<dyn Element>, background: Color) -> Box<dyn Element> {
    Container::new(label)
        .with_padding(Padding {
            left: ROW_INSET,
            right: ROW_INSET,
            top: ROW_PADDING,
            bottom: ROW_PADDING,
        })
        .with_background_color(background)
        .finish()
}

/// The hairline between two groups: full-bleed, in the role every other
/// divider in the application uses.
fn divider() -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(theme().overlay_2)
                .finish(),
        )
        .with_height(1.)
        .finish(),
    )
    .with_margin_top(6.)
    .with_margin_bottom(6.)
    .finish()
}
