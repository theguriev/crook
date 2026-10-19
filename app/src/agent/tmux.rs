//! The way out of tmux for a report made inside it.
//!
//! tmux in a pane is how a person keeps agents alive across a restart of
//! Crook, which keeps no processes, and tmux reads everything its panes write:
//! an OSC it does not know — 6340 and 6342 among them — goes no further. A
//! report written to the agent's terminal, which is one of tmux's panes, never
//! reached Crook's, and every hook exited 0 all the same.
//!
//! So a report from inside tmux goes around it, to the terminal of each client
//! attached to the pane's session. tmux's client is a program in a Crook pane
//! like any other, and the terminal it is on is that pane. `tmux` itself is
//! asked which terminals those are, once, under [`DEADLINE`]. Every client of
//! the session rather than the one tmux would call current, because current is
//! the one last typed at and the Crook pane need not be it; a client on any
//! other terminal drops the sequence unread, as every other terminal does. A
//! client in control mode — iTerm2's `tmux -CC` — is passed over: its terminal
//! carries tmux's protocol to a program that parses it, not a screen.
//!
//! Those terminals are tmux's to write to as well, and two things follow.
//! One is that a client's terminal can stop being read — a second attach over
//! an `ssh` whose connection has gone quiet — and fill, and a write to a full
//! terminal waits until it is read again; so the clients are written to side
//! by side and given [`TAKING`] between them, and a write still waiting then
//! is left to end with the process. The other is that tmux writes without
//! waiting, and cuts its output wherever the terminal stops taking it, the
//! middle of an escape sequence included: a report that lands in that gap
//! ends tmux's sequence early, and the rest of it is drawn as text until tmux
//! next redraws those cells. That takes a terminal read too slowly to keep up
//! with tmux at the moment a report is made.
//!
//! tmux has one other way out, its passthrough: a sequence wrapped for tmux
//! to pass on to its clients itself, in its own order, and so never inside
//! one of its own. It is taken only when there is no client to write to —
//! tmux cannot be asked, names none, or none of them took the report in time
//! — and never to spare those stray characters, whatever `allow-passthrough`
//! says, because on the same slow terminal it loses the report. What a
//! client has yet to take waits in a buffer of tmux's, and once that passes
//! eight bytes for every cell of the client's screen tmux throws the whole of
//! it away, counts it in `client_discarded`, and redraws the screen when the
//! terminal has caught up. A passthrough waits there with everything else
//! and goes with it, and the redraw brings back what tmux drew and nothing it
//! passed on: the report is gone without a word, and the tab says `running`
//! all through a wait for an answer. A write to the terminal is nothing tmux
//! holds, so nothing tmux throws away takes it. Where the passthrough is the
//! way, tmux passes it on only where the option lets it — tmux 3.3 added the
//! option, off by default, and every tmux before it passed each wrapper on —
//! and with `on` only from a pane on screen.
//!
//! The clients are told only when this process's terminal is the pane
//! `TMUX_PANE` names, by tmux's own account. `TMUX` is an environment
//! variable, and every process started from inside tmux inherits it: Crook
//! itself, when it is started from a terminal running tmux, and so every pane
//! of that Crook, where nothing is inside tmux at all and a report sent to
//! tmux's client would land on the terminal Crook was started from. tmux
//! tells whether a client is nested inside it the same way: by the variable,
//! and then by whether the client's terminal is one of its panes. And an
//! agent with no terminal on the pane — `claude -p` under another agent's
//! Bash tool — gets none from tmux either, for the reason `ancestors` gives
//! it none. When tmux cannot be asked there is no such account, and the
//! passthrough is a guess made on `TERM_PROGRAM` alone, which tmux sets to
//! `tmux` in its panes from 3.2 on: see [`route_from`] for why a wrong guess
//! costs nothing.
//!
//! What goes through here is what `crook --agent` writes, and nothing else:
//! a notification Claude Code sends itself — OSC 9 — is its own sequence in
//! tmux's pane, and reaches Crook only as far as tmux and its
//! `allow-passthrough` let it.

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long tmux is given to say where its clients are.
///
/// It answers in milliseconds. This is for a server that does not answer at
/// all, which would otherwise hold the hook — and the agent waiting on its
/// hook — for as long as it hung.
const DEADLINE: Duration = Duration::from_secs(1);

/// How long the clients' terminals are given to take a report, all of them
/// together.
///
/// A terminal that is read takes one at once. This is for one that is not:
/// waited for, it would hold the hook until somebody read it, on every tool
/// call, for as long as an `ssh` takes to give up on a connection that has
/// gone quiet.
const TAKING: Duration = Duration::from_millis(200);

/// Where a report from inside tmux goes instead of the pane's terminal.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// The terminals of the clients attached to the pane's session.
    Clients(Vec<PathBuf>),
    /// The pane's terminal still, wrapped by [`passthrough`] for tmux to pass
    /// on.
    Passthrough,
}

/// Where a report goes when this process is inside tmux, or `None` when it is
/// not, and the report goes to its own terminal as it would anywhere.
pub(super) fn route() -> Option<Route> {
    if std::env::var_os("TMUX").is_none_or(|tmux| tmux.is_empty()) {
        return None;
    }
    let answer = std::env::var("TMUX_PANE").ok().and_then(|pane| ask(&pane));
    let says_tmux = std::env::var_os("TERM_PROGRAM").is_some_and(|program| program == "tmux");
    route_from(
        answer.as_deref(),
        says_tmux,
        super::ancestors::is_the_terminal,
    )
}

/// The terminals of the clients attached to the pane's session, for a
/// question from inside tmux to name: `None` where [`route`] finds no client
/// to write to — not inside tmux, not its pane, or a session no client but
/// one in control mode is attached to.
pub(super) fn clients() -> Option<Vec<PathBuf>> {
    match route()? {
        Route::Clients(clients) => Some(clients),
        Route::Passthrough => None,
    }
}

/// [`route`], from what [`ask`] answered, whether `TERM_PROGRAM` says tmux
/// started this process's shell, and how to tell this process's terminal.
///
/// tmux's answer decides when there is one. Without it, `TERM_PROGRAM` is
/// what is left, and it is less: tmux says `tmux` there to its own panes, but
/// so does the environment of anything started from inside one. So it is
/// trusted with the passthrough and nothing more, a guess that costs nothing
/// when it is wrong: on a terminal that is Crook's after all, the escape that
/// ends the wrapper in Crook's reader is followed by the one that starts the
/// report, which is read as though it had never been wrapped, and every other
/// terminal drops it unread.
fn route_from(
    answer: Option<&str>,
    says_tmux: bool,
    is_ours: impl Fn(&Path) -> bool,
) -> Option<Route> {
    let Some(answer) = answer else {
        return says_tmux.then_some(Route::Passthrough);
    };
    let mut lines = answer.lines();
    if !is_ours(Path::new(lines.next()?)) {
        return None;
    }
    let clients: Vec<_> = lines.filter_map(client).collect();
    Some(if clients.is_empty() {
        Route::Passthrough
    } else {
        Route::Clients(clients)
    })
}

/// The terminal of the client one line of [`ask`]'s answer names, unless the
/// client is in control mode.
fn client(line: &str) -> Option<PathBuf> {
    let (control, terminal) = line.split_once(' ')?;
    (control != "1" && !terminal.is_empty()).then(|| PathBuf::from(terminal))
}

/// tmux's account of `pane`: on the first line its terminal, then a line for
/// each client attached to its session, its control mode — `1` or `0` — and
/// its terminal. `None` when tmux could not be run, refused, or did not
/// answer within [`DEADLINE`].
///
/// One `tmux` for both, with the `;` that runs two commands in one: a second
/// process would be a second wait on every hook.
fn ask(pane: &str) -> Option<String> {
    let mut tmux = crate::process::command("tmux")
        .args(["display-message", "-p", "-t", pane])
        .args(["#{pane_tty}", ";"])
        .args(["list-clients", "-t", pane, "-F"])
        .arg("#{client_control_mode} #{client_tty}")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut output = tmux.stdout.take()?;
    let (sender, said) = mpsc::channel();
    std::thread::spawn(move || {
        let mut answer = String::new();
        // Fails only when the deadline has passed and nobody is listening.
        let _ = sender.send(output.read_to_string(&mut answer).map(|_| answer));
    });
    // tmux's output ends when tmux does, so its end is the whole answer, and a
    // tmux that has not got there by the deadline is killed rather than
    // waited for.
    let answer = said.recv_timeout(DEADLINE).ok().and_then(Result::ok);
    if answer.is_none() {
        let _ = tmux.kill();
    }
    let finished = tmux.wait().ok()?;
    answer.filter(|_| finished.success())
}

/// `sequence` wrapped in tmux's passthrough: `ESC P tmux;`, the sequence with
/// every escape in it doubled, and `ESC \`.
///
/// Doubled because tmux reads an escape on its own as the start of the `ESC \`
/// that ends the wrapper, and a pair as one escape of what it passes on.
pub(super) fn passthrough(sequence: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
}

/// Writes `sequence` to the terminal of each of `clients`, and says whether
/// any of them took it within [`TAKING`].
///
/// Each on a thread of its own, and all of them waited for, up to then: one
/// terminal that is not being read holds its own write and no other, and the
/// process ends when this returns, which would end a write still under way.
/// A write that has not finished by then is left to that, having put on its
/// terminal whatever part of the report the terminal took. A terminal that
/// will not open — another user's client, on a socket they share — is passed
/// over rather than the end of it.
pub(super) fn tell(clients: &[PathBuf], sequence: &str) -> bool {
    let (sender, took) = mpsc::channel();
    for client in clients {
        let (sender, client, sequence) = (sender.clone(), client.clone(), sequence.to_owned());
        std::thread::spawn(move || {
            let written = OpenOptions::new()
                .write(true)
                .open(client)
                .and_then(|mut terminal| {
                    terminal.write_all(sequence.as_bytes())?;
                    terminal.flush()
                });
            // Fails only when the wait is over and nobody is listening.
            let _ = sender.send(written.is_ok());
        });
    }
    drop(sender);
    let deadline = Instant::now() + TAKING;
    let mut told = false;
    // Until every writer has said, or the time is up.
    while let Ok(wrote) = took.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        told |= wrote;
    }
    told
}

#[cfg(test)]
mod tests {
    use crook_terminal::{AgentReport, Emulator, Palette, TerminalSize};

    use super::*;

    /// The pane's terminal, as tmux names it.
    const PANE: &str = "/dev/pts/6";

    /// Whether `path` is [`PANE`]: this process's terminal, in a case where
    /// it is in that pane.
    fn in_the_pane(path: &Path) -> bool {
        path == Path::new(PANE)
    }

    #[test]
    fn the_passthrough_doubles_every_escape_and_ends_the_way_tmux_reads_it() {
        assert_eq!(
            "\x1bPtmux;\x1b\x1b]6340;running;port the tab bar\x07\x1b\\",
            passthrough("\x1b]6340;running;port the tab bar\x07")
        );
        // A status and its pull request travel as one write, and pass as one.
        assert_eq!(
            "\x1bPtmux;\x1b\x1b]6340;idle\x07\x1b\x1b]6342;pr;https://github.com/o/r/pull/1\x07\x1b\\",
            passthrough("\x1b]6340;idle\x07\x1b]6342;pr;https://github.com/o/r/pull/1\x07")
        );
    }

    #[test]
    fn a_wrapped_report_that_reaches_crook_itself_is_read_all_the_same() {
        // The passthrough guessed from TERM_PROGRAM, on a terminal that turns
        // out to be Crook's: an escape ends the wrapper there, and the
        // doubled one starts the report.
        let report = crook_terminal::agent::report(
            AgentReport::NeedsInput,
            Some("port the tab bar"),
            Some("run rm -rf build?"),
            None,
        );
        let mut emulator = Emulator::new(TerminalSize::new(20, 5), 100, Palette::default());
        emulator.advance(passthrough(&report).as_bytes());
        assert_eq!(AgentReport::NeedsInput, emulator.agent());
    }

    #[test]
    fn inside_tmux_a_report_goes_to_every_client_but_one_in_control_mode() {
        // Whatever the pane's `allow-passthrough`, which is not asked: a
        // passthrough is thrown away with the rest of what tmux has yet to
        // write to a client that has fallen behind.
        let answer = format!("{PANE}\n0 /dev/pts/5\n1 /dev/ttys004\n0 /dev/pts/9\n");
        assert_eq!(
            Some(Route::Clients(vec![
                PathBuf::from("/dev/pts/5"),
                PathBuf::from("/dev/pts/9")
            ])),
            route_from(Some(&answer), true, in_the_pane)
        );
    }

    #[test]
    fn a_session_with_no_client_to_write_to_is_left_to_the_passthrough() {
        // Detached, or attached only by iTerm2's control mode.
        for answer in [format!("{PANE}\n"), format!("{PANE}\n1 /dev/ttys004\n")] {
            assert_eq!(
                Some(Route::Passthrough),
                route_from(Some(&answer), true, in_the_pane),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn a_tmux_variable_on_a_terminal_that_is_not_its_pane_changes_nothing() {
        // Crook started from inside tmux: its panes inherit TMUX and
        // TMUX_PANE, and say TERM_PROGRAM=Crook. And an agent with no
        // terminal on the pane, where TERM_PROGRAM says tmux all the same.
        let answer = "/dev/pts/2\n0 /dev/pts/1\n";
        for says_tmux in [false, true] {
            assert_eq!(None, route_from(Some(answer), says_tmux, in_the_pane));
        }
        assert_eq!(None, route_from(Some(""), true, in_the_pane));
    }

    #[test]
    fn a_client_that_takes_nothing_holds_the_report_no_longer_than_its_time() {
        // A named pipe nobody reads stands in for a terminal nobody reads: a
        // write there waits for a reader that never comes, in `open` rather
        // than in `write`, on the same thread and for the same reason. A
        // plain file stands in for a terminal that takes the report. The
        // writers left waiting end with the test binary, as they end with
        // `crook --agent`.
        let scratch = std::env::temp_dir().join(format!("crook-tmux-tell-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let unread = scratch.join("unread");
        let read = scratch.join("read");
        let made = crate::process::command("mkfifo")
            .arg(&unread)
            .status()
            .is_ok_and(|status| status.success());
        assert!(made, "mkfifo made no pipe");

        // Before and after the one that takes it: neither order holds it
        // back, and the one is enough.
        let report = "\x1b]6340;running\x07";
        for clients in [
            [unread.clone(), read.clone()],
            [read.clone(), unread.clone()],
        ] {
            std::fs::write(&read, "").unwrap();
            let (told, took) = tell_in_time(&clients, report);
            assert_eq!(Ok(true), told, "{clients:?} held `tell` for {took:?}");
            assert_eq!(report, std::fs::read_to_string(&read).unwrap());
        }
        // Alone, it is no client told, and the report goes the other way.
        let (told, waited) = tell_in_time(std::slice::from_ref(&unread), report);
        assert_eq!(Ok(false), told, "{unread:?} held `tell` for {waited:?}");
        assert!(TAKING <= waited, "{waited:?}");

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// What [`tell`] says to `clients`, and how long it took to say it: run
    /// on a thread of its own and waited for no longer than [`DEADLINE`], so
    /// that a `tell` a client holds is this failing with a `Timeout`, rather
    /// than a test that never ends — a `tell` that opened its clients one
    /// after another would never get past the pipe nobody reads. The thread
    /// left waiting then ends with the test binary.
    fn tell_in_time(
        clients: &[PathBuf],
        sequence: &'static str,
    ) -> (Result<bool, mpsc::RecvTimeoutError>, Duration) {
        let clients = clients.to_vec();
        let (sender, told) = mpsc::channel();
        let started = Instant::now();
        std::thread::spawn(move || {
            // Fails only when the wait is over and nobody is listening.
            let _ = sender.send(tell(&clients, sequence));
        });
        (told.recv_timeout(DEADLINE), started.elapsed())
    }

    #[test]
    fn a_tmux_that_cannot_be_asked_is_trusted_with_the_passthrough_only_where_it_started_the_shell()
    {
        assert_eq!(
            Some(Route::Passthrough),
            route_from(None, true, in_the_pane)
        );
        assert_eq!(None, route_from(None, false, in_the_pane));
    }
}
