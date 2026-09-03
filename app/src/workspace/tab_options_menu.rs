//! The tab options menu: Warp's "View options" popup, geometry for geometry.
//!
//! Five sections in Warp's order — View as, Density, Pane title as, the
//! Show/Additional-metadata pair, and Show details on hover — inside a 200px
//! column that hangs four pixels below the gear button. Every number in here is
//! Warp's (`app/src/workspace/view/vertical_tabs.rs:5733`), because the menu is
//! Warp's; the two places Crook deliberately differs are marked where they
//! happen.
//!
//! One menu serves both layouts. What differs is only which corner it hangs
//! off, and that is [`controls::gear_button`](super::controls)'s parameter
//! rather than a second copy of this file: the strip's gear aligns the left
//! edges and the panel's the right ones, because a 200px menu hung leftwards
//! off a 248px column opens across the body. Warp has two of these functions
//! and they have drifted; there is nothing in the popup itself that knows
//! where it is.
//!
//! # Three rules the popup only half-works without
//!
//! **The root is a [`Container`] with a background.** A container is the only
//! element that records a hit rect, so a menu built out of flexes and texts
//! floats visually while every click passes through it to the tabs underneath.
//! That recorded rect is also what stops a click on the popup's own padding
//! from dismissing it: the popup paints one layer above the [`Dismiss`]
//! underlay, so a press anywhere over it is *covered* and never reaches the
//! dismiss branch. Warp needs an explicit `EventHandler` returning
//! `StopPropagation` for the same effect.
//!
//! **Clicking an option does not close the menu.** Each row is a [`Hoverable`]
//! with a click handler, and a `Hoverable` claims the press — so `Dismiss` sees
//! the event handled and returns before dismissing. The menu is a persistent
//! preferences panel: change the title field, turn two chips off, and only then
//! click away.
//!
//! **Re-clicking the gear closes it once.** The gear is under the modal
//! underlay, so its own click handler never fires; the press goes through the
//! dismiss path instead. Letting the button fire while the popup is open would
//! toggle twice in one click and leave the menu looking frozen open.
//!
//! Escape does not close it. Warp has no keydown handler anywhere in this path,
//! and this is a port of Warp's semantics rather than an improvement on them —
//! so the omission is deliberate and is written down here rather than left to
//! be read as an oversight.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::settings::{Density, Granularity, PrimaryInfo, resolve_subtitle, subtitle_options_for};
use crate::tab::TabAction;
use crate::theme::THEME;

use super::action::{OptionsAction, WorkspaceAction};
use super::view::Workspace;
use super::wrap;

/// The popup's fixed width. Not derived from anything: Warp's panel is 248 and
/// its popup is 200, and the two numbers are unrelated.
const POPUP_WIDTH: f32 = 200.;

/// The corner radius of the popup shell and of a segmented control's track.
const POPUP_RADIUS: f32 = 6.;

/// Every row, header and control is inset by this much. Dividers are not — they
/// are full-bleed, and getting that backwards is the most visible way to miss
/// the design.
const ROW_INSET: f32 = 16.;

/// The size of a check slot, and of a density icon.
const ICON_SIZE: f32 = 16.;

/// Every label in the menu except the two "View as" segments.
const LABEL_SIZE: f32 = 12.;

/// The one 14px text in the popup: a "View as" segment.
const SEGMENT_LABEL_SIZE: f32 = 14.;

/// What the "PR link" toggle admits when hovered.
///
/// Warp's info affordance says the GitHub CLI has to be installed and
/// authenticated. Crook's reason is different and more basic, and saying it
/// here is the alternative to rendering a chip that can never appear.
const PR_LINK_NOTE: &str = "Crook has no forge integration yet, so no session has a link to show.";

/// The note's width, fixed rather than sized to its sentence.
///
/// Two things make "sized to content" wrong here and neither is obvious.
/// [`Text`] never wraps — it is built for a tab title, not a paragraph — and
/// [`Stack`] lays an anchored child out against the *window* rather than
/// against the box it hangs off, so nothing anywhere in the path stops one
/// line from measuring four hundred pixels. A note twice the width of the menu
/// it belongs to hangs over the tabs on both sides of it.
pub(super) const NOTE_WIDTH: f32 = 176.;

/// How many characters of the note go on one line.
///
/// Against [`NOTE_WIDTH`] minus its 16px of padding, at 11px: a character of
/// the interface font averages a little over half its size, so 28 of them fit
/// with room to spare for a font whose average runs wider.
const NOTE_LINE_CHARS: usize = 28;

/// The info dot's diameter, which the note is centred against.
pub(super) const INFO_DOT_SIZE: f32 = 12.;

/// The whole popup.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let options = workspace.options();
    let menu = workspace.menu();
    let ui = workspace.fonts().ui;

    // The info dot is drawn in exactly one state of the menu, and a dot that
    // is not drawn cannot receive the hover-out that would clear its mouse
    // state. Left alone, clicking the dot — which the row underneath claims,
    // turning "PR link" off and taking the dot away — leaves it believing it
    // is hovered, so switching "PR link" back on pops the note open with the
    // pointer nowhere near it. `tab_bar`'s close button closes the same trap
    // the same way.
    if options.density != Density::Expanded || !options.show_pr_link {
        menu.pr_link_info.lock().reset_interaction_state();
    }

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    column.add_child(header("View as", ui));
    column.add_child(segmented_track(
        text_segment(
            "Panes",
            options.granularity == Granularity::Panes,
            menu.panes.clone(),
            OptionsAction::SetGranularity(Granularity::Panes),
            ui,
        ),
        text_segment(
            "Tabs",
            options.granularity == Granularity::Tabs,
            menu.tabs.clone(),
            OptionsAction::SetGranularity(Granularity::Tabs),
            ui,
        ),
    ));

    column.add_child(divider());
    column.add_child(header("Density", ui));
    column.add_child(segmented_track(
        icon_segment(
            list_icon(),
            options.density == Density::Compact,
            menu.compact.clone(),
            OptionsAction::SetDensity(Density::Compact),
        ),
        icon_segment(
            grid_icon(),
            options.density == Density::Expanded,
            menu.expanded.clone(),
            OptionsAction::SetDensity(Density::Expanded),
        ),
    ));

    column.add_child(divider());
    column.add_child(header("Pane title as", ui));
    for (primary, state) in [
        (PrimaryInfo::Command, &menu.primary_command),
        (PrimaryInfo::WorkingDirectory, &menu.primary_directory),
        (PrimaryInfo::Branch, &menu.primary_branch),
    ] {
        column.add_child(check_row(
            primary.label(),
            options.primary_info == primary,
            state.clone(),
            OptionsAction::SetPrimaryInfo(primary),
            None,
            ui,
        ));
    }

    column.add_child(divider());
    // The two arms of one conditional, which is why no state of the menu shows
    // both. Compact has no metadata line to hang a chip on, so its toggles
    // would be inert; Expanded's second line is chosen for it by the title
    // field, so it has no subtitle to offer.
    match options.density {
        Density::Compact => {
            column.add_child(header("Additional metadata", ui));
            let chosen = resolve_subtitle(options.primary_info, options.subtitle);
            let states = [&menu.subtitle_first, &menu.subtitle_second];
            for (subtitle, state) in subtitle_options_for(options.primary_info)
                .into_iter()
                .zip(states)
            {
                column.add_child(check_row(
                    subtitle.label(),
                    // Against the *resolved* subtitle, so the check is never
                    // beside an option the row is silently overriding.
                    chosen == subtitle,
                    state.clone(),
                    OptionsAction::SetSubtitle(subtitle),
                    None,
                    ui,
                ));
            }
        }
        Density::Expanded => {
            column.add_child(header("Show", ui));
            column.add_child(check_row(
                "PR link",
                options.show_pr_link,
                menu.pr_link.clone(),
                OptionsAction::ToggleShowPrLink,
                options
                    .show_pr_link
                    .then(|| InfoNote::new(menu.pr_link_info.clone(), PR_LINK_NOTE)),
                ui,
            ));
            column.add_child(check_row(
                "Diff stats",
                options.show_diff_stats,
                menu.diff_stats.clone(),
                OptionsAction::ToggleShowDiffStats,
                None,
                ui,
            ));
        }
    }

    column.add_child(divider());
    // The one row with no header above it, and the only option every state of
    // the menu shows.
    column.add_child(check_row(
        "Show details on hover",
        options.show_details_on_hover,
        menu.details_on_hover.clone(),
        OptionsAction::ToggleShowDetailsOnHover,
        None,
        ui,
    ));

    // Warp opens its settings from the application menu bar and from
    // `cmd-,`; this popup has no such row. Crook has no menu bar at all, so
    // the one menu it does have carries the entry — otherwise the settings
    // page would be reachable only by a keystroke nobody was told about. It
    // opens a tab, so it is a `TabAction`, and the workspace takes the menu
    // down as it applies it.
    column.add_child(divider());
    column.add_child(settings_row(menu.settings.clone(), ui));

    ConstrainedBox::new(
        // Warp finishes this with `DropShadow::default()`. Crook's fragment
        // shader has no shadow branch — it was deliberately removed — so the
        // popup is separated from the strip by its border and by an opaque
        // ground instead. Rendering a shadow into nothing would be worse.
        Container::new(column.finish())
            .with_vertical_padding(8.)
            .with_background_color(THEME.surface_raised)
            .with_border(Border::all(1.).with_border_color(THEME.overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(POPUP_RADIUS)))
            .finish(),
    )
    .with_width(POPUP_WIDTH)
    .finish()
}

/// The row that opens the settings page.
///
/// Built here rather than through [`check_row`] because it is not a check row:
/// nothing about it is ever ticked, and passing `false` forever to a parameter
/// named `is_checked` would be a worse lie than eleven lines of layout. It
/// keeps that function's geometry — the same 16px slot, the same 8px gap — so
/// its label starts where every other label in the popup starts.
fn settings_row(state: MouseStateHandle, ui: FamilyId) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        let background = if mouse.is_hovered() {
            THEME.overlay_1
        } else {
            Color::TRANSPARENT
        };

        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(ICON_SIZE)
                            .with_height(ICON_SIZE)
                            .finish(),
                    )
                    .with_margin_right(8.)
                    .finish(),
                )
                .with_child(
                    Text::new("Settings\u{2026}", ui, LABEL_SIZE)
                        .with_color(THEME.text_primary)
                        .finish(),
                )
                .finish(),
        )
        .with_horizontal_padding(ROW_INSET)
        .with_vertical_padding(4.)
        .with_background_color(background)
        .finish()
    })
    .on_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::OpenSettings));
    })
    .finish()
}

/// A section heading: 12px sub-text, inset, with four pixels under it.
fn header(label: &'static str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(label, ui, LABEL_SIZE)
            .with_color(THEME.text_muted)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}

/// A hairline between sections.
///
/// Full-bleed on purpose: it has no horizontal padding while every header and
/// row is inset 16, and that contrast is what makes the sections read as
/// sections.
fn divider() -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(THEME.overlay_2)
                .finish(),
        )
        .with_height(1.)
        .finish(),
    )
    .with_margin_top(8.)
    .with_margin_bottom(8.)
    .finish()
}

/// The track two segments sit in: a rounded well, inset like a row.
fn segmented_track(left: Box<dyn Element>, right: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                // Each segment is exactly half the track, whatever is in it.
                .with_child(Expanded::new(1., left).finish())
                .with_child(Expanded::new(1., right).finish())
                .finish(),
        )
        .with_uniform_padding(4.)
        .with_background_color(THEME.overlay_2)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(POPUP_RADIUS)))
        .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_padding_bottom(4.)
    .finish()
}

/// The fill behind a segment. Shared by both kinds, and the only thing the
/// icon segments change when they are selected.
fn segment_background(is_selected: bool, is_hovered: bool) -> Color {
    if is_selected {
        THEME.overlay_3
    } else if is_hovered {
        THEME.overlay_1
    } else {
        Color::TRANSPARENT
    }
}

/// Wraps a segment's content in its pill.
fn segment_pill(
    content: Box<dyn Element>,
    is_selected: bool,
    is_hovered: bool,
) -> Box<dyn Element> {
    Container::new(Align::new(content).finish())
        .with_background_color(segment_background(is_selected, is_hovered))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_vertical_padding(2.)
        .finish()
}

/// One half of the "View as" control.
///
/// The label *does* change colour with selection here. The icon segments below
/// deliberately do not — see [`icon_segment`].
fn text_segment(
    label: &'static str,
    is_selected: bool,
    state: MouseStateHandle,
    action: OptionsAction,
    ui: FamilyId,
) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        segment_pill(
            Text::new(label, ui, SEGMENT_LABEL_SIZE)
                .with_color(if is_selected {
                    THEME.text_primary
                } else {
                    THEME.text_muted
                })
                .finish(),
            is_selected,
            mouse.is_hovered(),
        )
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Options(action)))
    .finish()
}

/// One half of the "Density" control.
///
/// The glyph is tinted `text_muted` whether or not this segment is selected —
/// only the pill behind it changes. That is not an oversight ported by
/// accident: Warp passes one `icon_color` into `render_popup_segment` and never
/// branches on selection, and the two adjacent controls having deliberately
/// different rules is visible in the design.
fn icon_segment(
    glyph: Box<dyn Element>,
    is_selected: bool,
    state: MouseStateHandle,
    action: OptionsAction,
) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        segment_pill(
            ConstrainedBox::new(glyph)
                .with_width(ICON_SIZE)
                .with_height(ICON_SIZE)
                .finish(),
            is_selected,
            mouse.is_hovered(),
        )
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Options(action)))
    .finish()
}

/// The Compact glyph: three rules, 16 wide and 2 tall, at y = 3, 7 and 11.
///
/// Warp draws `WarpIcon::Menu01` from an SVG. Crook has no icon system and this
/// glyph is literally three rectangles, so it is three rectangles — cheaper
/// than a path renderer introduced for two icons.
fn list_icon() -> Box<dyn Element> {
    let mut rules = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.);

    for _ in 0..3 {
        rules.add_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(THEME.text_muted)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.)))
                    .finish(),
            )
            .with_height(2.)
            .finish(),
        );
    }

    Container::new(rules.finish()).with_padding_top(3.).finish()
}

/// The Expanded glyph: four 7x7 rounded squares in a 2x2, 2px apart.
fn grid_icon() -> Box<dyn Element> {
    let cell = || {
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(THEME.text_muted)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.)))
                .finish(),
        )
        .with_width(7.)
        .with_height(7.)
        .finish()
    };
    let row = || {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(2.)
            .with_child(cell())
            .with_child(cell())
            .finish()
    };

    Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.)
        .with_child(row())
        .with_child(row())
        .finish()
}

/// An affordance that explains, on hover, why an option cannot do anything yet.
///
/// Warp's `ShowToggleInfoTooltip`, which it hangs off "PR link" when GitHub CLI
/// validation is suppressed. Crook's reason is permanent rather than
/// configuration-dependent, so the note is always attached while the toggle is
/// on.
struct InfoNote {
    state: MouseStateHandle,
    text: &'static str,
}

impl InfoNote {
    fn new(state: MouseStateHandle, text: &'static str) -> Self {
        Self { state, text }
    }
}

/// One exclusive or toggled option: a 16x16 check slot, a gap, a label.
///
/// The slot is 16x16 whether or not it holds a check, so a label never shifts
/// when the selection moves — and the label is `text_primary` selected *or*
/// not. Dimming the unselected rows is the obvious instinct and it is not what
/// Warp does; the check glyph is the only difference.
///
/// One renderer for all four of Warp's check lists. Warp has four copies of it
/// because each closes over a different settings enum; an [`OptionsAction`]
/// built by the caller removes the reason.
fn check_row(
    label: &'static str,
    is_checked: bool,
    state: MouseStateHandle,
    action: OptionsAction,
    info: Option<InfoNote>,
    ui: FamilyId,
) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        let check: Box<dyn Element> = if is_checked {
            Align::new(
                Text::new("\u{2713}", ui, LABEL_SIZE)
                    .with_color(THEME.text_primary)
                    .finish(),
            )
            .finish()
        } else {
            Empty::new().finish()
        };

        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        row.add_child(
            Container::new(
                ConstrainedBox::new(check)
                    .with_width(ICON_SIZE)
                    .with_height(ICON_SIZE)
                    .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        );
        row.add_child(
            Text::new(label, ui, LABEL_SIZE)
                .with_color(THEME.text_primary)
                .finish(),
        );
        if let Some(info) = info {
            row.add_child(
                Container::new(info_icon(&info, ui))
                    .with_padding_left(4.)
                    .finish(),
            );
        }

        Container::new(row.finish())
            .with_horizontal_padding(ROW_INSET)
            .with_vertical_padding(2.)
            // Full-bleed across the whole 198px content width, square-cornered,
            // because the background covers the inset padding and the column
            // stretches its children.
            .with_background_color(if mouse.is_hovered() {
                THEME.overlay_1
            } else {
                Color::TRANSPARENT
            })
            .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Options(action)))
    .finish()
}

/// The 12x12 info dot, and the note it shows while the pointer is on it.
///
/// The note is an anchored overlay child of a [`Stack`] around the dot, so it
/// escapes the menu's own column without changing its layout. It is added last,
/// which is the rule every anchored child obeys: an element hit-tests against
/// the topmost overlay that existed when it painted.
fn info_icon(info: &InfoNote, ui: FamilyId) -> Box<dyn Element> {
    let text = info.text;
    Hoverable::new(info.state.clone(), move |mouse| {
        let dot = ConstrainedBox::new(
            Container::new(
                Align::new(
                    Text::new("i", ui, 9.)
                        .with_color(THEME.surface_raised)
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(THEME.text_muted)
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
        )
        .with_width(12.)
        .with_height(12.)
        .finish();

        if !mouse.is_hovered() {
            return dot;
        }

        let mut stack = Stack::new().with_child(dot);
        stack.add_anchored_overlay_child(
            note_panel(text, ui),
            AnchorTo {
                // Above the dot and centred on it, which is Warp's
                // `ParentAnchor::TopMiddle` → `ChildAnchor::BottomMiddle` with
                // its -4 offset. [`Corner`] has only the four corners, so the
                // centring is the offset's job — exact, because the note's
                // width is a constant rather than whatever the sentence
                // happens to measure.
                //
                // Above rather than below, and that is not a matter of taste.
                // An overlay covers only what was painted *before* it, so a
                // note hung below the dot floats over the "Diff stats" row
                // while that row goes on hit-testing as uncovered — the exact
                // inversion of the rule the whole layer scheme rests on. Hung
                // above, it covers rows the popup painted earlier, and they
                // report themselves covered.
                parent: Corner::TopLeft,
                child: Corner::BottomLeft,
                offset: vec2f(-(NOTE_WIDTH - INFO_DOT_SIZE) / 2., -4.),
                keep_on_screen: true,
                keep_clear_of_parent: false,
            },
        );
        stack.finish()
    })
    .finish()
}

/// The floating panel the note is written in: a fixed-width column of wrapped
/// lines, clipped to it.
///
/// [`Clipped`] is the belt to [`NOTE_WIDTH`]'s braces. The width budget in
/// [`NOTE_LINE_CHARS`] is counted in characters and the font is measured in
/// pixels, so an interface font wider than the budget assumes would paint past
/// the panel's border; clipping means the worst case is a cut word rather than
/// a sentence lying across the menu.
fn note_panel(text: &str, ui: FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start);
    for line in wrap(text, NOTE_LINE_CHARS) {
        column.add_child(
            Text::new(line, ui, 11.)
                .with_color(THEME.text_primary)
                .finish(),
        );
    }

    ConstrainedBox::new(
        Container::new(Clipped::new(column.finish()).finish())
            .with_background_color(THEME.surface_raised)
            .with_border(Border::all(1.).with_border_color(THEME.overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_padding(Padding {
                top: 4.,
                left: 8.,
                bottom: 5.,
                right: 8.,
            })
            .finish(),
    )
    .with_width(NOTE_WIDTH)
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_is_broken_into_lines_that_fit_the_panel() {
        // The whole reason the note is not one `Text`: at 11px this sentence
        // measures about four hundred pixels, in a menu two hundred wide.
        let lines = wrap(PR_LINK_NOTE, NOTE_LINE_CHARS);

        assert!(lines.len() > 1, "the note was left on one line");
        for line in &lines {
            assert!(
                line.chars().count() <= NOTE_LINE_CHARS,
                "{line:?} is {} characters, over the {NOTE_LINE_CHARS} budget",
                line.chars().count()
            );
        }
        assert_eq!(
            lines.join(" "),
            PR_LINK_NOTE,
            "wrapping dropped or duplicated a word"
        );
    }

    #[test]
    fn a_word_longer_than_the_budget_gets_a_line_to_itself() {
        assert_eq!(
            wrap("a supercalifragilistic b", 8),
            vec!["a", "supercalifragilistic", "b"]
        );
    }
}
