//! The Claude Code usage chip: a pirate in the header that clicks to refresh.
//!
//! Its own view rather than a few elements inside the header, for two reasons.
//! It needs an action type of its own, which keeps the workspace's enum about
//! tabs; and it observes the usage model itself, so a reading that lands
//! repaints a pill instead of the tab strip beside it.
//!
//! # The pirate
//!
//! The mark is Warp's: a Pac-Man in an eyepatch, whose mouth says whether
//! anything is happening. Two [`Art`] layers — a yellow head and the black on
//! it — because a rasterized mark is a coverage mask and a mask has one
//! colour; they are stacked here and nothing else in the tree knows the chip
//! draws a picture rather than an icon.
//!
//! # What the mouth means
//!
//! It chomps while a person is waiting on a reading they asked for, and only
//! then. Warp's chip animates on every background poll and re-renders the
//! whole header 110ms at a time to do it, which spends a repaint timer on
//! nobody — the number it is drawing has not changed and nobody is looking.
//! The rule the model already keeps ([`UsageModel::is_busy_for_user`]) is the
//! same one the animation keeps: a background poll is invisible, a click is
//! not.
//!
//! The cycle ends where it starts, on a shut mouth, so a pirate whose refresh
//! finished mid-bite is never left frozen half-open.
//!
//! # The pill
//!
//! There is no outline around it. The header it sits in is `surface` and so
//! was the pill's resting ground, which left the border drawing a box around
//! a pirate for no reason other than that it could — the mark reads as a
//! button on its own. What is left is a ground that appears under the cursor
//! and darkens on the press, so the chip is a pirate until someone reaches
//! for it. The refresh a click asks for is still visible without a line
//! lighting up: that is what the chomping mouth is for.
//!
//! # What a click does
//!
//! It opens [`panel`], and on the way asks for both halves of what the panel
//! shows: a fresh reading from Claude, and — unless one was read in the last
//! minute — a fresh scan of the local transcripts. So a click still refreshes,
//! which is what it did when the chip was the whole feature, and now it also
//! shows the reader what the percentage is made of.
//!
//! The panel is anchored to the chip's right edge rather than its left: the
//! chip is the last thing in the header, so a panel hung leftwards is the only
//! one that opens over the window instead of over its edge.

use std::time::Duration;

use crook_usage::{ClaudeUsageLevel, ClaudeUsageSnapshot};
use crookui_core::elements::{AnchorTo, Corner, Dismiss, MouseStateHandle, Padding, Stack};
use crookui_core::fonts::{Properties, Weight};
use crookui_core::icons::{Art, Chomp};
use crookui_core::prelude::*;

use crate::theme::theme;
use crate::usage_model::{UsageModel, UsageProblem};

use crate::workspace::Fonts;

use super::panel;

/// What the chip prints before the first reading lands. An en dash, not a
/// hyphen: a chip that showed nothing at all would read as a bug.
const UNREAD_LABEL: &str = "\u{2013}";

/// What the percentage is given, so that the pill does not resize — and the
/// pirate does not shuffle sideways — as the reading goes from "9%" to "100%".
///
/// On the label rather than on the pill, which is where it used to be: a pill
/// with a floor of its own leaves a short label sitting in the middle of a box
/// with a gap after it, and the gap is the thing a reader notices. Reserving
/// the space where the digits actually vary keeps the chip the same size and
/// the pirate in the same place, and the states that do not fit — "session
/// expired" — grow the pill, which is what a state that is not a percentage
/// should do.
const LABEL_WIDTH: f32 = 30.;

/// How big the pirate is drawn, a little over the 12pt label beside it. The
/// artwork is a disc that fills its box edge to edge, where an icon of the
/// same nominal size is a stroke inside two units of margin, so matching the
/// numbers would draw a pirate that towered over everything else in the row.
const PIRATE_SIZE: f32 = 15.;

/// Between the pirate and the percentage.
const PIRATE_GUTTER: f32 = 5.;

/// The pirate's yellow, and the black of the patch and the strap.
///
/// Constants rather than theme roles: this is a piece of artwork, like a logo,
/// and a pirate whose face took the palette's cast would stop being the mark
/// people recognise. The one thing the theme does decide is what a *stale*
/// pirate looks like — see [`pill_color`].
const PIRATE_FACE: Color = Color::hex(0xf9d949);
const PIRATE_INK: Color = Color::hex(0x151515);

/// How long one frame of the chomp is held.
const CHOMP_FRAME: Duration = Duration::from_millis(110);

/// The bite, in order. It returns to a shut mouth so that stopping on any
/// frame boundary stops on a whole face.
const CHOMP_CYCLE: [Chomp; 4] = [Chomp::Shut, Chomp::Open, Chomp::Wide, Chomp::Open];

/// What the chip can be asked to do.
///
/// One variant, because clicking the chip does one thing: it opens the panel,
/// and the panel is what asks for a reading. Refreshing without opening
/// anything is still reachable — it is `crook/usage/refresh`, which goes to
/// the model directly rather than through the chip.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UsageChipAction {
    /// Show the panel, or take it down.
    TogglePanel,
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
        None => theme().usage_normal,
        Some(level) => level_color(level),
    }
}

/// The colour of a usage band.
fn level_color(level: ClaudeUsageLevel) -> Color {
    match level {
        ClaudeUsageLevel::Normal => theme().usage_normal,
        ClaudeUsageLevel::Elevated => theme().usage_elevated,
        ClaudeUsageLevel::High => theme().usage_high,
        ClaudeUsageLevel::Critical => theme().usage_critical,
    }
}

/// The colour a percentage is drawn in, wherever it is drawn.
///
/// The panel's bars go through here rather than through a table of their own:
/// the session in the panel and the session on the chip are one number, and
/// two thresholds tables would eventually disagree about it.
pub(super) fn band_color(percent: f32) -> Color {
    level_color(ClaudeUsageLevel::from_percent(percent))
}

/// What the pirate is painted in: his own yellow while the reading is current,
/// and the label's muted grey when it is not.
///
/// The same rule the percentage follows, for the same reason — a stale chip
/// has to read as stale, and a picture that stayed bright beside a greyed-out
/// number would be the loudest thing in the row saying otherwise.
fn face_color(problem: Option<UsageProblem>) -> Color {
    if problem.is_some() {
        theme().text_muted
    } else {
        PIRATE_FACE
    }
}

/// The last reading, if the most recent failure has not made it meaningless.
fn usable(
    snapshot: Option<&ClaudeUsageSnapshot>,
    problem: Option<UsageProblem>,
) -> Option<&ClaudeUsageSnapshot> {
    snapshot.filter(|_| !problem.is_some_and(UsageProblem::invalidates_the_reading))
}

/// Which frame of the bite to draw.
///
/// `None` is a pirate that is not chomping, which is a shut mouth — the same
/// frame the cycle starts and ends on.
fn chomp_frame(chomp: Option<usize>) -> Chomp {
    match chomp {
        Some(frame) => CHOMP_CYCLE[frame % CHOMP_CYCLE.len()],
        None => Chomp::Shut,
    }
}

/// The header's usage indicator.
pub struct UsageChip {
    usage: ModelHandle<UsageModel>,
    fonts: Fonts,
    mouse: MouseStateHandle,

    /// How far into the bite the pirate is, and — because a frame timer is
    /// outstanding exactly while this is `Some` — whether one is running. One
    /// field for both, so a second click cannot start a second timer chain
    /// that would chomp at twice the speed for the rest of the run.
    chomp: Option<usize>,

    /// Whether the panel is up.
    panel_open: bool,
}

impl UsageChip {
    /// Builds the chip and subscribes it to the usage model.
    pub fn new(fonts: Fonts, ctx: &mut ViewContext<Self>) -> Self {
        let usage = UsageModel::handle(ctx);
        ctx.observe(&usage, |chip, usage, ctx| {
            chip.start_chomping(usage.as_ref(ctx).is_busy_for_user(), ctx);
            ctx.notify();
        });

        Self {
            usage,
            fonts,
            mouse: MouseStateHandle::default(),
            chomp: None,
            panel_open: false,
        }
    }

    /// Shows the panel or takes it down, and asks for what it shows.
    ///
    /// Both reads are started on the way *up* only. Closing asks for nothing,
    /// and neither does a panel that is already open: the figures a person is
    /// looking at should not move under them because they clicked the chip
    /// again to dismiss it.
    pub(super) fn toggle_panel(&mut self, ctx: &mut ViewContext<Self>) {
        self.panel_open = !self.panel_open;
        if self.panel_open {
            self.usage.update(ctx, |model, ctx| {
                model.refresh_from_user(ctx);
                model.read_history(ctx);
            });
        }
        ctx.notify();
    }

    /// Starts the bite if a person is waiting and it is not running already.
    fn start_chomping(&mut self, busy: bool, ctx: &mut ViewContext<Self>) {
        if !busy || self.chomp.is_some() {
            return;
        }
        self.chomp = Some(0);
        self.schedule_frame(ctx);
    }

    /// Draws the next frame of the bite one frame's time from now.
    ///
    /// A sleep on the background pool rather than a timer of the foreground's
    /// own, which is what everything else here that waits does: there is no
    /// foreground timer, and a chip is not the place to introduce one.
    fn schedule_frame(&mut self, ctx: &mut ViewContext<Self>) {
        let sleeping = ctx.background().spawn(async move {
            std::thread::sleep(CHOMP_FRAME);
        });
        ctx.spawn(sleeping, |chip, (), ctx| {
            // The refresh may have landed while this frame was held, in which
            // case the mouth shuts here rather than a frame later.
            if !chip.usage.as_ref(ctx).is_busy_for_user() {
                chip.chomp = None;
            } else {
                chip.chomp = chip.chomp.map(|frame| frame + 1);
                chip.schedule_frame(ctx);
            }
            ctx.notify();
        })
        .detach();
    }

    /// The pirate, at the frame the bite is on.
    fn pirate(&self, problem: Option<UsageProblem>) -> Box<dyn Element> {
        let chomp = chomp_frame(self.chomp);

        Stack::new()
            .with_child(
                Icon::new(Art::PirateFace(chomp), PIRATE_SIZE)
                    .with_color(face_color(problem))
                    .finish(),
            )
            .with_child(
                Icon::new(Art::PirateInk(chomp), PIRATE_SIZE)
                    .with_color(PIRATE_INK)
                    .finish(),
            )
            .finish()
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
        let pirate = self.pirate(model.problem());

        let ui = self.fonts.ui;

        let pill = Hoverable::new(self.mouse.clone(), move |state| {
            let background = if state.is_clicked() {
                theme().ground
            } else if state.is_hovered() {
                theme().tab_active
            } else {
                theme().surface
            };

            let content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(pirate)
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Text::new(label, ui, 12.)
                                .with_color(label_color)
                                .with_style(Properties {
                                    weight: Weight::Semibold,
                                    ..Default::default()
                                })
                                .finish(),
                        )
                        .with_min_width(LABEL_WIDTH)
                        .finish(),
                    )
                    .with_margin_left(PIRATE_GUTTER)
                    .finish(),
                )
                .finish();

            Container::new(content)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                // A pixel more on every side than the pill carried when it had
                // an outline, because a border widened the box: the ground the
                // hover paints now ends where the line used to be drawn.
                .with_padding(Padding {
                    top: 4.,
                    left: 7.,
                    bottom: 4.,
                    right: 11.,
                })
                .finish()
        })
        .on_click(|_, ctx, _| ctx.dispatch_typed_action(UsageChipAction::TogglePanel))
        .finish();

        if !self.panel_open {
            return pill;
        }

        let mut stack = Stack::new().with_child(pill);
        stack.add_anchored_overlay_child(
            Dismiss::new(panel::render(model, ui))
                // Modal, like the options menu: the rest of the window is
                // inert while the panel is up, which is what makes clicking
                // the chip again one toggle rather than two.
                .modal()
                .on_dismiss(|ctx, _| ctx.dispatch_typed_action(UsageChipAction::TogglePanel))
                .finish(),
            AnchorTo {
                // Right edges aligned: the chip is the last thing in the
                // header, and a panel hung off its left corner would open past
                // the window.
                parent: Corner::BottomRight,
                child: Corner::TopRight,
                offset: vec2f(0., 6.),
                keep_on_screen: true,
                keep_clear_of_parent: false,
            },
        );
        stack.finish()
    }
}

impl TypedActionView for UsageChip {
    type Action = UsageChipAction;

    fn handle_action(&mut self, action: &UsageChipAction, ctx: &mut ViewContext<Self>) {
        match action {
            UsageChipAction::TogglePanel => self.toggle_panel(ctx),
        }
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
            weekly_resets_at: None,
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
        assert_eq!(
            face_color(Some(UsageProblem::Unreachable)),
            theme().text_muted,
            "and the pirate goes grey with it"
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

    #[test]
    fn a_pirate_nobody_is_waiting_on_has_his_mouth_shut() {
        assert_eq!(chomp_frame(None), Chomp::Shut);
    }

    #[test]
    fn the_bite_comes_back_round_to_a_whole_face() {
        let cycle: Vec<Chomp> = (0..6).map(|frame| chomp_frame(Some(frame))).collect();

        assert_eq!(
            cycle,
            vec![
                Chomp::Shut,
                Chomp::Open,
                Chomp::Wide,
                Chomp::Open,
                // Round again, from a mouth that is shut rather than from
                // wherever the last frame left it.
                Chomp::Shut,
                Chomp::Open,
            ]
        );
    }
}
