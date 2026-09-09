//! What the palette looks like.
//!
//! A card floated over the middle of the window: a field at the top, a list of
//! commands under it, and a line at the bottom saying what the keys do. The
//! shape is the one every palette has had since Sublime Text, and there is
//! nothing to argue with in it.
//!
//! # It is centred by the element tree, not by the anchor
//!
//! An anchor's offset is a constant and the window's width is not, so the
//! surface is anchored at the window's top-left corner and laid out against
//! the whole window — a row with a spacer on each side, which is what puts the
//! card in the middle at any width. See `WINDOW_OVERLAY`.
//!
//! # Two lists, one card
//!
//! The card draws whatever [`super::rows`] worked out and decides none of it.
//! What it does decide comes out of one `match` on [`Rows::mode`]: the field's
//! icon, how tall the list may be, and the line at the bottom. Everything else
//! is one implementation read twice — Run mode is the list of keys with the
//! headings and the second cap taken out of it, not a second card — which is
//! what keeps the launcher's hot path the thing that shipped.
//!
//! # Nothing that is not a row may look like one
//!
//! A heading and a [`Row::Fact`] are drawn flat, with no fill and no
//! `Hoverable`, because the only line Enter can act on is a command and a
//! surface that lit up under the pointer on the others would be promising
//! something it cannot do. It is also what a test leans on: the selection is
//! found on a frame by being *the* wide `overlay_2` band with the row's
//! radius, and a second one anywhere would fail that assertion somewhere else
//! entirely.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crook_plugin::ActionName;

use crate::plugin::ActionId;
use crate::plugins::shortcuts::NOT_BOUND;
use crate::theme::theme;
use crate::workspace::{Fonts, TextField, WorkspaceAction};

use super::rows::{Entry, Mode, Row, Rows};
use super::state::{Control, Palette};

/// How wide the card is.
///
/// Wide enough for a title and an owner on one line without the two meeting,
/// and narrow enough that the eye does not have to travel from one to the
/// other. VS Code's is 600 at a 13px body; this is that at Crook's 12.
///
/// It is also the whole of a row's budget: `560 − 2 × 1 (the card's border)
/// − 2 × 10 (the card's padding) − 2 × 8 (the row's) = 522px` for a title, its
/// keys and its name. Nothing shipped comes near it — the widest bound row is
/// `Make the text bigger` with two caps and `crook/window/zoom-in`, at ≈ 416 —
/// and the title is the flexible child, so what happens past it is a clipped
/// title rather than a key painted over.
const CARD_WIDTH: f32 = 560.;

/// How far down the window the card starts. Not centred vertically: a palette
/// that grows downwards from a fixed point does not move under the pointer as
/// the list shortens, and every palette worth copying does this.
const TOP: f32 = 96.;

/// One row's height, which is what makes scrolling to a row arithmetic.
pub(super) const ROW_HEIGHT: f32 = 34.;

/// One group heading's height.
///
/// `10 (top) + 10.5 × 1.2 (=12.6) + 5 (bottom) = 27.6`, pinned rather than
/// measured for the same reason a row is: the offset a list scrolls to is a
/// sum over the lines above it, and a height that came out of a layout would
/// make that sum a guess. Pinned, the text can only be clamped, never spill.
///
/// It lives here and not in [`super::rows`] because a line must not be drawn
/// one height and scrolled to at another: one constant, read by both.
pub(super) const GROUP_HEIGHT: f32 = 28.;

/// How tall the list of things to run may grow.
///
/// Nine rows, which is what shipped. The list is as tall as it needs to be up
/// to that, so a palette with three commands in it is a small card rather than
/// a tall one with a hole in the bottom.
const RUN_LIST_HEIGHT: f32 = ROW_HEIGHT * 9.;

/// How tall the list of keys may grow.
///
/// Twelve rows, or nine rows and three headings: a grouped list has to show
/// enough of a group for the grouping to be visible at all. The card comes to
/// `408 + 2 (border) + 20 (padding) + 26 (field) + 10 + 10 + 13 (hint) ≈ 489`,
/// and `TOP + 489 = 585` fits the 640px window a snapshot opens.
const KEYS_LIST_HEIGHT: f32 = ROW_HEIGHT * 12.;

/// How tall this mode's list may grow, which is the window a row has to be
/// inside of.
///
/// Worked out from the mode rather than read off the last frame's layout,
/// because the two modes cap the list at two different heights and `tab`
/// changes the cap before anything has been measured under the new one:
/// scrolling a Run row into view against the 408 the keys list was measured at
/// puts it a hundred pixels below the fold, where nothing brings it back until
/// the next arrow key.
///
/// An upper bound and not the viewport: a window too short for the card
/// narrows the `ConstrainedBox` below this, and there the same under-scroll
/// returns — in a window where the card is already clipped.
pub(super) fn list_height(mode: Mode) -> f32 {
    match mode {
        Mode::Run => RUN_LIST_HEIGHT,
        Mode::Keys => KEYS_LIST_HEIGHT,
    }
}

/// The inset inside the card.
const PADDING: f32 = 10.;

/// What the field says while nothing has been typed.
///
/// Never seen in the list of keys, whose field always holds at least the
/// sigil, so it stays the launcher's word.
const PLACEHOLDER: &str = "Run a command";

/// What the launcher says when a query answers to nothing.
const NOTHING_RUN: &str = "No command matches that.";

/// The same sentence for the list of keys, which is not a list of commands.
///
/// A person who typed `?ctrl+c` asked about a key; telling them no *command*
/// matches would answer a question they did not ask, and would be wrong twice
/// over on a list that also holds the bindings naming nothing and the keys a
/// pane eats.
const NOTHING_KEYS: &str = "No key or command matches that.";

/// How many keys a command's row prints before it counts the rest.
///
/// Two, because a row is one line and the zoom on macOS answers to two
/// genuinely different chords. Past that the row says how many are left rather
/// than pushing the name off the end of the line.
const CHORDS_SHOWN: usize = 2;

/// The radius of one key's cap: concentric inside the row's 6, inside the
/// card's 10.
const CAP_RADIUS: f32 = 4.;

/// Between two caps.
const CAP_GAP: f32 = 6.;

/// Between the keys and the action name, which is what shipped as
/// `margin_right(10.)`.
const CLUSTER_GAP: f32 = 10.;

/// The size of everything on a row that is not its title.
///
/// The palette's secondary scale: the name, the keys, `not bound` and the
/// headings are all one step under the title, so a row reads as a title with
/// facts after it rather than as four columns of equals.
const SECONDARY_SIZE: f32 = 10.5;

/// The card, drawn over whatever is in the window.
///
/// Only ever called with the palette up: the contribution answers a closed one
/// with nothing at all, before there is a list to draw, so a card that is not
/// on screen costs a branch rather than a grouped build every frame.
pub(super) fn render(palette: &Palette, rows: &Rows) -> Box<dyn Element> {
    let fonts = palette.fonts();
    let mode = rows.mode();
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                TextField::new(
                    palette.query().clone(),
                    palette.clipboard().clone(),
                    fonts,
                    palette.control(Control::Query),
                    PLACEHOLDER,
                )
                // The whole of the mode indicator on screen, and it costs one
                // `match`: the icon set's own word for this glyph is "a chord:
                // what one key would do".
                .with_icon(match mode {
                    Mode::Run => Lucide::Search,
                    Mode::Keys => Lucide::Keyboard,
                })
                .finish(),
            )
            .with_margin_bottom(PADDING)
            .finish(),
        );

    if rows.is_empty() {
        column.add_child(
            Container::new(
                Text::new(
                    match mode {
                        Mode::Run => NOTHING_RUN,
                        Mode::Keys => NOTHING_KEYS,
                    },
                    fonts.ui,
                    12.,
                )
                .with_color(theme().text_muted)
                .finish(),
            )
            .with_uniform_padding(8.)
            .finish(),
        );
    } else {
        let mut lines = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for (index, row) in rows.rows().iter().enumerate() {
            lines.add_child(match row {
                Row::Group(name) => heading(name, fonts.ui),
                Row::Command(entry, id) => command(
                    entry,
                    *id,
                    index == palette.selected(),
                    palette.control(Control::Row(index)),
                    mode,
                    fonts,
                ),
                Row::Fact(entry) => fact(entry, mode, fonts),
            });
        }

        column.add_child(
            ConstrainedBox::new(
                Scrollable::new(palette.scroll(), lines.finish())
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .with_max_height(list_height(mode))
            .finish(),
        );
    }

    column.add_child(hint(mode, fonts.ui));

    let card = ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
            .with_uniform_padding(PADDING)
            .finish(),
    )
    .with_width(CARD_WIDTH)
    .finish();

    // A row that spans the window with the card between two spacers, which is
    // what centres it — see this module's own doc.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(card)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();

    Container::new(centred).with_margin_top(TOP).finish()
}

/// The name over a block of rows: the plugin that registered them, or one of
/// the blocks that belong to no plugin.
///
/// Text, and that is not only taste. **A heading must never paint a filled
/// rounded band:** a frame is asserted to carry exactly one wide `overlay_2`
/// band with the row's 6px radius on it, and that band is how a test finds the
/// selected row. A heading drawn like a row would make it two, and would fail
/// a test about clicking a row with a message about nothing.
fn heading(name: &str, ui: FamilyId) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Text::new(name.to_owned(), ui, SECONDARY_SIZE)
                .with_color(theme().text_muted)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Properties::default()
                })
                .finish(),
        )
        .with_padding(Padding {
            top: 10.,
            bottom: 5.,
            left: 8.,
            right: 8.,
        })
        .finish(),
    )
    .with_height(GROUP_HEIGHT)
    .finish()
}

/// One command: the only line the keyboard lands on and the pointer runs.
fn command(
    entry: &Entry,
    id: ActionId,
    selected: bool,
    state: MouseStateHandle,
    mode: Mode,
    fonts: Fonts,
) -> Box<dyn Element> {
    let line = line(entry, mode, fonts);

    ConstrainedBox::new(
        Hoverable::new(state, move |mouse| {
            let background = match (selected, mouse.is_hovered()) {
                (true, _) => theme().overlay_2,
                (false, true) => theme().overlay_1,
                (false, false) => Color::TRANSPARENT,
            };

            Container::new(line)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .with_padding(Padding {
                    top: 7.,
                    bottom: 7.,
                    left: 8.,
                    right: 8.,
                })
                .finish()
        })
        .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(id)))
        .finish(),
    )
    .with_height(ROW_HEIGHT)
    .finish()
}

/// One line that only explains itself: a key the pane eats, or a binding
/// naming a command this machine does not have.
///
/// The same line in the same box, and no `Hoverable` around it — there is
/// nothing to run, so nothing here may light up under the pointer as though
/// there were. It costs no mouse state either, which is what keeps the hover
/// map a map of commands.
fn fact(entry: &Entry, mode: Mode, fonts: Fonts) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(line(entry, mode, fonts))
            .with_padding(Padding {
                top: 7.,
                bottom: 7.,
                left: 8.,
                right: 8.,
            })
            .finish(),
    )
    .with_height(ROW_HEIGHT)
    .finish()
}

/// What is written across a row: what it is called, what reaches it, and the
/// name to write in a file.
fn line(entry: &Entry, mode: Mode, fonts: Fonts) -> Box<dyn Element> {
    let mut line = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(titled(entry, fonts.ui));

    // The keys before the name and brighter than it: the name answers "what do
    // I write in my keybindings file", and the keys answer "is there a faster
    // way to do this again", which is the question somebody reading a list of
    // things to do is actually asking.
    let named = name_column(entry);
    if let Some(cluster) = cluster(entry, mode, fonts) {
        let cluster = Container::new(cluster);
        line.add_child(match named.is_some() {
            true => cluster.with_margin_right(CLUSTER_GAP).finish(),
            false => cluster.finish(),
        });
    }

    if let Some(name) = named {
        line.add_child(
            Text::new(name.to_owned(), fonts.ui, SECONDARY_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        );
    }

    line.finish()
}

/// The action name, when it says something the title does not.
///
/// Nothing for a key the pane eats, which has no name because no keybindings
/// file can reach it; and nothing for a binding on a plain action, whose title
/// *is* its name — the same string twice on one row is not a column, it is a
/// stutter.
fn name_column(entry: &Entry) -> Option<&str> {
    entry
        .action
        .as_ref()
        .map(ActionName::as_str)
        .filter(|name| *name != entry.title)
}

/// What the row is called, and what is wrong with it when something is.
///
/// **The title is the flexible child**, and that is the one structural thing
/// about this line. `Flex` measures an inflexible child against an *infinite*
/// main axis, so a plain `Text` reports its full natural width, starves the
/// spacer beside it to nothing and paints over the keys. `Expanded` hands it
/// `FlexFit::Tight` — `min.x == max.x == whatever is left` — and a `Text` laid
/// out against that drops the glyphs past its bounds. So a long title is what
/// loses characters, never the caps, which are the half of the row that cannot
/// be guessed from the other half. `with_no_overflow` is the wrong tool for
/// the same reason: it takes the shortfall from the *last* inflexible child,
/// which is the action name.
fn titled(entry: &Entry, ui: FamilyId) -> Box<dyn Element> {
    let title = Text::new(entry.title.clone(), ui, 12.)
        .with_color(theme().text_primary)
        .finish();

    let Some(note) = entry.note else {
        return Expanded::new(1., title).finish();
    };

    // The note sits against the title rather than out by the keys, because it
    // is something about *this* name and not a column of its own. Inflexible,
    // so the title is still what gives way — and with air on both sides,
    // because the title cell ends where it ends and the keys begin right
    // there: without the second margin `nothing answers to this` and
    // `not bound` come out as one word.
    Expanded::new(
        1.,
        Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., title).finish())
            .with_child(
                Container::new(
                    Text::new(note, ui, SECONDARY_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_horizontal_margin(8.)
                .finish(),
            )
            .finish(),
    )
    .finish()
}

/// Every key that reaches the row, and what to say when nothing does.
fn cluster(entry: &Entry, mode: Mode, fonts: Fonts) -> Option<Box<dyn Element>> {
    if entry.chords.is_empty() {
        // In the list of keys only. `not bound` on forty rows of a launcher is
        // forty pieces of noise on the fast path, and the *absence of the cap
        // shape* is what lets the eye sort bound from unbound in the keymap
        // without reading either. The word is `shortcuts::NOT_BOUND`, shared
        // with the Keyboard Shortcuts page: two surfaces, one word.
        if mode == Mode::Run {
            return None;
        }
        return Some(
            Text::new(NOT_BOUND, fonts.ui, SECONDARY_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        );
    }

    // A key the pane eats has no action name, and that is the whole of what
    // tells the two kinds of row apart here. Its value is everything the
    // program answers to and all of it is drawn: there is no name column to
    // make room for, there are at most four parts, and a group whose point is
    // the keys must not hide one of them behind a counter.
    let eaten = entry.action.is_none();
    let shown = match (eaten, mode) {
        (true, _) => entry.chords.len(),
        // One, as it shipped: a launcher's row says whether there is a faster
        // way, not how many.
        (false, Mode::Run) => 1,
        (false, Mode::Keys) => CHORDS_SHOWN,
    };

    let mut cluster = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);
    for (at, chord) in entry.chords.iter().take(shown).enumerate() {
        // A part with a space in it is a phrase and not a key —
        // `hover it, then click`, `alt+right for one word` — and a rounded
        // outline around it would be claiming it is something you can press.
        // Only a pane's row can hold one: a space inside a chord is a
        // *sequence*, `ctrl+k ctrl+s`, which is one key pressed after another
        // and one cap.
        let part = match eaten && chord.text.contains(' ') {
            true => Text::new(chord.text.clone(), fonts.ui, SECONDARY_SIZE)
                .with_color(theme().text_muted)
                .finish(),
            false => cap(&chord.text, fonts.monospace),
        };
        cluster.add_child(match at {
            0 => part,
            _ => Container::new(part).with_margin_left(CAP_GAP).finish(),
        });
    }

    if mode == Mode::Keys && !eaten {
        // What the budget left behind, and `shown` is a budget rather than a
        // count: a row with one key has nothing past two.
        let rest = entry.chords.len().saturating_sub(shown);
        if rest > 0 {
            cluster.add_child(
                Container::new(
                    Text::new(format!("+{rest}"), fonts.ui, SECONDARY_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_margin_left(CAP_GAP)
                .finish(),
            );
        }

        // Off the first key, because that is the one the row is showing. The
        // words say "this is conditional, and here is the term to look for";
        // the clause itself is on the settings page, which is the surface that
        // can also change it.
        if let Some(when) = entry
            .chords
            .first()
            .and_then(|chord| chord.only_when.as_ref())
        {
            cluster.add_child(
                Container::new(
                    Text::new(format!("when {when}"), fonts.ui, SECONDARY_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            );
        }
    }

    Some(cluster.finish())
}

/// One key, in a box the size of the notation on it.
///
/// Four choices, each with a reason. **Monospace**, because a chord is
/// something a person also reads in a file — the reason the Keyboard Shortcuts
/// page already draws its chord in the terminal's family. **`overlay_3` for
/// the outline and not `border`**, because a theme flattens `border` against
/// `surface`, and inside a filled box on a light theme it disappears.
/// **`surface` for the fill**, so the cap is monotone in all three of a row's
/// states — on a plain row it is the ground and only the outline shows, on a
/// hovered row it is one step darker than the band, on the selected row two —
/// whereas a transparent cap loses its shape on exactly the row the eye is on.
/// And **10.5 rather than that page's 11**, so a key never comes out larger
/// than the name sitting next to it.
fn cap(text: &str, monospace: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_owned(), monospace, SECONDARY_SIZE)
            .with_color(theme().text_primary)
            .finish(),
    )
    .with_padding(Padding {
        top: 2.,
        bottom: 2.,
        left: 5.,
        right: 5.,
    })
    .with_background_color(theme().surface)
    .with_border(Border::all(1.).with_border_color(theme().overlay_3))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CAP_RADIUS)))
    .finish()
}

/// The line at the bottom that says what the keys do.
///
/// It is also where the second list is advertised, on every single open, which
/// is a discovery mechanism no chord and no settings page can claim. The
/// arrows stay in the ui family: `fonts.monospace` is the user's *terminal*
/// font, and a programming font without U+2191 would put tofu in the one line
/// whose job is to teach the keys.
fn hint(mode: Mode, ui: FamilyId) -> Box<dyn Element> {
    let text = match mode {
        Mode::Run => {
            "\u{2191}\u{2193} to move \u{00b7} enter to run \u{00b7} ? for the keys \
             \u{00b7} esc to close"
        }
        Mode::Keys => {
            "\u{2191}\u{2193} to move \u{00b7} enter to run \u{00b7} tab for the commands \
             \u{00b7} esc to close"
        }
    };

    Container::new(
        Text::new(text, ui, SECONDARY_SIZE)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_margin_top(PADDING)
    .finish()
}
