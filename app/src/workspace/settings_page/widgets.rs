//! The controls a settings row is built from.
//!
//! Warp's settings pages are built out of roughly a hundred and ten switches,
//! sixty-five buttons, twenty-two dropdowns and a handful of radio groups; the
//! shape of a row is the same in all of them, and it is the shape this file
//! reproduces. A row is a column: a header line with the label on the left and
//! the control hard against the right edge, and — when there is one — a
//! description on a second line, set smaller and muted, ending well short of
//! the control so the two never read as one sentence.
//!
//! # What is here and what is not
//!
//! A **switch**, because it is the control Warp reaches for twice as often as
//! everything else combined. A **segmented control**, because Crook already
//! has one in the options menu and a two-value choice reads better as two halves
//! than as a dropdown Crook cannot draw. A **choice row**, which is Warp's
//! radio group with the check mark on the right, for the three-value options.
//! A **text button**, for the one action a page can take. And a **fact row**,
//! label left and value right, for the About page, where nothing is editable.
//!
//! No dropdown and no text input: both need a popup or a caret, and Crook has
//! neither. Where Warp uses a dropdown for a short list, the choice row says
//! the same thing with every option visible, which is strictly more useful at
//! three options and unusable at thirty. That is the trade this page is
//! allowed to make and Warp's, at eight hundred settings, is not.
//!
//! # Disabled rather than hidden
//!
//! An option that another option has made inert is drawn, greyed, and given no
//! click handler — Warp's third way of handling an irrelevant setting, and the
//! one it picks when the dependency is worth showing. The options menu takes the
//! other route for the same two options: it drops "PR link" and "Diff stats"
//! from the popup entirely while the density is `Compact`. Both are right for
//! their surface. A 200px popup that grew and shrank as you used it would be
//! unusable, and a settings page that silently omitted the switch you came
//! looking for would send you to the file.

use crookui_core::elements::{MouseStateHandle, Padding, Paragraph};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use super::search::{Query, Words};
use crate::theme::theme;

use super::super::action::WorkspaceAction;

/// The size a page's own title is set in.
///
/// Warp's is 23px against a 12px body. Crook's whole interface lives between
/// 10 and 14, and a 23px heading inside a 720px card reads as a different
/// application's dialog dropped into this one, so the ratio is kept and the
/// absolute size is not.
pub(crate) const TITLE_SIZE: f32 = 16.;

/// A category heading, above a group of rows.
pub(crate) const CATEGORY_SIZE: f32 = 11.;

/// A row's label, and the interface's default size everywhere else.
pub(crate) const LABEL_SIZE: f32 = 12.;

/// A row's second line, and every other piece of text that is explaining
/// rather than naming.
pub(crate) const DESCRIPTION_SIZE: f32 = 11.;

/// The gap under one row, before the next one's label.
/// The check on the chosen row of a picker.
const CHECK_SIZE: f32 = 13.;

const ROW_SPACING: f32 = 14.;

/// How far a description stops short of the right edge, so it wraps — or here,
/// where nothing wraps, is cut off — well clear of the control.
const DESCRIPTION_RIGHT_MARGIN: f32 = 96.;

/// The switch's track.
const SWITCH_WIDTH: f32 = 28.;
const SWITCH_HEIGHT: f32 = 16.;

/// The knob inside it, and the space either side of it.
const KNOB_SIZE: f32 = 12.;
const KNOB_INSET: f32 = 2.;

/// The corner radius of a segmented control's track and of a button.
const CONTROL_RADIUS: f32 = 6.;

/// What a control does when it is clicked, and whether it can be.
///
/// `None` is the disabled state, and it carries no action precisely so that a
/// disabled control cannot be given one by accident: the click handler is
/// attached in exactly one place, behind a match on this.
pub(crate) type Command = Option<WorkspaceAction>;

/// One thing in a category: what is drawn, and the words that find it.
///
/// `words` is `None` for a note. A paragraph belongs to the category rather
/// than to any row in it, so there is nothing for a query to land on — and a
/// search that answered with four paragraphs of prose because one of them
/// contains the word "tab" would be worse than one that answered with the row.
pub(crate) struct Entry {
    /// What finds it, unless nothing does.
    pub(crate) words: Option<Words>,
    /// What the row *says* on its right-hand side, where that is a string
    /// rather than a control: a chord, a path, a version.
    ///
    /// Searchable, and it has to be a `String` rather than one more
    /// `&'static str` in [`Words`] because these are computed — the chord
    /// depends on the platform and the path on the machine. Somebody who
    /// remembers a key and not what it does types the key.
    value: Option<String>,
    /// What is drawn.
    pub(crate) element: Box<dyn Element>,
}

impl Entry {
    /// An entry nothing can search for.
    fn unsearchable(element: Box<dyn Element>) -> Self {
        Self {
            words: None,
            value: None,
            element,
        }
    }

    /// Whether `query` is looking for this.
    ///
    /// `context` is the page and the category the row is in. A note goes
    /// wherever its category goes: drawn when nothing is being searched for,
    /// and left out the moment something is.
    pub(crate) fn matches(&self, query: &Query, context: &[&str]) -> bool {
        let Some(words) = &self.words else {
            return query.is_empty();
        };

        match &self.value {
            Some(value) => {
                let mut haystack = context.to_vec();
                haystack.push(value);
                query.matches(words, &haystack)
            }
            None => query.matches(words, context),
        }
    }
}

/// A heading and the rows under it.
///
/// Built as a value rather than as an element because the search filters it:
/// which categories survive — and therefore which one is *first* and draws no
/// divider above itself — is not known until the query has been applied.
pub(crate) struct Category {
    /// What the heading says, which is also a word every row in it is found
    /// by.
    ///
    /// Owned, like everything else a row is described by: a page contributed
    /// by a plugin has headings nobody wrote down here.
    pub(crate) title: String,
    /// The rows and the notes, in order.
    pub(crate) entries: Vec<Entry>,
}

/// A page's heading.
pub(crate) fn page_title(title: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(title.to_owned(), ui, TITLE_SIZE)
            .with_color(theme().text_primary)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Properties::default()
            })
            .finish(),
    )
    .with_margin_bottom(16.)
    .finish()
}

/// A heading and the rows under it, gathered.
pub(crate) fn category(title: impl Into<String>, entries: Vec<Entry>) -> Category {
    Category {
        title: title.into(),
        entries,
    }
}

/// One category, drawn.
///
/// The separator goes *above* the heading rather than below the last row, so
/// that a page never ends in a rule with nothing under it — Warp's rule, which
/// it implements by not drawing the separator after the final category. Here
/// the first category is the one that skips it.
///
/// Which one that is used to be settled at the call site, on the grounds that
/// `first` is a property of the category rather than of the list it happens to
/// be in. The search made that false: a category is first when every category
/// before it has been filtered away.
pub(crate) fn category_element(
    title: &str,
    first: bool,
    rows: Vec<Box<dyn Element>>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    if !first {
        column.add_child(
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(1.)
                    .finish(),
            )
            .with_background_color(theme().border)
            .with_margin_top(6.)
            .with_margin_bottom(18.)
            .finish(),
        );
    }

    column.add_child(
        Container::new(
            Text::new(title.to_owned(), ui, CATEGORY_SIZE)
                .with_color(theme().text_muted)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Properties::default()
                })
                .finish(),
        )
        .with_margin_bottom(10.)
        .finish(),
    );

    column.add_children(rows);
    column.finish()
}

/// One setting: a label, an optional description under it, and a control.
pub(crate) fn row(words: Words, enabled: bool, control: Box<dyn Element>, ui: FamilyId) -> Entry {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(label_text(words.label.clone(), enabled, ui))
                // The control is pushed against the right edge by a spacer
                // rather than placed at a column position, so that rows with
                // labels of wildly different lengths still line their controls
                // up.
                .with_child(Expanded::new(1., Empty::new().finish()).finish())
                .with_child(control)
                .finish(),
        );

    // Under the control as well as under the label, which is why it is a child
    // of the row's column rather than of the label's: a description confined
    // to the label's share of the line would be cut off at the width of
    // whatever control happens to sit beside it.
    if let Some(description) = words.description.clone() {
        column.add_child(description_text(description, ui));
    }

    Entry {
        words: Some(words),
        value: None,
        element: Container::new(column.finish())
            .with_margin_bottom(ROW_SPACING)
            .finish(),
    }
}

/// A row's label, greyed when the row is inert.
fn label_text(label: String, enabled: bool, ui: FamilyId) -> Box<dyn Element> {
    Text::new(label, ui, LABEL_SIZE)
        .with_color(if enabled {
            theme().text_primary
        } else {
            theme().text_muted
        })
        .finish()
}

/// The second line under a label.
fn description_text(description: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(description, ui, DESCRIPTION_SIZE)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_margin_top(4.)
    .with_margin_right(DESCRIPTION_RIGHT_MARGIN)
    .finish()
}

/// A label and description with a list of values under them, one of which is
/// ticked.
///
/// Warp puts a several-valued option in a dropdown, so its label and its
/// control share a line like every other row's. Crook draws the values, so
/// they need a line each and the label needs to sit above them rather than
/// beside a control that is not there. The bottom margin belongs to the group
/// rather than to the last value, which is what keeps the gap between two
/// groups the same as the gap between two rows.
pub(crate) fn choice_group(
    words: Words,
    enabled: bool,
    choices: Vec<Box<dyn Element>>,
    ui: FamilyId,
) -> Entry {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label_text(words.label.clone(), enabled, ui));

    if let Some(description) = words.description.clone() {
        column.add_child(description_text(description, ui));
    }

    column.add_child(
        Container::new(Empty::new().finish())
            .with_margin_bottom(6.)
            .finish(),
    );
    column.add_children(choices);

    Entry {
        words: Some(words),
        value: None,
        element: Container::new(column.finish())
            .with_margin_bottom(ROW_SPACING)
            .finish(),
    }
}

/// A switch: on, off, or inert.
///
/// The knob is placed by a row flex rather than by an offset, because the
/// track has no coordinate system of its own — an element is told where it
/// ends up only after it has been measured, so "12 pixels from the left of
/// something 28 wide" is not a thing this layer can say. Main-axis alignment
/// is, and it says the same thing in the one vocabulary the box protocol has.
pub(crate) fn switch(on: bool, command: Command, state: MouseStateHandle) -> Box<dyn Element> {
    let enabled = command.is_some();

    let control = Hoverable::new(state, move |mouse| {
        let track = match (on, enabled) {
            (true, true) => theme().accent,
            (true, false) => theme().overlay_3,
            (false, true) if mouse.is_hovered() => theme().overlay_3,
            (false, _) => theme().overlay_2,
        };
        let knob = if enabled {
            theme().text_primary
        } else {
            theme().text_muted
        };

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(if on {
                MainAxisAlignment::End
            } else {
                MainAxisAlignment::Start
            });
        row.add_child(
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(KNOB_SIZE)
                    .with_height(KNOB_SIZE)
                    .finish(),
            )
            .with_background_color(knob)
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
        );

        ConstrainedBox::new(
            Container::new(row.finish())
                .with_background_color(track)
                .with_uniform_padding(KNOB_INSET)
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .finish(),
        )
        .with_width(SWITCH_WIDTH)
        .with_height(SWITCH_HEIGHT)
        .finish()
    });

    with_command(control, command)
}

/// One half of a segmented control.
pub(crate) struct Segment {
    /// What it says.
    pub(crate) label: &'static str,
    /// Whether it is the one currently chosen.
    pub(crate) selected: bool,
    /// What clicking it does, or `None` while the control is inert.
    pub(crate) command: Command,
    /// Its own hover state, never shared with the other segments.
    pub(crate) state: MouseStateHandle,
}

/// A track holding two or more segments, of which exactly one is lit.
pub(crate) fn segmented(segments: Vec<Segment>, ui: FamilyId) -> Box<dyn Element> {
    let mut row = Flex::row().with_main_axis_size(MainAxisSize::Min);

    for segment in segments {
        let selected = segment.selected;
        let enabled = segment.command.is_some();
        let label = segment.label;

        let pill = Hoverable::new(segment.state, move |mouse| {
            let (background, color) = if selected {
                (theme().overlay_3, theme().text_primary)
            } else if enabled && mouse.is_hovered() {
                (theme().overlay_2, theme().text_primary)
            } else {
                (Color::TRANSPARENT, theme().text_muted)
            };

            Container::new(Text::new(label, ui, LABEL_SIZE).with_color(color).finish())
                .with_padding(Padding {
                    top: 3.,
                    bottom: 3.,
                    left: 10.,
                    right: 10.,
                })
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS - 2.)))
                .finish()
        });

        row.add_child(with_command(pill, segment.command));
    }

    Container::new(row.finish())
        .with_background_color(theme().overlay_2)
        .with_uniform_padding(2.)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)))
        .finish()
}

/// A full-width row that names one value of a several-valued option, with a
/// check mark when it is the chosen one.
///
/// Warp draws these as a dropdown. This is the options menu's check row at the
/// page's size, and it is what a three-value option looks like when there is
/// no popup to put a list in.
pub(crate) fn choice(
    label: &'static str,
    selected: bool,
    command: Command,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let enabled = command.is_some();

    let control = Hoverable::new(state, move |mouse| {
        let background = if enabled && mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        };
        let color = if enabled {
            theme().text_primary
        } else {
            theme().text_muted
        };

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Text::new(label, ui, LABEL_SIZE).with_color(color).finish())
                .with_child(Expanded::new(1., Empty::new().finish()).finish())
                .with_child(
                    // A check that is drawn in the background colour when the
                    // row is not the chosen one, rather than not drawn at all:
                    // an element that comes and goes changes the row's height
                    // by a pixel as the pointer moves down the list.
                    Icon::new(Lucide::Check, CHECK_SIZE)
                        .with_color(if selected {
                            theme().accent
                        } else {
                            Color::TRANSPARENT
                        })
                        .finish(),
                )
                .finish(),
        )
        .with_padding(Padding {
            top: 5.,
            bottom: 5.,
            left: 8.,
            right: 8.,
        })
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_margin_bottom(2.)
        .finish()
    });

    with_command(control, command)
}

/// The corner of the box a control that answers a question sits in.
///
/// Softer than a row's, because the box is a *card's* and a card's own
/// rounding is what makes it read as part of the page rather than as something
/// dropped on top of it.
pub(crate) const ANSWER_RADIUS: f32 = 8.;

/// The inset inside that box.
///
/// Wider than it is tall, which is what the switch has carried since it was
/// the only control on a card: a label at one end and a control at the other
/// need more air along the row than across it, or both sit against a corner.
pub(crate) const ANSWER_PADDING: Padding = Padding {
    top: 10.,
    bottom: 10.,
    left: 12.,
    right: 12.,
};

/// The box a control that answers a question about a plugin sits in.
///
/// One shape for every one of them, and there are now four: is it on, may it
/// do this, do you want it, and do you want it gone. Two boxes differing by a
/// couple of pixels would read as two mechanisms, which is why this is here
/// rather than on either card — the Store and the Plugins page are read one
/// after the other.
///
/// `live` is whether the control does anything, and it is the *label* that
/// says so: a muted word beside a control that cannot be pressed is the only
/// hint there is, since a disabled control carries no handler at all.
pub(crate) fn answer(
    label: impl Into<String>,
    live: bool,
    control: Box<dyn Element>,
    ui: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(label.into(), ui, LABEL_SIZE)
                    .with_color(if live {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(control)
            .finish(),
    )
    .with_background_color(theme().overlay_1)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ANSWER_RADIUS)))
    .with_padding(ANSWER_PADDING)
    .finish()
}

/// A small outlined button.
///
/// `changed` is Warp's trick, and it is the only "this differs from the
/// default" indicator either application has: the reset button is drawn
/// de-emphasised and does nothing while there is nothing to reset, so the
/// control that undoes a change is also the one that says a change was made.
pub(crate) fn text_button(
    label: impl Into<String>,
    command: Command,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let enabled = command.is_some();
    // Owned rather than `&'static str`, for the reason a row's words are: the
    // Keyboard Shortcuts page puts a chord on a button, and a chord is
    // computed.
    let label = label.into();

    let control = Hoverable::new(state, move |mouse| {
        let (background, color) = if !enabled {
            (Color::TRANSPARENT, theme().text_muted)
        } else if mouse.is_hovered() {
            (theme().overlay_2, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().text_primary)
        };

        Container::new(
            Text::new(label.clone(), ui, DESCRIPTION_SIZE)
                .with_color(color)
                .finish(),
        )
        .with_padding(Padding {
            top: 4.,
            bottom: 4.,
            left: 10.,
            right: 10.,
        })
        .with_background_color(background)
        .with_border(Border::all(1.).with_border_color(if enabled {
            theme().border
        } else {
            Color::TRANSPARENT
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)))
        .finish()
    });

    with_command(control, command)
}

/// The chord on a Keyboard Shortcuts row, and the button that changes it.
///
/// A [`text_button`] in the terminal's own family, because a chord is
/// something a person also reads in a file, plus the one state no other
/// control on the page has: while it is recording it is lit with the accent
/// and it is the only thing in the window the keyboard is reaching.
///
/// `bound` is false for a command nothing reaches, which is drawn the way a
/// disabled control is drawn everywhere else on this page — the row still
/// works, it just has nothing to say yet.
pub(crate) fn chord_button(
    chord: impl Into<String>,
    recording: bool,
    bound: bool,
    command: Command,
    state: MouseStateHandle,
    fonts: super::super::view::Fonts,
) -> Box<dyn Element> {
    let enabled = command.is_some();
    let chord = chord.into();

    let control = Hoverable::new(state, move |mouse| {
        let (background, border, color) = if recording {
            (theme().overlay_2, theme().accent, theme().text_primary)
        } else if !enabled {
            (Color::TRANSPARENT, theme().border, theme().text_muted)
        } else if mouse.is_hovered() {
            (theme().overlay_2, theme().border, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().border, theme().text_primary)
        };

        Container::new(
            Text::new(chord.clone(), fonts.monospace, DESCRIPTION_SIZE)
                .with_color(match bound || recording {
                    true => color,
                    false => theme().text_muted,
                })
                .finish(),
        )
        .with_padding(Padding {
            top: 4.,
            bottom: 4.,
            left: 10.,
            right: 10.,
        })
        .with_background_color(background)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)))
        .finish()
    });

    with_command(control, command)
}

/// Two controls side by side on the right of a row.
///
/// The Keyboard Shortcuts page is the only page with a row that has more than
/// one: a chord and the button that puts it back.
pub(crate) fn pair(first: Box<dyn Element>, second: Box<dyn Element>) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(first)
        .with_child(Container::new(second).with_margin_left(6.).finish())
        .finish()
}

/// A stepper: a minus, the value, and a plus.
///
/// The control the terminal's type size is set with. Not a `segmented`,
/// because that control lights the chosen segment and nothing here is chosen —
/// and not a text field, because `crookui_core` has no such element yet and
/// the one half of one that exists measures in terminal cells.
///
/// Either button is `None` at its end of the range, which is the same "does
/// nothing and says so" [`text_button`] already draws for the reset button.
pub(crate) fn stepper(
    value: String,
    decrease: Command,
    decrease_state: MouseStateHandle,
    increase: Command,
    increase_state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(text_button("\u{2212}", decrease, decrease_state, ui))
        .with_child(
            Container::new(
                Text::new(value, ui, DESCRIPTION_SIZE)
                    .with_color(theme().text_primary)
                    .finish(),
            )
            // Wide enough that the row does not shuffle sideways as the number
            // gains and loses a digit under a pointer that is holding still on
            // one of the two buttons.
            .with_horizontal_padding(12.)
            .finish(),
        )
        .with_child(text_button("+", increase, increase_state, ui))
        .finish()
}

/// The row that says which theme is in force, and opens the panel.
///
/// Warp's shape: the preview on the left, the name to its right, and the whole
/// row is the button. Hovering draws the accent border Warp draws, because the
/// row is the only control on the page that leads somewhere rather than
/// changing something.
pub(crate) fn current_theme_row(
    words: Words,
    card: Box<dyn Element>,
    name: String,
    action: WorkspaceAction,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Entry {
    let name_text = name.clone();
    let mut card = Some(card);
    let mut name = Some(name);

    let row = Hoverable::new(state, move |mouse| {
        let border = if mouse.is_hovered() {
            theme().accent
        } else {
            theme().border
        };

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(card.take().unwrap_or_else(|| Empty::new().finish()))
                .with_child(
                    Container::new(
                        Flex::column()
                            .with_main_axis_size(MainAxisSize::Min)
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_child(
                                Text::new(name.take().unwrap_or_default(), ui, LABEL_SIZE)
                                    .with_color(theme().text_primary)
                                    .with_style(Properties {
                                        weight: Weight::Semibold,
                                        ..Properties::default()
                                    })
                                    .finish(),
                            )
                            .with_child(
                                Container::new(
                                    Text::new("Choose another, or make one", ui, DESCRIPTION_SIZE)
                                        .with_color(theme().text_muted)
                                        .finish(),
                                )
                                .with_margin_top(3.)
                                .finish(),
                            )
                            .finish(),
                    )
                    .with_margin_left(14.)
                    .finish(),
                )
                .finish(),
        )
        .with_border(Border::all(1.).with_border_color(border))
        // Ten rather than the eight this shape would otherwise take: a tab
        // chip is rounded by eight, and the workspace tests find the tabs in a
        // frame by exactly that radius. A settings row that answered to
        // `tab_boxes` would not fail a test, it would quietly become one of
        // the tabs those tests reason about.
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .with_uniform_padding(8.)
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
    .finish();

    // The gap under the row goes *outside* the `Hoverable`: a margin inside it
    // is part of the box the hit test is resolved against, so the row lit up —
    // and opened the panel — from ten pixels below where it is drawn.
    Entry {
        words: Some(words),
        value: Some(name_text),
        element: Container::new(row).with_margin_bottom(10.).finish(),
    }
}

/// A label and a value that cannot be edited, for the About page.
///
/// `monospace` is for the values that are paths: a settings file's location is
/// something a person copies into a shell, and proportional text turns runs of
/// slashes and dots into a smear.
///
/// A description, where the words carry one, goes on a second line under the
/// label, exactly as it does on a row with a control.
pub(crate) fn fact(
    words: Words,
    value: String,
    monospace: bool,
    fonts: super::super::view::Fonts,
) -> Entry {
    let family = if monospace { fonts.monospace } else { fonts.ui };
    let value_text = value.clone();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new(words.label.clone(), fonts.ui, LABEL_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_child(Expanded::new(1., Empty::new().finish()).finish())
                .with_child(
                    Text::new(value, family, if monospace { 10.5 } else { LABEL_SIZE })
                        .with_color(theme().text_primary)
                        .finish(),
                )
                .finish(),
        );

    // The second line every other row in this file already has. A fact used to
    // print its label and its value and drop the description on the floor,
    // which was invisible while the only facts were a version and a path — and
    // wrong the moment a page had something to say about a row it prints, like
    // the name a chord is bound by.
    if let Some(description) = words.description.clone() {
        column.add_child(description_text(description, fonts.ui));
    }

    Entry {
        words: Some(words),
        value: Some(value_text),
        element: Container::new(column.finish())
            .with_margin_bottom(10.)
            .finish(),
    }
}

/// A paragraph of explanation that belongs to a page rather than to a row.
///
/// Unsearchable, deliberately: a note explains the rows around it, so it goes
/// wherever its category goes and is left out the moment something is being
/// searched for.
pub(crate) fn note(text: &str, ui: FamilyId) -> Entry {
    Entry::unsearchable(paragraph(text, ui))
}

/// The wrapped, muted lines both of the above are made of.
fn paragraph(text: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text.to_owned(), ui, DESCRIPTION_SIZE)
            .with_color(theme().text_muted)
            .with_line_height_ratio(1.45)
            .finish(),
    )
    .with_margin_bottom(10.)
    .finish()
}

/// Attaches the click handler, or does not.
///
/// The one place a settings control learns to be clicked. A disabled control
/// is not a control with a handler that returns early — it has no handler at
/// all, so a press falls through to the page under it and nothing has to
/// remember to check an `enabled` flag a second time.
fn with_command(control: Hoverable, command: Command) -> Box<dyn Element> {
    match command {
        Some(action) => control
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
            .finish(),
        None => control.finish(),
    }
}
