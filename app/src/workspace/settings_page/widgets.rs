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
//! And, because the plugin cards are drawn out of the same pieces, the shapes
//! a *card* needs that a settings page does not. A **facts line** with the
//! **dot** the plugins list draws, so the state under a name and the state
//! beside it in the list are one mark. A **box** ([`asked`]) in which a
//! question, its terms and the row that answers it share one fill, because a
//! control four things away from what it decides is a control pressed unread;
//! an **item** for one term of that question, marked by the same dot; and a
//! **footnote** under the box for the mechanism. The category **heading**
//! comes in two tones, so that a warning is the same heading in the one
//! signal colour rather than a note that happens to be red. They are here
//! rather than on either card because the Store's card and a plugin's are
//! read one after the other, and two boxes differing by a couple of pixels
//! would read as two mechanisms.
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

/// The check on the chosen row of a picker.
const CHECK_SIZE: f32 = 13.;

/// The gap under one row, before the next one's label — and under the last
/// box on a card, before the first rule, so that the rule sits above a box
/// exactly where it sits above the last row of a category.
pub(crate) const ROW_SPACING: f32 = 14.;

/// The gap a note ends in, before whatever is next: another note, a rule, or
/// a box.
const NOTE_GAP: f32 = 10.;

/// The gap between a box and the paragraph that explains it.
///
/// Closer to its box than the next box is, so that it reads as the box's
/// footnote rather than as the next thing.
const FOOTNOTE_GAP: f32 = 6.;

/// How far a description stops short of the right edge, so that it wraps well
/// clear of the control beside its first line rather than under it.
const DESCRIPTION_RIGHT_MARGIN: f32 = 96.;

/// The switch's track.
const SWITCH_WIDTH: f32 = 28.;
const SWITCH_HEIGHT: f32 = 16.;

/// The knob inside it, and the space either side of it.
const KNOB_SIZE: f32 = 12.;
const KNOB_INSET: f32 = 2.;

/// The corner radius of a segmented control's track and of a button.
const CONTROL_RADIUS: f32 = 6.;

/// The height a row that answers a question is held to, whatever control is
/// in it.
///
/// A switch is 16 tall and an outlined button 23, and until this existed the
/// box around each took its height from whichever it held: the Enabled box on
/// a plugin's card was 36 tall and the box under it 43, two boxes that
/// [`asked`] promises are one shape. Twenty-four holds both with air to
/// spare. Not twenty, which would make the box 40: the theme creator's tests
/// find a swatch by "any filled rect exactly 40 tall", and a box that answered
/// to that would become a swatch the day the two share a frame.
const CONTROL_LANE: f32 = 24.;

/// The gap between two things inside a box, and under a heading.
///
/// Six between items, and ten under a heading — the ten a category keeps
/// under its own, so a heading inside a box and a heading over a category
/// hold their lines at the same distance.
const BODY_GAP: f32 = 6.;
/// See [`BODY_GAP`].
const HEADING_GAP: f32 = 10.;

/// The dot that says whether something is running.
const DOT: f32 = 6.;

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

/// The colour a heading speaks in.
///
/// Two, not a ladder. A heading is quiet or it is a warning, and the Store
/// already says what went wrong in the one colour a theme calls red; three
/// rare headings in three shades would be a scale nobody learns.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Over a category of settings, and over anything else that is merely
    /// being described.
    Plain,
    /// Something is wrong with what is under it: a version the registry took
    /// back, a plugin that did not load.
    Warning,
}

/// The colour a tone is drawn in, for a heading and for a note alike.
fn ink(tone: Tone) -> Color {
    match tone {
        Tone::Plain => theme().text_muted,
        Tone::Warning => theme().usage_critical,
    }
}

/// A heading on its own, with nothing above or below it.
///
/// The bare text [`category_element`] draws over a category, taken out so
/// that the same heading can open a box: a box that opened with a heading of
/// its own would be a second kind of heading on the page, and a warning has to
/// be this heading in the one signal colour rather than a note that happens
/// to be red.
pub(crate) fn heading(title: &str, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
    Text::new(title.to_owned(), ui, CATEGORY_SIZE)
        .with_color(ink(tone))
        .with_style(Properties {
            weight: Weight::Semibold,
            ..Properties::default()
        })
        .finish()
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
    toned_category(title, Tone::Plain, first, rows, ui)
}

/// The same, with the heading in whatever colour it deserves.
///
/// Beside [`category_element`] rather than a parameter on it, because every
/// category of settings is plain, and a signature that made forty call sites
/// say so would be forty copies of one word.
pub(crate) fn toned_category(
    title: &str,
    tone: Tone,
    first: bool,
    rows: Vec<Box<dyn Element>>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    if !first {
        column.add_child(
            Container::new(hairline(theme().border))
                .with_margin_top(6.)
                .with_margin_bottom(18.)
                .finish(),
        );
    }

    column.add_child(
        Container::new(heading(title, tone, ui))
            .with_margin_bottom(HEADING_GAP)
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
///
/// Wrapped rather than cut off. This was a `Text`, which loses its last words
/// wherever a description is long — under a slot that lists eight entries, or
/// in the window the docked Themes panel leaves — and at the line height a
/// `Text` draws one line at, so a description that fitted draws the pixels it
/// drew.
fn description_text(description: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(description, ui, DESCRIPTION_SIZE)
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

/// The box a decision about a plugin is made in: what is being asked, and the
/// row that answers it, in one fill.
///
/// One shape for every one of them, and there are now five: is it on, may it
/// do this, what is it doing, do you want it, and do you want it gone. Two
/// boxes differing by a couple of pixels would read as two mechanisms, which
/// is why this is here rather than on either card — the Store and the Plugins
/// page are read one after the other.
///
/// It used to be the answer row alone, and what it answered was drawn above it
/// on the bare page — a heading, a list of lines, a paragraph, and then the
/// box — so the control was four things away from what it decided, and a
/// plugin's own row under it was a second box touching the first. This is the
/// same box grown to hold the question: a heading, a body of whatever the
/// decision is about, a hairline seam, and the foot. What a person has to
/// read before pressing anything is inside the rectangle the button is in.
/// A box with nothing to read is a foot on its own, which is what the Enabled
/// box is on most cards — and on the rest it holds the reason the switch is
/// what it is, above the switch.
///
/// The body column owns every gap — an item, a note and a plugin's row all sit
/// ten pixels above the seam, and the foot's label ten below it — so a body
/// may end in anything. The box has a fill and no border, and a foot row has
/// neither, so nothing here has `overlay_1` and a border at once: that is the
/// shape a text field is found by, and a box that matched it would be one the
/// tests pressed as a field.
pub(crate) fn asked(
    question: Option<(&str, Tone)>,
    body: Vec<Box<dyn Element>>,
    foot: Vec<Box<dyn Element>>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    let has_body = question.is_some() || !body.is_empty();
    if has_body {
        let mut inside = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(BODY_GAP);
        if let Some((title, tone)) = question {
            // Its own margin plus the column's gap under it: the ten a
            // category keeps under its heading.
            inside.add_child(
                Container::new(heading(title, tone, ui))
                    .with_margin_bottom(HEADING_GAP - BODY_GAP)
                    .finish(),
            );
        }
        inside.add_children(body);
        column.add_child(
            Container::new(inside.finish())
                .with_padding(ANSWER_PADDING)
                .finish(),
        );
    }

    for (index, row) in foot.into_iter().enumerate() {
        if index > 0 || has_body {
            column.add_child(seam());
        }
        column.add_child(row);
    }

    Container::new(column.finish())
        .with_background_color(theme().overlay_1)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ANSWER_RADIUS)))
        .finish()
}

/// The row at the foot of an [`asked`] box: a label, and the control that
/// answers it, hard against the right edge.
///
/// Held to [`CONTROL_LANE`] by a post of no width standing in the row rather
/// than by a minimum height on a box around it: a flex centres its children
/// on its own cross size, and a box's minimum is not the flex's size — the
/// reason `choice` draws its check in a transparent colour rather than leaving
/// it out.
///
/// `live` is whether the control does anything, and it is the *label* that
/// says so: a muted word beside a control that cannot be pressed is the only
/// hint there is, since a disabled control carries no handler at all.
pub(crate) fn answer_row(
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
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(0.)
                    .with_height(CONTROL_LANE)
                    .finish(),
            )
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
    .with_padding(ANSWER_PADDING)
    .finish()
}

/// The hairline between a box's body and its foot, and between two feet.
///
/// The column it sits in stretches it to the box's edge, so it runs the full
/// width with no negative margin; and it is a fill rather than a border, so
/// nothing that looks for a bordered box can see it.
///
/// `overlay_2` and not `border`, though the category rule is `border`: a
/// theme flattens `border` against its surface, and on a light theme that
/// comes out *lighter* than a box on the receded ground — a seam that was
/// drawn in it vanished there. A hairline inside a fill has to be the
/// foreground at a percentage over whatever it lies on, which is what the
/// overlay ladder is, and `overlay_2` is already what divides a menu.
fn seam() -> Box<dyn Element> {
    hairline(theme().overlay_2)
}

/// One pixel of `color`, as wide as whatever holds it.
fn hairline(color: Color) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.)
            .finish(),
    )
    .with_background_color(color)
    .finish()
}

/// Where one line of a list stands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Not yet allowed, or not yet decided: a hollow dot and lit text.
    Open,
    /// In force: a filled dot and lit text.
    Settled,
    /// In force, beside lines that are not: a filled dot and muted text, so
    /// that the open lines are the only lit ones in the list.
    Faded,
}

/// One term of what a box asks about, with the dot that says where it stands.
///
/// Filled means in force and hollow means not, which is what the dot means on
/// the row of the list two inches to the left. A check mark would spend the
/// accent once per line, and a card that spent it a dozen times would have
/// none left for the switch.
///
/// The sentence is a `Paragraph` inside an `Expanded`, in a row, which this
/// file otherwise avoids — and it holds here for a reason `flex.rs` states: a
/// flexible child is handed what is left of a *bounded* axis as its maximum,
/// and a card's column is stretched to the measure, so a sentence wider than
/// the box breaks at the box's inner edge and its second line sits under the
/// first line's text rather than under the dot. A plugin's manifest names
/// hosts and paths, and a line that could not wrap would be cut off at
/// whichever host came last.
pub(crate) fn item(text: &str, mark: Mark, ui: FamilyId) -> Box<dyn Element> {
    let color = match mark {
        Mark::Open | Mark::Settled => theme().text_primary,
        Mark::Faded => theme().text_muted,
    };
    // On the first line rather than in the paragraph's middle, so a sentence
    // that wraps keeps its dot beside its first words.
    let drop = (LABEL_SIZE * SENTENCE_LINE_HEIGHT - DOT) / 2.;

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(
            Container::new(state_dot(mark != Mark::Open))
                .with_margin_top(drop)
                .with_margin_right(super::super::section::LEADING_GAP)
                .finish(),
        )
        .with_child(
            Expanded::new(
                1.,
                Paragraph::new(text.to_owned(), ui, LABEL_SIZE)
                    .with_color(color)
                    .with_line_height_ratio(SENTENCE_LINE_HEIGHT)
                    .finish(),
            )
            .finish(),
        )
        .finish()
}

/// The line height of a card's [`description`] and of an [`item`] in a box
/// under it: the same sentences at the same size, so a term and the sentence
/// above it hold their lines at one distance.
const SENTENCE_LINE_HEIGHT: f32 = 1.5;

/// What a plugin says it is for, under its facts and above its first box.
///
/// The one sentence on a card in the plugin's own words; everything under it
/// is the host's. Here rather than on either card because the two cards are
/// read one after the other.
pub(crate) fn description(text: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text.to_owned(), ui, LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_line_height_ratio(SENTENCE_LINE_HEIGHT)
            .finish(),
    )
    .with_margin_bottom(IDENTITY_GAP)
    .finish()
}

/// Filled for something that is running, hollow for something that is not.
///
/// The plugins list draws one before every row, and a card draws the same one
/// before the plugin's state and before every line of what it may do — from
/// one function, so that "filled means in force" is one rule the page cannot
/// contradict. The filled one is `usage_normal`, which a theme derives exactly
/// as it derives muted text: a signal at rest is grey, and the accent is kept
/// for what the window is doing.
pub(crate) fn state_dot(on: bool) -> Box<dyn Element> {
    let (background, border) = if on {
        (theme().usage_normal, theme().usage_normal)
    } else {
        (Color::TRANSPARENT, theme().text_muted)
    };

    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(background)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(DOT)
    .with_height(DOT)
    .finish()
}

/// The line under a card's title: the state the thing is in, and the facts
/// that do not change.
///
/// The state goes first, with the list's own dot in front of it, because it is
/// what the card is opened for — it used to be the last word of a muted
/// sentence, after the id and the version, where the eye arrives after reading
/// three things it did not come to read. The Store's cards have no state to
/// lead with and pass `None`; both cards are drawn through this so that the
/// line under a name is one line on both.
///
/// Two `Text`s rather than one because they are two colours, and an
/// `Expanded` around the tail because a flex measures an inflexible child free
/// along its axis, and an id somebody else chose could otherwise run past the
/// measure.
pub(crate) fn facts(state: Option<(bool, &str)>, rest: &str, ui: FamilyId) -> Box<dyn Element> {
    let mut line = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);

    let tail = match state {
        Some((on, word)) => {
            line.add_child(
                Container::new(state_dot(on))
                    .with_margin_right(super::super::section::LEADING_GAP)
                    .finish(),
            );
            line.add_child(
                Text::new(word.to_owned(), ui, DESCRIPTION_SIZE)
                    .with_color(if on {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            );
            format!(" \u{b7} {rest}")
        }
        None => rest.to_owned(),
    };
    line.add_child(
        Expanded::new(
            1.,
            Text::new(tail, ui, DESCRIPTION_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        )
        .finish(),
    );

    Container::new(line.finish())
        .with_margin_bottom(FACTS_GAP)
        .finish()
}

/// The gap under the facts line, before the description: the two are one
/// group, and read closer together than either is to the first box.
const FACTS_GAP: f32 = 8.;

/// The gap under a card's description, before its first box.
///
/// Wider than the gap between the facts and the description, which are one
/// group, and wider than the gap between two boxes, which are one stack: the
/// eye has to see that the sentence and the switch are different kinds of
/// thing. Here rather than on either card, because the Store's card and the
/// plugin's are read one after the other.
pub(crate) const IDENTITY_GAP: f32 = 16.;

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
        // `overlay_3` rather than `border`, for the reason the seam in a box
        // is not `border` either: a theme flattens `border` against its
        // surface, and inside a box on a light theme that came out lighter
        // than the fill around it, so Allow and Revoke were bare words. The
        // foreground at 15% is the same weight `border` has on the ground of
        // a dark theme and reads on every ground of a light one.
        .with_border(Border::all(1.).with_border_color(if enabled {
            theme().overlay_3
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
        // `overlay_3` for the outline, as on [`text_button`], and for the same
        // reason.
        let (background, border, color) = if recording {
            (theme().overlay_2, theme().accent, theme().text_primary)
        } else if !enabled {
            (Color::TRANSPARENT, theme().overlay_3, theme().text_muted)
        } else if mouse.is_hovered() {
            (theme().overlay_2, theme().overlay_3, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().overlay_3, theme().text_primary)
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
    Entry::unsearchable(
        Container::new(note_text(text, ui))
            .with_margin_bottom(NOTE_GAP)
            .finish(),
    )
}

/// The wrapped, muted lines a note is made of, with nothing around them.
///
/// Inside an [`asked`] box the column owns the gaps, and a note that brought
/// its own ten pixels would end the box in twenty of air.
pub(crate) fn note_text(text: &str, ui: FamilyId) -> Box<dyn Element> {
    toned_note(text, Tone::Plain, ui)
}

/// The same lines, in whatever colour they deserve: the Store's line about
/// what just went wrong is this paragraph in the warning's colour.
fn toned_note(text: &str, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
    Paragraph::new(text.to_owned(), ui, DESCRIPTION_SIZE)
        .with_color(ink(tone))
        .with_line_height_ratio(1.45)
        .finish()
}

/// The paragraph under an [`asked`] box: the mechanism, which explains the
/// decision and is not part of it.
///
/// Under the box rather than in it, because the card exists so that a person
/// reads the terms before answering, and a paragraph inside the box stood
/// between the terms and the button. It sits closer to its box than the next
/// box does, and ends in the gap a note ends in everywhere else.
pub(crate) fn footnote(text: &str, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
    Container::new(toned_note(text, tone, ui))
        .with_margin_top(FOOTNOTE_GAP)
        .with_margin_bottom(NOTE_GAP)
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
