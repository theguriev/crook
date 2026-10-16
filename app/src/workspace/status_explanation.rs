//! The panel that says why a row's dot is what it is.
//!
//! A row is amber, and nothing on it says whether the agent asked for an
//! answer, the shell rang, a program sent a notification, a person marked it
//! to come back to, or a command ended under an agent that never said it
//! stopped. Every one of those is a fact the session already holds — see
//! [`AgentSession`] — and this is the one place they are read out in words.
//! herdr answers the same question with `herdr agent explain <target>`;
//! Crook's socket lists the panes, opens tabs and lets a pane watch the tabs
//! it opened, and explains no row's status yet — see [`crate::control`] — so
//! the answer is an entry on the row's own menu, "Why this status",
//! contributed by [`crook/tabs`](crate::plugins::tabs). A tab another pane
//! opened says so here too, since who started the work is the first thing a
//! person asks about a row they did not open.
//!
//! # What an agent asks
//!
//! When the agent's permission hook is waiting on a question the window
//! holds — see [`crate::control::ask`] — the panel says what it asks to run,
//! whole but for its length, written out the way [`visible`] writes it, and
//! what goes with it. All of that comes from the question the hook handed
//! the window and none of it from what the agent reported: a status's
//! message is whatever the agent said, and a command is shown only as it
//! will run.
//!
//! # Where it hangs
//!
//! Off the tab menu's corner, where the worktree list hangs, and for the same
//! reason that list is not a popup: a second popup would float above the
//! menu's underlay while the underlay ate the press meant to dismiss it. It is
//! read-only — no rows, no field, nothing to press — so it takes no key but
//! Escape, which takes it down and leaves the menu standing, and the arrows
//! keep walking the menu under it.
//!
//! # Nothing ticks
//!
//! "2 minutes ago" is a subtraction done when the panel is drawn, from an
//! [`Instant`] the session wrote when the report arrived. The panel is drawn
//! when something else redraws the window, and a person who leaves it up
//! reads the age it had when they opened it — which is what they asked, and
//! what keeps a timer out of a window that has none.

use std::time::{Duration, Instant};

use crookui_core::elements::Paragraph;
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::agent::request::{AskRequest, visible};
use crate::control::ask::Ask;
use crate::plugins::store::ago;
use crate::tab::{AgentSession, AgentStatus, Attention, Pane, StatusSource};
use crate::theme::theme;

use super::tab_menu::{LABEL_SIZE, MENU_WIDTH, PATH_SIZE, ROW_INSET};
use super::view::Workspace;

/// The popup's corner radius, which is every other menu's in this application.
const PANEL_RADIUS: f32 = 6.;

/// The whole panel: a heading, and one sentence per fact.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let session = workspace
        .menu_target()
        .and_then(|(_, pane)| workspace.tabs().pane(pane))
        .map(Pane::session);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(heading("Why this status", ui));

    let ask = workspace
        .menu_target()
        .and_then(|(_, pane)| workspace.asks().in_pane(pane));

    // A menu is only ever up on a row that exists, so the branch with no
    // session is a frame between a pane closing and the menu hearing of it.
    if let Some(session) = session {
        for line in lines(session, ask, Instant::now()) {
            column.add_child(sentence(line, ui));
        }
    }

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PANEL_RADIUS)))
            .with_vertical_padding(6.)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish()
}

/// What the panel says about a session, one sentence per line, at `now`,
/// with the question its agent's hook is waiting on when there is one.
///
/// Separated from the drawing so that the words can be read without a frame:
/// the sentences are the feature, and the elements around them are the
/// worktree menu's. The order is the order a person asks — what the dot
/// says and who said it, then what the agent is asking to do, then what the
/// agent is doing, then who asked for a look, then what the shell is up to.
pub(super) fn lines(session: &AgentSession, ask: Option<&Ask>, now: Instant) -> Vec<String> {
    let status = session.status.label();
    let mut lines = vec![match session.source {
        StatusSource::NoReport => format!("Status: {status} — no report yet."),
        StatusSource::Agent(at) => {
            format!("Status: {status} — the agent said so {}.", since(at, now))
        }
        StatusSource::Title(at) => format!(
            "Status: {status} — the agent's title said so {}.",
            since(at, now)
        ),
        StatusSource::TitleAtStart(at) => format!(
            "Status: {status} — the agent's title said {} that it was there, and no turn has \
             started since.",
            since(at, now)
        ),
        StatusSource::TitleStoppedAsking(at) => format!(
            "Status: {status} — the agent's title stopped asking {}, and has not said since \
             whether it is at work.",
            since(at, now)
        ),
        StatusSource::TitleBetweenHooks(at) if session.status == AgentStatus::Idle => format!(
            "Status: {status} — the agent's title stopped saying it was at work {}, and its \
             hooks have not said since whether the turn is over.",
            since(at, now)
        ),
        StatusSource::TitleBetweenHooks(at) => format!(
            "Status: {status} — the agent's title said it was at work again {}, after its \
             hooks' last report.",
            since(at, now)
        ),
        StatusSource::CommandEnded(at) => format!(
            "Status: {status} — the command ended {} and took the agent's last report back.",
            since(at, now)
        ),
    }];

    if let Some(ask) = ask {
        asking(ask, now, &mut lines);
    }

    // The one case the dot and the status disagree, and the case most worth
    // a sentence: an amber dot on a row whose agent said nothing of the sort.
    let shown = session.shown_status();
    if shown != session.status {
        lines.push(format!(
            "The dot says {}, because the row asks for a look.",
            shown.label()
        ));
    }

    if let Some(title) = &session.derived_title {
        lines.push(format!("The agent calls its work “{title}”."));
    }
    if let Some(message) = &session.message {
        lines.push(match session.status {
            AgentStatus::Failed => format!("What went wrong: {message}"),
            _ => format!("It asks: {message}"),
        });
    }

    match &session.attention {
        Some(Attention::Bell) => {
            lines.push("Attention: the bell rang while nobody was looking.".to_owned());
        }
        Some(Attention::StatusChange) => {
            lines.push("Attention: the status changed while nobody was looking.".to_owned());
        }
        Some(Attention::Notification(message)) => lines.push(format!(
            "Attention: a notification arrived while nobody was looking: {message}"
        )),
        None => {}
    }
    if session.marked {
        lines.push("Marked as waiting by you.".to_owned());
    }
    if session.attention.is_none() && !session.marked {
        lines.push(match session.status {
            AgentStatus::NeedsInput => "Nothing else asks for a look.".to_owned(),
            _ => "Nothing asks for a look.".to_owned(),
        });
    }

    lines.push(match &session.running_command {
        Some(command) => format!("The shell is running {command}."),
        None => "The shell is at a prompt.".to_owned(),
    });

    if let Some(lineage) = &session.spawned_by {
        lines.push(format!("Opened by “{}” with crook tab new.", lineage.title));
    }

    lines
}

/// The question the agent's hook is waiting on, written out: what the hook
/// handed the window, and nothing the agent reported about it.
///
/// The command line by line, through [`visible`], so that a carriage return,
/// an escape or a right-to-left override reads as the mark it is rather than
/// rewriting what is shown; the first [`SHOWN_LINES`] of it, and how many
/// more there are.
fn asking(ask: &Ask, now: Instant, lines: &mut Vec<String>) {
    let AskRequest::Command {
        command,
        note,
        timeout_ms,
        background,
        outside_sandbox,
    } = &ask.request;
    let agent = ask.agent.name();
    lines.push(format!(
        "It asks Crook, through {agent}'s PermissionRequest hook, to run:"
    ));
    let shown = visible(command);
    let all: Vec<&str> = shown.split('\n').collect();
    lines.extend(all.iter().take(SHOWN_LINES).map(|line| (*line).to_owned()));
    match all.len().saturating_sub(SHOWN_LINES) {
        0 => {}
        1 => lines.push("and 1 more line".to_owned()),
        more => lines.push(format!("and {more} more lines")),
    }
    if let Some(note) = note {
        lines.push(format!("{agent} says: {}", visible(note)));
    }
    if *outside_sandbox {
        lines.push(format!(
            "Outside {agent}'s sandbox: it asks to run with none of the sandbox's limits."
        ));
    }
    if *background {
        lines.push("In the background: the agent goes on without waiting for it.".to_owned());
    }
    if let Some(timeout) = timeout_ms {
        lines.push(format!(
            "With a time limit of {}.",
            duration(Duration::from_millis(*timeout))
        ));
    }
    lines.push(format!(
        "Asked {} ago.",
        duration(now.saturating_duration_since(ask.asked_at))
    ));
    if let Some(cannot) = ask.cannot_answer {
        lines.push(cannot.sentence().to_owned());
    }
}

/// How many lines of a command the panel shows before saying how many more
/// there are.
const SHOWN_LINES: usize = 12;

/// "40 s", "2 min 5 s": a span short enough to say to the second.
fn duration(span: Duration) -> String {
    let seconds = span.as_secs();
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

/// "just now", "2 minutes ago": how long before `now` the instant was.
///
/// Saturating, because an `Instant` written on one thread and read on
/// another can be a hair in the future, and a panic in a menu over a
/// nanosecond would be the wrong answer to it.
fn since(at: Instant, now: Instant) -> String {
    ago(now.saturating_duration_since(at).as_secs())
}

/// The panel's title, in the worktree list's own heading style.
fn heading(title: &'static str, ui: FamilyId) -> Box<dyn Element> {
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

/// One fact, wrapped to the panel's width.
fn sentence(text: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text, ui, LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::control::ask::{Asks, CannotAnswer, Found};
    use crate::control::protocol::{AgentAsk, AskingAgent, Verb};
    use crate::control::watch::Feed;
    use crate::tab::PaneId;

    /// A question as a Claude Code hook hands it to the window, about
    /// `command`.
    fn asked(command: &str) -> AgentAsk {
        AgentAsk {
            agent: AskingAgent::Claude,
            session: "4f1c2b7e".to_owned(),
            cwd: "/home/someone/work/crook".to_owned(),
            tty: 34819,
            wait: 300,
            request: AskRequest::Command {
                command: command.to_owned(),
                note: Some("Clean \u{202e}up".to_owned()),
                timeout_ms: Some(120_000),
                background: true,
                outside_sandbox: true,
            },
        }
    }

    #[test]
    fn why_this_status_shows_the_whole_command_written_out() {
        let mut session = AgentSession::new("agent 1");
        session.status = AgentStatus::NeedsInput;
        // What the agent said, which is not what is shown as the command.
        session.message = Some("Claude needs your permission to use Bash".to_owned());

        let command: String = ["echo safe\rrm -rf ~", "printf '\u{1b}[2J'"]
            .into_iter()
            .chain((1..=14).map(|line| if line == 5 { "\u{200b}ls" } else { "true" }))
            .collect::<Vec<_>>()
            .join("\n");
        let mut asks = Asks::default();
        let asked_at = Instant::now();
        asks.add(
            asked(&command),
            Found {
                pane: PaneId::next(),
                cannot_answer: Some(CannotAnswer::OtherTab),
                tab_title: "crook".to_owned(),
            },
            Arc::new(Feed::for_verb(&Verb::AgentAsk(asked("ls")))),
            asked_at,
        )
        .expect("held");
        let ask = asks.live().next().expect("held");

        let said = lines(&session, Some(ask), asked_at + Duration::from_secs(40));
        let from = said
            .iter()
            .position(|line| {
                line == "It asks Crook, through Claude Code's PermissionRequest hook, to run:"
            })
            .expect("the panel says what it asks");
        assert_eq!(from, 1, "right after the status: {said:#?}");
        let shown = &said[from + 1..];
        assert_eq!(shown[0], "echo safe␍rm -rf ~", "{said:#?}");
        assert_eq!(shown[1], "printf '⟨U+001B⟩[2J'");
        assert_eq!(shown[6], "⟨U+200B⟩ls");
        assert_eq!(shown[11], "true", "the twelfth line");
        assert_eq!(shown[12], "and 4 more lines");
        assert_eq!(shown[13], "Claude Code says: Clean ⟨U+202E⟩up");
        let rest = shown[14..].join("\n");
        assert!(rest.contains("Outside Claude Code's sandbox"), "{rest}");
        assert!(rest.contains("In the background"), "{rest}");
        assert!(rest.contains("2 min"), "{rest}");
        assert!(rest.contains("Asked 40 s ago."), "{rest}");
        assert!(rest.contains(CannotAnswer::OtherTab.sentence()), "{rest}");
        // Still the session's own sentences after it.
        assert!(
            said.iter()
                .any(|line| line == "It asks: Claude needs your permission to use Bash"),
            "{said:#?}"
        );

        // Without a question, none of it.
        let without = lines(&session, None, asked_at);
        assert!(
            !without
                .iter()
                .any(|line| line.contains("PermissionRequest")),
            "{without:#?}"
        );
    }
}
