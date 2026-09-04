//! The panel under the pirate: the limits, and the week behind them.
//!
//! The chip has room for one number, and one number is not an answer to "am I
//! about to run out, and on what". This is the rest of it, hung under the chip
//! when it is clicked: the two limits Claude reports with the time left on
//! each, then what this machine actually did with the week — per model, per
//! day, per project — read from the transcripts Claude Code writes.
//!
//! # Two sources, and why the seam is visible
//!
//! The limits come from Claude and the week comes from the disk. They are not
//! the same measurement and they are not drawn as though they were: the top
//! block is what Anthropic says is left, the bottom block is what happened
//! here, and the divider between them is the honest place to put the
//! difference. A week's tokens do not add up to a percentage of a limit —
//! different windows, different weights, and a cache read is not priced like a
//! token the model wrote — so nothing here pretends to derive one from the
//! other.
//!
//! # Traffic, not money
//!
//! Every figure below is tokens. There is no dollar column, because the price
//! of a token is not in any of this feature's inputs: it would have to be a
//! table in the binary, wrong the week prices move, and meaningless to
//! everyone on a plan rather than on pay-as-you-go. What the panel does say —
//! output beside cache — is enough to see which model did the work.

use chrono::{Datelike as _, Utc};
use crook_usage::{HISTORY_DAYS, ModelUsage, ProjectUsage, UsageHistory};
use crookui_core::elements::{Clipped, Margin, Paragraph};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::theme::theme;
use crate::usage_model::UsageModel;

/// How wide the panel is. Wider than the options menu's 200: this one carries
/// a figure and a label on one line, and a week of bars across another.
pub(super) const PANEL_WIDTH: f32 = 280.;

/// Its corner, matching the options menu's.
const PANEL_RADIUS: f32 = 6.;

/// The inset every row shares.
const INSET: f32 = 14.;

/// How wide anything that spans the panel is: a bar, a chart, a rule.
const SPAN: f32 = PANEL_WIDTH - 2. * INSET;

/// Ordinary label size, matching the options menu.
const LABEL_SIZE: f32 = 12.;

/// The quieter size, for what a label is measured in.
const NOTE_SIZE: f32 = 10.5;

/// A section title's size, and the size of the figure beside it.
const SECTION_SIZE: f32 = 10.5;

/// A limit bar's height, and its corner.
const BAR_HEIGHT: f32 = 5.;

/// How tall the tallest day of the week is drawn.
const DAY_CHART_HEIGHT: f32 = 34.;

/// Between one day's column and the next.
const DAY_GAP: f32 = 3.;

/// How wide one day's bar is.
///
/// Fixed, and much narrower than the seventh of the panel each day is given:
/// a bar as wide as it is tall is a block, and seven blocks in a row read as a
/// bar chart of nothing. The space around it is what makes the shape legible.
const DAY_BAR_WIDTH: f32 = 13.;

/// A day with nothing in it still gets this much, so that the week reads as
/// seven days rather than as however many were busy.
const DAY_FLOOR: f32 = 2.;

/// The whole panel.
pub(super) fn render(usage: &UsageModel, ui: FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for element in limits(usage, ui) {
        column.add_child(element);
    }

    column.add_child(divider());
    for element in week(usage, ui) {
        column.add_child(element);
    }

    ConstrainedBox::new(
        // The options menu's chrome, for the same reason it has it: there is
        // no shadow in the shader, so an opaque ground and a hairline are what
        // separate a floating panel from the header behind it.
        Container::new(column.finish())
            .with_vertical_padding(10.)
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PANEL_RADIUS)))
            .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// What Claude says is left: the session, the week, and any credits past them.
fn limits(usage: &UsageModel, ui: FamilyId) -> Vec<Box<dyn Element>> {
    let Some(snapshot) = usage.snapshot() else {
        // No reading at all: say why in a sentence rather than drawing two
        // empty bars, which would read as "nothing used yet".
        return vec![note(
            usage
                .problem()
                .map(|problem| problem.message().to_owned())
                .unwrap_or_else(|| "Reading Claude usage\u{2026}".to_owned()),
            ui,
        )];
    };

    let now = Utc::now();
    let mut rows: Vec<Box<dyn Element>> = Vec::new();

    rows.push(limit(
        "Session",
        snapshot.session_percent,
        snapshot
            .time_until_session_reset(now)
            .map(|left| format!("resets in {left}")),
        ui,
    ));

    if let Some(weekly) = snapshot.weekly_percent {
        rows.push(limit(
            "Week",
            weekly,
            snapshot
                .time_until_weekly_reset(now)
                .map(|left| format!("resets in {left}")),
            ui,
        ));
    }

    if let Some(extra) = snapshot.extra_usage {
        rows.push(figure_row(
            "Extra usage",
            &format!(
                "${:.0} of ${:.0}",
                extra.used_credits / 100.,
                extra.monthly_limit / 100.
            ),
            theme().text_primary,
            ui,
        ));
    }

    // A reading that failed to refresh is still drawn — it is the best number
    // there is — with the failure named underneath, which is the same rule the
    // chip follows when it greys the percentage out.
    if let Some(problem) = usage.problem() {
        rows.push(note(problem.message().to_owned(), ui));
    }

    rows
}

/// One limit: its name, its percentage, a bar, and what is left of its window.
fn limit(label: &str, percent: f32, resets: Option<String>, ui: FamilyId) -> Box<dyn Element> {
    let percent = percent.clamp(0., 100.);
    let color = crate::plugins::usage::chip::band_color(percent);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(figure_row(
            label,
            &format!("{}%", percent.round()),
            color,
            ui,
        ))
        .with_child(
            Container::new(bar(percent / 100., color))
                .with_horizontal_padding(INSET)
                .with_margin_top(5.)
                .finish(),
        );

    if let Some(resets) = resets {
        column.add_child(
            Container::new(
                Text::new(resets, ui, NOTE_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_horizontal_padding(INSET)
            .with_margin_top(4.)
            .finish(),
        );
    }

    Container::new(column.finish())
        .with_margin_bottom(8.)
        .finish()
}

/// What the transcripts say the week was spent on.
fn week(usage: &UsageModel, ui: FamilyId) -> Vec<Box<dyn Element>> {
    let Some(history) = usage.history() else {
        return vec![
            section("Last 7 days", None, ui),
            note(
                if usage.is_reading_history() {
                    "Reading this machine's transcripts\u{2026}".to_owned()
                } else {
                    "No transcripts read yet".to_owned()
                },
                ui,
            ),
        ];
    };

    if history.is_empty() {
        return vec![
            section("Last 7 days", None, ui),
            note(
                "Nothing in the last 7 days. Crook counts the turns Claude Code writes to this \
                 machine."
                    .to_owned(),
                ui,
            ),
        ];
    }

    let total = history.tokens();
    let mut rows: Vec<Box<dyn Element>> = vec![
        section("Last 7 days", Some(&compact(total)), ui),
        Container::new(day_chart(history, ui))
            .with_horizontal_padding(INSET)
            .with_margin_bottom(10.)
            .finish(),
    ];

    for model in &history.by_model {
        rows.push(model_row(model, total, ui));
    }

    if !history.projects.is_empty() {
        rows.push(divider());
        rows.push(section("Projects", None, ui));
        for project in &history.projects {
            rows.push(project_row(project, ui));
        }
    }

    rows.push(divider());
    rows.push(note(
        format!(
            "{} turns \u{00b7} {} sessions \u{00b7} {} models",
            thousands(history.requests),
            thousands(history.sessions),
            history.by_model.len()
        ),
        ui,
    ));

    rows
}

/// The week as seven columns, today on the right.
///
/// Heights are against the busiest day rather than against a fixed ceiling:
/// what a person reads off this is the *shape* of their week — which day the
/// long session was on — and a chart scaled to an absolute number would be
/// flat for everyone but the heaviest user of all.
fn day_chart(history: &UsageHistory, ui: FamilyId) -> Box<dyn Element> {
    let busiest = history
        .by_day
        .iter()
        .map(|day| day.tokens)
        .max()
        .unwrap_or_default()
        .max(1);
    let today = Utc::now().with_timezone(&chrono::Local).date_naive();
    let width = (SPAN - DAY_GAP * (HISTORY_DAYS as f32 - 1.)) / HISTORY_DAYS as f32;

    let mut chart = Flex::row()
        .with_spacing(DAY_GAP)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for day in &history.by_day {
        let is_today = day.date == today;
        let filled = (DAY_CHART_HEIGHT * day.tokens as f32 / busiest as f32).max(DAY_FLOOR);
        let color = if is_today {
            theme().accent
        } else {
            theme().overlay_3
        };

        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_background_color(color)
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.)))
                        .finish(),
                )
                .with_width(DAY_BAR_WIDTH)
                .with_height(filled)
                .finish(),
            );
        column.add_child(
            Container::new(
                Text::new(weekday_initial(day.date), ui, NOTE_SIZE)
                    .with_color(if is_today {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .with_margin_top(3.)
            .finish(),
        );

        chart.add_child(
            ConstrainedBox::new(column.finish())
                .with_width(width)
                .finish(),
        );
    }

    chart.finish()
}

/// One model: what it is called, what it moved, and its share of the week.
fn model_row(model: &ModelUsage, total: u64, ui: FamilyId) -> Box<dyn Element> {
    let share = if total == 0 {
        0.
    } else {
        model.tokens() as f32 / total as f32
    };

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(figure_row(
                &model.display_name(),
                &format!("{}%", (share * 100.).round()),
                theme().text_primary,
                ui,
            ))
            .with_child(
                Container::new(
                    Text::new(
                        format!(
                            "{} out \u{00b7} {} cache \u{00b7} {} turns",
                            compact(model.output),
                            compact(model.cache_read + model.cache_write),
                            thousands(model.requests)
                        ),
                        ui,
                        NOTE_SIZE,
                    )
                    .with_color(theme().text_muted)
                    .finish(),
                )
                .with_horizontal_padding(INSET)
                .with_margin_top(2.)
                .finish(),
            )
            .with_child(
                Container::new(bar(share, theme().accent))
                    .with_horizontal_padding(INSET)
                    .with_margin_top(5.)
                    .finish(),
            )
            .finish(),
    )
    .with_margin_bottom(8.)
    .finish()
}

/// One project: the directory, the branch most of it was on, and its tokens.
fn project_row(project: &ProjectUsage, ui: FamilyId) -> Box<dyn Element> {
    let label = elided(&match project.branch.as_deref() {
        Some(branch) => format!("{} \u{00b7} {branch}", project.name),
        None => project.name.clone(),
    });

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    // A long branch name is cut rather than allowed to push
                    // the figure off the panel: which project it is reads from
                    // the start of the name, and the number is the point of
                    // the row.
                    Clipped::new(
                        Text::new(label, ui, LABEL_SIZE)
                            .with_color(theme().text_primary)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(compact(project.tokens), ui, LABEL_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            )
            .finish(),
    )
    .with_horizontal_padding(INSET)
    .with_margin_bottom(5.)
    .finish()
}

/// How many characters of a row's label fit beside its figure.
///
/// Counted rather than measured: the interface font is proportional and the
/// element tree has no measurement to offer until layout, which is after this
/// string is built. A character averages a little over half its size, so a
/// 12px label has about this many across the panel's span minus the figure —
/// and being a character or two out costs an ellipsis where none was needed,
/// which is the harmless direction.
const LABEL_CHARS: usize = 26;

/// `label`, cut to [`LABEL_CHARS`] with an ellipsis when it is longer.
///
/// A branch name is the part that runs long, and it is cut at the end rather
/// than through the middle: `worktree-terminal-features` and
/// `worktree-terminal-search` differ at the end, but a row that started with
/// an ellipsis would not say which project it was at all.
fn elided(label: &str) -> String {
    if label.chars().count() <= LABEL_CHARS {
        return label.to_owned();
    }

    let kept: String = label.chars().take(LABEL_CHARS - 1).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// A label on the left and a figure on the right, both on one line.
fn figure_row(label: &str, figure: &str, color: Color, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Clipped::new(
                        Text::new(label.to_owned(), ui, LABEL_SIZE)
                            .with_color(theme().text_primary)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(figure.to_owned(), ui, LABEL_SIZE)
                        .with_color(color)
                        .with_style(Properties {
                            weight: Weight::Semibold,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_margin_left(8.)
                .finish(),
            )
            .finish(),
    )
    .with_horizontal_padding(INSET)
    .finish()
}

/// A section title, with the section's own total beside it when it has one.
fn section(title: &str, figure: Option<&str>, ui: FamilyId) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Expanded::new(
                1.,
                Text::new(title.to_owned(), ui, SECTION_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .finish(),
        );

    if let Some(figure) = figure {
        row.add_child(
            Text::new(figure.to_owned(), ui, SECTION_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        );
    }

    Container::new(row.finish())
        .with_horizontal_padding(INSET)
        .with_margin_bottom(6.)
        .finish()
}

/// A sentence, wrapped, for the states that are a sentence rather than a
/// figure.
fn note(text: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text, ui, NOTE_SIZE)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_horizontal_padding(INSET)
    .with_margin_bottom(4.)
    .finish()
}

/// A filled track: `fraction` of the panel's span, in `color`.
fn bar(fraction: f32, color: Color) -> Box<dyn Element> {
    let filled = (SPAN * fraction.clamp(0., 1.)).max(0.);

    ConstrainedBox::new(
        Container::new(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BAR_HEIGHT / 2.)))
                    .finish(),
            )
            .with_width(filled)
            .finish(),
        )
        .with_background_color(theme().overlay_2)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(BAR_HEIGHT / 2.)))
        .finish(),
    )
    .with_height(BAR_HEIGHT)
    .finish()
}

/// A hairline between sections, full-bleed like the options menu's.
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
    .with_margin(Margin {
        top: 2.,
        bottom: 10.,
        ..Margin::default()
    })
    .finish()
}

/// The one letter a day of the week is drawn under.
fn weekday_initial(date: chrono::NaiveDate) -> String {
    match date.weekday() {
        chrono::Weekday::Mon => "M",
        chrono::Weekday::Tue => "T",
        chrono::Weekday::Wed => "W",
        chrono::Weekday::Thu => "T",
        chrono::Weekday::Fri => "F",
        chrono::Weekday::Sat => "S",
        chrono::Weekday::Sun => "S",
    }
    .to_owned()
}

/// A token count as a person reads it: 984k, 6.2M, 3.0B.
///
/// Three significant figures at most, because the panel is 264 pixels wide and
/// the difference between 2,949,905,892 and 2.9B is not a difference anybody
/// is going to act on.
pub(super) fn compact(tokens: u64) -> String {
    const THOUSAND: f64 = 1_000.;
    let tokens = tokens as f64;

    for (limit, suffix) in [
        (THOUSAND.powi(3), "B"),
        (THOUSAND.powi(2), "M"),
        (THOUSAND, "k"),
    ] {
        if tokens >= limit {
            let scaled = tokens / limit;
            // One decimal below ten, none above it: 9.4M, then 12M.
            return if scaled < 10. {
                format!("{scaled:.1}{suffix}")
            } else {
                format!("{scaled:.0}{suffix}")
            };
        }
    }

    format!("{tokens:.0}")
}

/// A count with its thousands separated, for the figures that are counts of
/// things rather than of tokens.
pub(super) fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }

    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_count_is_read_at_a_glance_or_it_is_not_read() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(942), "942");
        assert_eq!(compact(9_400), "9.4k");
        assert_eq!(compact(48_300), "48k");
        assert_eq!(compact(5_047_886), "5.0M");
        assert_eq!(compact(2_949_905_892), "2.9B");
    }

    #[test]
    fn a_count_of_things_keeps_its_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(30), "30");
        assert_eq!(thousands(6_886), "6,886");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_label_too_long_for_its_row_ends_in_an_ellipsis() {
        assert_eq!(elided("crook \u{00b7} main"), "crook \u{00b7} main");
        assert_eq!(
            elided("terminal-features \u{00b7} worktree-terminal-features"),
            "terminal-features \u{00b7} workt\u{2026}"
        );
    }

    #[test]
    fn a_weekday_is_one_letter() {
        let monday = chrono::NaiveDate::from_ymd_opt(2026, 8, 31).expect("a real date");
        assert_eq!(weekday_initial(monday), "M");
        assert_eq!(weekday_initial(monday.succ_opt().expect("a next day")), "T");
    }
}
