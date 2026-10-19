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
//! written out the way [`visible`] writes it, in the terminal's family and
//! wrapped at any character so that no part of a line it shows is cut, and
//! what goes with it. A command longer than a menu has room for is shown up
//! to [`SHOWN_LINES`] lines and [`SHOWN_CHARACTERS`] characters, and what is
//! left is counted in words where it stops. All of that comes from the
//! question the hook handed the window and none of it from what the agent
//! reported: a status's message is whatever the agent said, and a command
//! is shown only as it will run.
//!
//! Under it, how the pane's last three questions ended: what Crook sent and
//! whether the agent's hook said it took it, or what ended one nobody
//! answered here. Never that the agent did as it was told, which only its
//! next report can say.
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
use crate::control::ask::{Ask, Cause, Choice, How, Outcome};
use crate::plugins::store::ago;
use crate::tab::{AgentSession, AgentStatus, Attention, Pane, StatusSource};
use crate::theme::theme;

use super::answering::duration;
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
        let monospace = workspace.fonts().monospace;
        for line in lines(session, ask, Instant::now()) {
            column.add_child(match line {
                Said::Sentence(text) => sentence(text, ui),
                Said::Command(text) => command(text, monospace),
            });
        }
    }
    if let Some((_, pane)) = workspace.menu_target() {
        let ended: Vec<&Outcome> = workspace
            .asks()
            .history()
            .filter(|outcome| outcome.pane == pane)
            .collect();
        for line in answered(&ended, Instant::now()) {
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

/// One line of the panel: a sentence, or a line of a command an agent asks
/// to run, which is drawn as the terminal would and never cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Said {
    /// A sentence, wrapped between words.
    Sentence(String),
    /// A line of a command, as [`visible`] writes it.
    Command(String),
}

impl Said {
    /// What the line says.
    #[cfg(test)]
    pub(super) fn text(&self) -> &str {
        match self {
            Self::Sentence(text) | Self::Command(text) => text,
        }
    }
}

/// What the panel says about a session, one line each, at `now`, with the
/// question its agent's hook is waiting on when there is one.
///
/// Separated from the drawing so that the words can be read without a frame:
/// the sentences are the feature, and the elements around them are the
/// worktree menu's. The order is the order a person asks — what the dot
/// says and who said it, then what the agent is asking to do, then what the
/// agent is doing, then who asked for a look, then what the shell is up to.
pub(super) fn lines(session: &AgentSession, ask: Option<&Ask>, now: Instant) -> Vec<Said> {
    let mut sentences = about(session, now).into_iter().map(Said::Sentence);
    let mut said: Vec<Said> = sentences.next().into_iter().collect();
    if let Some(ask) = ask {
        asking(ask, now, &mut said);
    }
    said.extend(sentences);
    said
}

/// What the panel says about the session itself, the status first.
fn about(session: &AgentSession, now: Instant) -> Vec<String> {
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
/// rewriting what is shown; the first [`SHOWN_LINES`] of it and at most
/// [`SHOWN_CHARACTERS`] of those, each drawn whole, and how much more there
/// is, said in words wherever it stops — never an ellipsis a person could
/// take for the end of it.
///
/// A change to a file the same way: its path, and the lines of the change as
/// the card draws them, each behind its `-` or `+`.
fn asking(ask: &Ask, now: Instant, said: &mut Vec<Said>) {
    let agent = ask.agent.name();
    let all: Vec<String> = match &ask.request {
        AskRequest::Command { command, .. } => {
            said.push(Said::Sentence(format!(
                "It asks Crook, through {agent}'s PermissionRequest hook, to run:"
            )));
            visible(command).split('\n').map(str::to_owned).collect()
        }
        AskRequest::Edit { path, .. } | AskRequest::Write { path, .. } => {
            let verb = match ask.request {
                AskRequest::Write { replaces: None, .. } => "create",
                AskRequest::Write { .. } => "replace",
                _ => "edit",
            };
            said.push(Said::Sentence(format!(
                "It asks Crook, through {agent}'s PermissionRequest hook, to {verb} the file:"
            )));
            // On a line of its own, broken anywhere and never cut, as the
            // command is: the end of a path is the part that names the file.
            said.push(Said::Command(visible(path)));
            if let AskRequest::Write { replaces, .. } = &ask.request {
                said.push(Said::Sentence(super::answering::what_a_write_does(
                    *replaces,
                    ask.request.change_lines(),
                    agent,
                )));
            }
            if let Some(outside) = super::answering::outside_warning(&ask.request, &ask.cwd) {
                said.push(Said::Sentence(outside.to_owned()));
            }
            super::answering::change_lines(&ask.request)
                .into_iter()
                .map(|line| line.text)
                .collect()
        }
    };
    let mut room = SHOWN_CHARACTERS;
    let mut lines = 0;
    for line in all.iter().take(SHOWN_LINES) {
        lines += 1;
        let length = line.chars().count();
        if length <= room {
            room -= length;
            said.push(Said::Command(line.clone()));
            continue;
        }
        said.push(Said::Command(line.chars().take(room).collect()));
        said.push(Said::Sentence(match length - room {
            1 => "and 1 more character on that line".to_owned(),
            more => format!("and {more} more characters on that line"),
        }));
        break;
    }
    match all.len() - lines {
        0 => {}
        1 => said.push(Said::Sentence("and 1 more line".to_owned())),
        more => said.push(Said::Sentence(format!("and {more} more lines"))),
    }
    if let AskRequest::Command {
        note,
        timeout_ms,
        background,
        outside_sandbox,
        ..
    } = &ask.request
    {
        command_notes(
            agent,
            note.as_deref(),
            *timeout_ms,
            *background,
            *outside_sandbox,
            said,
        );
    }
    said.push(Said::Sentence(format!(
        "Asked {} ago.",
        duration(now.saturating_duration_since(ask.asked_at))
    )));
    if let Some(cannot) = ask.cannot_answer {
        said.push(Said::Sentence(cannot.sentence().to_owned()));
    }
}

/// What goes with a command: the agent's words, and how it would run.
fn command_notes(
    agent: &str,
    note: Option<&str>,
    timeout_ms: Option<u64>,
    background: bool,
    outside_sandbox: bool,
    said: &mut Vec<Said>,
) {
    if let Some(note) = note {
        said.push(Said::Sentence(format!("{agent} says: {}", visible(note))));
    }
    if outside_sandbox {
        said.push(Said::Sentence(format!(
            "Outside {agent}'s sandbox: it asks to run with none of the sandbox's limits."
        )));
    }
    if background {
        said.push(Said::Sentence(
            "In the background: the agent goes on without waiting for it.".to_owned(),
        ));
    }
    if let Some(timeout) = timeout_ms {
        said.push(Said::Sentence(format!(
            "With a time limit of {}.",
            duration(Duration::from_millis(timeout))
        )));
    }
}

/// How many lines of a command the panel shows before saying how many more
/// there are.
const SHOWN_LINES: usize = 12;

/// How many characters of a command, as [`visible`] writes it, the panel
/// shows at most, over all the lines it shows: a menu is a few hundred
/// pixels tall, and a command as long as the socket carries would be
/// hundreds of rows of it. The rest is counted in words.
const SHOWN_CHARACTERS: usize = 1024;

/// How many of a pane's last questions the panel says the end of.
const OUTCOMES: usize = 3;

/// How the last [`OUTCOMES`] questions asked in a pane ended, newest first,
/// one sentence each, given every one the window remembers of the pane's,
/// oldest first.
///
/// What was sent and what became of it, and no more: the window knows the
/// hook took an answer, never that the agent did as it said — a confined
/// session drops a hook's allow — so the agent's next report is the truth
/// about that, and nothing here claims it.
pub(super) fn answered(ended: &[&Outcome], now: Instant) -> Vec<String> {
    ended
        .iter()
        .rev()
        .take(OUTCOMES)
        .map(|outcome| {
            let agent = outcome.agent.name();
            let when = since(outcome.ended_at, now);
            // Written out already, as the row's line is: see `ask::summary`.
            let what = &outcome.summary;
            let how = match outcome.how {
                How::Answered(choice) => {
                    let sent = match choice {
                        Choice::Allow => format!("Allow sent to {agent}"),
                        Choice::Deny => "Denied in Crook".to_owned(),
                    };
                    let after = if outcome.delivered {
                        format!(", and {agent}'s hook took it")
                    } else if outcome.lost {
                        ", but the hook was gone before it could pass it on".to_owned()
                    } else {
                        String::new()
                    };
                    format!("{sent} {when}{after}")
                }
                How::Ended(cause) => format!("Not answered here: {} {when}", ended_by(cause)),
            };
            format!("{what} — {how}.")
        })
        .collect()
}

/// What ended a question nobody answered in Crook, as the panel says it.
fn ended_by(cause: Cause) -> &'static str {
    match cause {
        Cause::AnsweredInAgent => "it was answered in the agent's own tab",
        Cause::HookGone => "its hook stopped waiting",
        Cause::Expired => "its time ran out",
        Cause::PaneClosed => "its pane closed",
        Cause::CommandEnded => "the agent's command ended",
        Cause::TurnEnded => "the agent's turn ended",
        Cause::AskedAgain => "a newer question replaced it",
        Cause::WindowClosed => "the window closed",
        Cause::AnsweringOff => "letting agents ask in Crook was turned off",
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

/// One line of a command, in the terminal's family, wrapped at whichever
/// character the panel's width runs out at: every space where it was, and
/// nothing cut — a command with no spaces in it is as likely as one with
/// many, and the end of it is the part that does the damage.
fn command(text: String, monospace: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text, monospace, LABEL_SIZE)
            .breaking_anywhere()
            .with_color(theme().text_primary)
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
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

        let lines_said = lines(&session, Some(ask), asked_at + Duration::from_secs(40));
        let said: Vec<&str> = lines_said.iter().map(Said::text).collect();
        let from = said
            .iter()
            .position(|line| {
                *line == "It asks Crook, through Claude Code's PermissionRequest hook, to run:"
            })
            .expect("the panel says what it asks");
        assert_eq!(from, 1, "right after the status: {said:#?}");
        let shown = &said[from + 1..];
        assert_eq!(shown[0], "echo safe␍rm -rf ~", "{said:#?}");
        // The twelve lines of the command are drawn as a command, and what
        // follows them as sentences.
        let kinds = &lines_said[from + 1..];
        assert!(
            kinds[..12]
                .iter()
                .all(|line| matches!(line, Said::Command(_))),
            "{kinds:#?}"
        );
        assert!(matches!(kinds[12], Said::Sentence(_)), "{kinds:#?}");
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
            said.contains(&"It asks: Claude needs your permission to use Bash"),
            "{said:#?}"
        );

        // Without a question, none of it.
        let without = lines(&session, None, asked_at);
        assert!(
            !without
                .iter()
                .any(|line| line.text().contains("PermissionRequest")),
            "{without:#?}"
        );
    }

    /// A command longer than the panel has room for is shown as far as the
    /// room goes and the rest counted, so that where it stops is never taken
    /// for where it ends.
    #[test]
    fn a_command_too_long_for_the_panel_says_how_much_is_left() {
        let session = AgentSession::new("agent 1");
        let long = "A".repeat(SHOWN_CHARACTERS - 8);
        let command = format!("cd /srv\n{long}\nrm -rf ~\nls");
        let mut asks = Asks::default();
        let asked_at = Instant::now();
        asks.add(
            asked(&command),
            Found {
                pane: PaneId::next(),
                cannot_answer: None,
                tab_title: "crook".to_owned(),
            },
            Arc::new(Feed::for_verb(&Verb::AgentAsk(asked("ls")))),
            asked_at,
        )
        .expect("held");
        let ask = asks.live().next().expect("held");

        let said = lines(&session, Some(ask), asked_at);
        let from = said
            .iter()
            .position(|line| matches!(line, Said::Command(_)))
            .expect("the command is shown");
        assert_eq!(said[from], Said::Command("cd /srv".to_owned()));
        assert_eq!(said[from + 1], Said::Command(long));
        // The first two lines leave room for one character of the third.
        assert_eq!(said[from + 2], Said::Command("r".to_owned()));
        assert_eq!(
            said[from + 3],
            Said::Sentence("and 7 more characters on that line".to_owned())
        );
        assert_eq!(said[from + 4], Said::Sentence("and 1 more line".to_owned()));
    }

    #[test]
    fn why_this_status_shows_an_edits_path_and_the_first_twelve_lines_of_its_change() {
        let session = AgentSession::new("agent 1");
        let old: String = (1..=10).map(|line| format!("o{line}\n")).collect();
        let request = AskRequest::Edit {
            path: "/etc/\u{202e}hosts".to_owned(),
            edits: vec![crate::agent::request::FileEdit {
                old,
                new: "n1\nn2".to_owned(),
                replace_all: false,
            }],
            resolves_outside: false,
        };
        let edit = AgentAsk {
            request,
            ..asked("ls")
        };
        let mut asks = Asks::default();
        let asked_at = Instant::now();
        asks.add(
            edit,
            Found {
                pane: PaneId::next(),
                cannot_answer: None,
                tab_title: "crook".to_owned(),
            },
            Arc::new(Feed::for_verb(&Verb::AgentAsk(asked("ls")))),
            asked_at,
        )
        .expect("held");
        let ask = asks.live().next().expect("held");
        assert_eq!(ask.summary, "Edit: /etc/⟨U+202E⟩hosts");

        let said = lines(&session, Some(ask), asked_at);
        let from = said
            .iter()
            .position(|line| {
                line.text() == "It asks Crook, through Claude Code's PermissionRequest hook, to edit the file:"
            })
            .unwrap_or_else(|| panic!("{said:#?}"));
        assert_eq!(
            said[from + 1],
            Said::Command("/etc/⟨U+202E⟩hosts".to_owned())
        );
        assert!(
            said[from + 2]
                .text()
                .starts_with("Outside its working directory"),
            "{said:#?}"
        );
        // Ten removed lines, the empty one after the last newline, and the
        // first added: twelve. The other is counted.
        let change: Vec<&str> = said[from + 3..from + 15].iter().map(Said::text).collect();
        assert_eq!(change[0], "- o1");
        assert_eq!(change[9], "- o10");
        assert_eq!(change[10], "- ");
        assert_eq!(change[11], "+ n1");
        assert!(
            said[from + 3..from + 15]
                .iter()
                .all(|line| matches!(line, Said::Command(_)))
        );
        assert_eq!(
            said[from + 15],
            Said::Sentence("and 1 more line".to_owned())
        );
        // A command's notes are a command's: none of them here.
        assert!(
            !said.iter().any(|line| line.text().contains("sandbox")),
            "{said:#?}"
        );
    }
}
