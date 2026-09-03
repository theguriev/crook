//! The Claude Code usage chip: a pill in the header that clicks to refresh.
//!
//! Its own view rather than a few elements inside the header, for two reasons.
//! It needs an action type of its own, which keeps the workspace's enum about
//! tabs; and it observes the usage model itself, so a reading that lands
//! repaints a pill instead of the tab strip beside it. Warp's chip animates on
//! every background poll and re-renders the whole header 110ms at a time to do
//! it — this one only ever redraws when the number it prints has changed, or
//! when a person is waiting on it.

use crook_usage::{ClaudeUsageLevel, ClaudeUsageSnapshot};
use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::theme::theme;
use crate::usage_model::{UsageModel, UsageProblem};

use super::view::Fonts;

/// What the chip prints before the first reading lands. An en dash, not a
/// hyphen: a chip that showed nothing at all would read as a bug.
const UNREAD_LABEL: &str = "\u{2013}";

/// Keeps the pill from resizing as the label goes from "–" to "100%".
const MIN_WIDTH: f32 = 88.;

/// What the chip can be asked to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UsageChipAction {
    /// Read usage again, now, because a person asked.
    Refresh,
}

/// What the pill prints: a percentage when there is a usable one, and what is
/// wrong when there is not.
///
/// The poller keeps the last successful reading through a failure, so a chip
/// that preferred the snapshot unconditionally could never report a problem
/// again after its first success — "session expired" would be unreachable for
/// the rest of the run, with a percentage that stopped being refreshed sitting
/// where it used to be. Which of the two wins is therefore
/// [`UsageProblem::invalidates_the_reading`]'s decision, not the order of the
/// match arms.
fn pill_label(snapshot: Option<&ClaudeUsageSnapshot>, problem: Option<UsageProblem>) -> String {
    match (usable(snapshot, problem), problem) {
        (Some(snapshot), _) => format!("{}%", snapshot.session_percent_rounded()),
        (None, Some(problem)) => problem.chip_label().to_owned(),
        (None, None) => UNREAD_LABEL.to_owned(),
    }
}

/// What colour to print it in: the usage band while the reading is current,
/// and the muted grey the unread chip uses the moment it is not. A percentage
/// the last cycle failed to refresh is still the best answer available, and it
/// has to be visibly not a fresh one.
fn pill_color(snapshot: Option<&ClaudeUsageSnapshot>, problem: Option<UsageProblem>) -> Color {
    if problem.is_some() {
        return theme().text_muted;
    }

    match snapshot.map(ClaudeUsageSnapshot::level) {
        None | Some(ClaudeUsageLevel::Normal) => theme().usage_normal,
        Some(ClaudeUsageLevel::Elevated) => theme().usage_elevated,
        Some(ClaudeUsageLevel::High) => theme().usage_high,
        Some(ClaudeUsageLevel::Critical) => theme().usage_critical,
    }
}

/// The last reading, if the most recent failure has not made it meaningless.
fn usable(
    snapshot: Option<&ClaudeUsageSnapshot>,
    problem: Option<UsageProblem>,
) -> Option<&ClaudeUsageSnapshot> {
    snapshot.filter(|_| !problem.is_some_and(UsageProblem::invalidates_the_reading))
}

/// The header's usage indicator.
pub struct UsageChip {
    usage: ModelHandle<UsageModel>,
    fonts: Fonts,
    mouse: MouseStateHandle,
}

impl UsageChip {
    /// Builds the chip and subscribes it to the usage model.
    pub fn new(fonts: Fonts, ctx: &mut ViewContext<Self>) -> Self {
        let usage = UsageModel::handle(ctx);
        ctx.observe(&usage, |_, _, ctx| ctx.notify());

        Self {
            usage,
            fonts,
            mouse: MouseStateHandle::default(),
        }
    }
}

impl Entity for UsageChip {
    type Event = ();
}

impl View for UsageChip {
    fn ui_name() -> &'static str {
        "UsageChip"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let model = self.usage.as_ref(app);
        let label = pill_label(model.snapshot(), model.problem());
        let label_color = pill_color(model.snapshot(), model.problem());

        // Only a person's own click lights the border. A background poll that
        // did this would put the header on a repaint timer for no one.
        let outline = if model.is_busy_for_user() {
            theme().accent
        } else {
            theme().border
        };

        let ui = self.fonts.ui;

        Hoverable::new(self.mouse.clone(), move |state| {
            let background = if state.is_clicked() {
                theme().ground
            } else if state.is_hovered() {
                theme().tab_active
            } else {
                theme().surface
            };

            let content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new("claude", ui, 10.5)
                        .with_color(theme().text_muted)
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new(label, ui, 12.)
                            .with_color(label_color)
                            .with_style(Properties {
                                weight: Weight::Semibold,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .with_margin_left(6.)
                    .finish(),
                )
                .finish();

            ConstrainedBox::new(
                Container::new(content)
                    .with_background_color(background)
                    .with_border(Border::all(1.).with_border_color(outline))
                    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                    .with_padding(Padding {
                        top: 4.,
                        left: 11.,
                        bottom: 5.,
                        right: 11.,
                    })
                    .finish(),
            )
            .with_min_width(MIN_WIDTH)
            .finish()
        })
        .on_click(|_, ctx, _| ctx.dispatch_typed_action(UsageChipAction::Refresh))
        .finish()
    }
}

impl TypedActionView for UsageChip {
    type Action = UsageChipAction;

    fn handle_action(&mut self, _: &UsageChipAction, ctx: &mut ViewContext<Self>) {
        self.usage
            .update(ctx, |model, ctx| model.refresh_from_user(ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(percent: f32) -> ClaudeUsageSnapshot {
        ClaudeUsageSnapshot {
            session_percent: percent,
            session_resets_at: None,
            weekly_percent: None,
            extra_usage: None,
        }
    }

    #[test]
    fn a_chip_with_nothing_to_report_yet_says_so() {
        assert_eq!(pill_label(None, None), UNREAD_LABEL);
        assert_eq!(pill_color(None, None), theme().usage_normal);
    }

    #[test]
    fn a_reading_is_printed_in_the_colour_of_its_band() {
        assert_eq!(pill_label(Some(&snapshot(42.4)), None), "42%");
        assert_eq!(
            pill_color(Some(&snapshot(97.)), None),
            theme().usage_critical
        );
    }

    #[test]
    fn a_transient_failure_keeps_the_last_reading_and_marks_it_stale() {
        let last = snapshot(42.);

        assert_eq!(
            pill_label(Some(&last), Some(UsageProblem::Unreachable)),
            "42%",
            "a network blip is not a reason to throw away a number from a minute ago"
        );
        assert_eq!(
            pill_color(Some(&last), Some(UsageProblem::Unreachable)),
            theme().text_muted,
            "but it has to stop reading as current"
        );
    }

    #[test]
    fn a_dead_session_replaces_the_reading_it_will_never_refresh() {
        let last = snapshot(42.);

        for problem in [UsageProblem::SessionExpired, UsageProblem::NoSession] {
            assert_eq!(
                pill_label(Some(&last), Some(problem)),
                problem.chip_label(),
                "{problem:?} left a stale percentage on screen instead of reporting itself"
            );
        }
    }
}
