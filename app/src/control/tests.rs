//! The window answering, tested where each part can be reached: the wire with
//! no connection, a connection with no window, the window with no display —
//! and all of it at once, a real socket asked by the real command line about
//! a real workspace.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::FamilyId;
use crookui_core::{App, ViewHandle};
use serde_json::{Value, json};

use super::protocol::{self, PaneEntry, Reply, Verb, code};
use super::*;
use crate::Channel;
use crate::git::{GitFacts, Head};
use crate::settings::Settings;
use crate::tab::{AgentStatus, Attention, Direction, Tab, TabAction};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::window_controls::Recorder;
use crate::workspace::{Fonts, Opening};

/// Two panes as a window would describe them: one focused and working, one
/// waiting on a question, in a group.
fn two_panes() -> Vec<PaneEntry> {
    vec![
        PaneEntry {
            pane_id: 3,
            tab_id: 2,
            title: "port the tab bar".to_owned(),
            tab_title: "port the tab bar".to_owned(),
            group: None,
            focused: true,
            status: "running".to_owned(),
            message: None,
            cwd: Some("/home/someone/work/crook".to_owned()),
            branch: Some("main".to_owned()),
        },
        PaneEntry {
            pane_id: 7,
            tab_id: 5,
            title: "sandbox the host".to_owned(),
            tab_title: "sandbox the host".to_owned(),
            group: Some("atlas".to_owned()),
            focused: false,
            status: "needs-input".to_owned(),
            message: Some("run rm -rf build?".to_owned()),
            cwd: None,
            branch: None,
        },
    ]
}

/// The verb a line asks for, and the code of the refusal when it asks for
/// none.
fn verdict(line: &str) -> (Option<Value>, Result<Verb, String>) {
    let (id, verb) = protocol::read(line.as_bytes());
    (id, verb.map_err(|refusal| refusal.code))
}

#[test]
fn a_request_and_its_answer_come_back_off_the_wire_as_they_went_on() {
    let line = protocol::request_line(Verb::PaneList);
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
    assert_eq!(verdict(line.trim_end()), (None, Ok(Verb::PaneList)));
    // Whatever the request called itself comes back with the answer, so a
    // client with several in flight can tell them apart.
    assert_eq!(
        verdict(r#"{"v":1,"id":{"n":7},"verb":"pane.list"}"#),
        (Some(json!({"n": 7})), Ok(Verb::PaneList))
    );

    let panes = two_panes();
    let reply = Reply::answered(
        Some(json!(7)),
        serde_json::to_value(&panes).expect("encodes"),
    );
    let line = reply.line();
    assert!(
        line.ends_with('\n') && line.matches('\n').count() == 1,
        "a reply is one line: {line:?}"
    );
    let read: Reply = serde_json::from_str(&line).expect("a reply reads back");
    assert_eq!(read, reply);
    assert_eq!(read.v, protocol::VERSION, "every reply says its version");
    let listed: Vec<PaneEntry> =
        serde_json::from_value(read.result.expect("an answer")).expect("a list of panes");
    assert_eq!(listed, panes);

    let refused = Reply::refused(None, Refusal::new(code::GONE, "closed"));
    let read: Value = serde_json::from_str(&refused.line()).expect("reads back");
    assert_eq!(
        read,
        json!({"v": protocol::VERSION, "ok": false, "error": {"code": "gone", "message": "closed"}}),
        "a refusal carries the version too, and nothing it has no value for"
    );
}

#[test]
fn a_verb_this_window_does_not_know_is_refused_by_name() {
    let (id, verb) = protocol::read(br#"{"v":1,"id":"a","verb":"pane.frobnicate"}"#);
    assert_eq!(
        id,
        Some(json!("a")),
        "a refusal is matched to its request too"
    );
    let refusal = verb.expect_err("not a verb");
    assert_eq!(refusal.code, code::UNKNOWN_VERB);
    assert!(
        refusal.message.contains("pane.frobnicate") && refusal.message.contains("pane.list"),
        "the refusal names what was asked and what there is: {}",
        refusal.message
    );
}

#[test]
fn a_request_that_is_not_one_is_refused_with_what_one_looks_like() {
    for line in [
        "pane.list",
        "[1, 2]",
        r#"{"verb":"pane.list"}"#,
        r#"{"v":"1","verb":"pane.list"}"#,
        r#"{"v":1}"#,
        r#"{"v":1,"verb":7}"#,
        r#"{"v":1,"min_version":"one","verb":"pane.list"}"#,
    ] {
        let (_, verb) = protocol::read(line.as_bytes());
        let refusal = verb.expect_err(line);
        assert_eq!(refusal.code, code::BAD_REQUEST, "{line}");
        assert!(
            refusal.message.contains(r#"{"v":1,"verb":"pane.list"}"#),
            "{line}: the refusal shows a request that works: {}",
            refusal.message
        );
    }
}

#[test]
fn a_request_the_versions_cannot_agree_on_is_refused_as_a_version() {
    assert_eq!(
        verdict(r#"{"v":0,"verb":"pane.list"}"#).1,
        Err(code::VERSION.to_owned()),
        "older than the oldest this window answers"
    );
    let (_, needs_more) = protocol::read(br#"{"v":1,"min_version":2,"verb":"pane.list"}"#);
    let refusal = needs_more.expect_err("this window is version 1");
    assert_eq!(refusal.code, code::VERSION);
    assert!(
        refusal.message.contains("update Crook"),
        "{}",
        refusal.message
    );

    // Versions add: a request written for a later one still means what it
    // says here, unless it says it needs the later one.
    assert_eq!(
        verdict(r#"{"v":2,"verb":"pane.list"}"#).1,
        Ok(Verb::PaneList)
    );
    assert_eq!(
        verdict(r#"{"v":1,"min_version":1,"verb":"pane.list"}"#).1,
        Ok(Verb::PaneList)
    );
}

#[test]
fn crook_pane_takes_list_and_json_and_refuses_anything_else() {
    let arguments = |words: &[&str]| {
        cli::list_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(arguments(&["list"]), Ok(false));
    assert_eq!(arguments(&["list", "--json"]), Ok(true));
    for (words, said) in [
        (&[][..], "needs a verb"),
        (&["lsit"][..], "takes list, not lsit"),
        (&["list", "--json", "--json"][..], "given twice"),
        (&["list", "--tsv"][..], "unrecognised argument --tsv"),
    ] {
        let refusal = arguments(words).expect_err("refused");
        assert!(refusal.contains(said), "{words:?}: {refusal}");
    }
}

#[test]
fn the_table_marks_the_focused_pane_and_puts_a_question_under_its_title() {
    let table = cli::table(&two_panes(), Some(Path::new("/home/someone")));
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "a heading, two rows and one question:\n{table}"
    );
    assert!(lines[0].starts_with("PANE  STATUS"), "{table}");
    assert!(
        lines[1].starts_with("3*"),
        "the focused pane is marked:\n{table}"
    );
    assert!(lines[1].ends_with("~/work/crook"), "{table}");
    assert!(lines[2].starts_with("7 "), "and no other:\n{table}");
    assert!(lines[2].contains("atlas"), "{table}");

    let title = lines[2]
        .find("sandbox the host")
        .expect("the row has its title");
    assert_eq!(
        lines[3].find("run rm -rf build?"),
        Some(title),
        "the question sits under the title it belongs to:\n{table}"
    );
    assert!(
        lines.iter().all(|line| line.trim_end() == *line),
        "no line trails spaces:\n{table}"
    );
}

/// A window with no display, carrying nothing but the tab strip.
struct Window {
    queue: Arc<LocalQueue>,
    app: App,
    workspace: ViewHandle<Workspace>,
}

impl Window {
    fn new() -> Self {
        let queue = LocalQueue::new();
        let mut app = App::new(queue.foreground(), Arc::new(Background::new(2)));
        let quits = Rc::new(Cell::new(0));
        let quit: crate::workspace::QuitRequest = Rc::new(move || quits.set(quits.get() + 1));
        let fonts = Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        };
        // No plugins: what `pane.list` answers is the strip's, and nothing a
        // plugin draws is in it.
        let (_, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                CellFont::headless(CELL_FONT_SIZE),
                Opening {
                    settings: Settings::ephemeral(),
                    channel: Channel::Dev,
                    plugins: Vec::new(),
                    withdrawn: Default::default(),
                    heard: Default::default(),
                    plugins_directory: None,
                },
                quit,
                Rc::new(Recorder::default()),
                ctx,
            )
        });
        Self {
            queue,
            app,
            workspace,
        }
    }

    /// What `pane.list` would answer now.
    fn panes(&self) -> Vec<PaneEntry> {
        self.workspace.read(&self.app, panes)
    }
}

/// A window with three tabs: the first split in two, a second on its own,
/// and a third opened into a group with the first — which puts it beside the
/// first, ahead of the second — and every pane saying something different.
///
/// Returns the window and the pane numbers in the panel's order.
fn busy_window() -> (Window, Vec<u64>) {
    let mut window = Window::new();
    window.workspace.update(&mut window.app, |workspace, ctx| {
        let first = workspace.tabs().iter().map(Tab::id).next().expect("a tab");
        let left = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::Split(Direction::Right), ctx);
        let right = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::New, ctx);
        let alone = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::NewInGroupOf(first), ctx);
        let grouped = workspace.tabs().focused_pane_id().expect("a pane");

        let checkout = PathBuf::from("/work/crook");
        workspace.update_session(left, ctx, |session| {
            session.derived_title = Some("port the tab bar".to_owned());
            session.status = AgentStatus::Running;
            session.working_directory = Some(checkout.clone());
        });
        workspace.update_session(right, ctx, |session| {
            session.custom_title = Some("the tests".to_owned());
            session.status = AgentStatus::NeedsInput;
            session.message = Some("run rm -rf build?".to_owned());
            session.working_directory = Some(PathBuf::from("/work/elsewhere"));
        });
        // The bell rang there while nobody was looking: the row's dot says
        // needs input, and the agent has said nothing.
        workspace.update_session(alone, ctx, |session| {
            session.attention = Some(Attention::Bell);
        });
        workspace.update_session(grouped, ctx, |session| {
            session.status = AgentStatus::Failed;
            session.working_directory = Some(PathBuf::from("/work/crook-atlas"));
        });
        workspace.git().update(ctx, |model, ctx| {
            model.record(
                checkout,
                GitFacts {
                    branch: Some(Head::Branch("main".to_owned())),
                    diff: None,
                    worktree: false,
                },
                ctx,
            )
        });
    });
    let ids = window.workspace.read(&window.app, |workspace, _| {
        workspace
            .tabs()
            .panes()
            .map(|(_, pane)| pane.id().as_u64())
            .collect()
    });
    (window, ids)
}

#[test]
fn pane_list_names_every_pane_by_the_number_its_shell_was_given() {
    let (window, ids) = busy_window();
    let panes = window.panes();

    // The number `CROOK_PANE_ID` carries, in the panel's order, and every
    // pane of a split rather than one a tab.
    assert_eq!(
        panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
        ids,
        "{panes:#?}"
    );
    assert_eq!(ids.len(), 4);
    let focused = window
        .workspace
        .read(&window.app, |workspace, _| {
            workspace.tabs().focused_pane_id()
        })
        .map(|pane| pane.as_u64());
    assert_eq!(
        panes
            .iter()
            .filter(|pane| pane.focused)
            .map(|pane| pane.pane_id)
            .collect::<Vec<_>>(),
        focused.into_iter().collect::<Vec<_>>(),
        "one pane is the one being worked in, and it is the window's focused one"
    );

    let [split_left, split_right, grouped, alone] = &panes[..] else {
        panic!("four panes: {panes:#?}");
    };
    assert_eq!(split_left.tab_id, split_right.tab_id, "one tab, split");
    assert_ne!(split_left.tab_id, alone.tab_id);

    assert_eq!(split_left.title, "port the tab bar");
    assert_eq!(split_left.status, "running");
    assert_eq!(split_left.cwd.as_deref(), Some("/work/crook"));
    assert_eq!(
        split_left.branch.as_deref(),
        Some("main"),
        "git has been asked there"
    );

    assert_eq!(split_right.title, "the tests");
    assert_eq!(
        split_right.status, "needs-input",
        "the word `crook --agent` takes"
    );
    assert_eq!(split_right.message.as_deref(), Some("run rm -rf build?"));
    assert_eq!(split_right.branch, None, "git has not been asked there");
    // The split's right half has the keyboard in that tab, so it is what a
    // row standing for the tab would say.
    assert_eq!(split_left.tab_title, "the tests");

    assert_eq!(grouped.status, "failed");
    assert_eq!(
        grouped.title, "crook-atlas",
        "a pane with no name of its own is called what its row calls it"
    );
    assert!(grouped.group.is_some(), "opened into a group: {grouped:#?}");
    assert_eq!(
        split_left.group, grouped.group,
        "the group it was opened into"
    );
    assert_eq!(alone.group, None);
    assert_eq!(
        alone.status, "idle",
        "the status is what the agent said, not the dot a bell turned amber"
    );
}

#[test]
fn a_question_to_a_window_that_has_closed_is_refused_as_gone_at_once() {
    let inbox = Arc::new(Inbox::default());
    let asking = inbox.clone();
    let started = Instant::now();
    let asked = std::thread::spawn(move || {
        asking.ask(Verb::PaneList, Instant::now() + Duration::from_secs(5))
    });
    std::thread::sleep(Duration::from_millis(50));
    inbox.close();
    let refusal = asked
        .join()
        .expect("the asking thread")
        .expect_err("nothing answered");
    assert_eq!(
        refusal.code,
        code::GONE,
        "a closed window says so rather than leaving the question to time out"
    );
    assert!(started.elapsed() < Duration::from_secs(4));

    // And one asked after it closed is refused before it is queued.
    let refusal = inbox
        .ask(Verb::PaneList, Instant::now() + Duration::from_secs(5))
        .expect_err("closed");
    assert_eq!(refusal.code, code::GONE);
}

#[test]
fn a_question_nobody_answers_is_refused_as_a_timeout() {
    let inbox = Inbox::default();
    let refusal = inbox
        .ask(Verb::PaneList, Instant::now() + Duration::from_millis(50))
        .expect_err("no window is serving this inbox");
    assert_eq!(refusal.code, code::TIMEOUT);
}

/// The socket, which is only Unix's.
#[cfg(unix)]
mod socket {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::super::protocol::{MAX_LINE, Refusal};
    use super::super::server::{self, Answer, Socket, converse};
    use super::*;

    /// A directory that removes itself, short enough that a socket's path in
    /// it fits the hundred-odd bytes a socket address holds on macOS.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "crook-cc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a temporary directory");
            Self(path)
        }

        /// Where a control directory would go, not yet made.
        fn sockets(&self) -> PathBuf {
            self.0.join("s")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A window that answers `pane.list` with these, and never looks at a
    /// tab strip.
    fn answering(panes: Vec<PaneEntry>) -> Answer {
        Arc::new(move |verb, _| match verb {
            Verb::PaneList => Ok(serde_json::to_value(&panes).expect("encodes")),
        })
    }

    /// The name this process asks for first.
    fn first_name(directory: &Path) -> PathBuf {
        directory.join(format!("{}.sock", std::process::id()))
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("there")
            .permissions()
            .mode()
            & 0o777
    }

    /// One end of a conversation, read a reply at a time.
    struct Client {
        stream: UnixStream,
        replies: BufReader<UnixStream>,
    }

    impl Client {
        fn on(stream: UnixStream) -> Self {
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("a timeout");
            let replies = BufReader::new(stream.try_clone().expect("a second handle"));
            Self { stream, replies }
        }

        fn connect(path: &Path) -> Self {
            Self::on(UnixStream::connect(path).expect("the socket answers"))
        }

        fn send(&mut self, line: &str) {
            self.stream.write_all(line.as_bytes()).expect("sent");
        }

        /// The next reply, or `None` once the window has closed its end.
        fn reply(&mut self) -> Option<Reply> {
            let mut line = String::new();
            match self.replies.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(serde_json::from_str(&line).expect("a reply")),
            }
        }
    }

    /// A conversation with no listener in front of it, and the thread
    /// answering it.
    fn conversation(answer: Answer, deadline: Duration) -> (Client, thread::JoinHandle<()>) {
        let (client, window) = UnixStream::pair().expect("a pair");
        let answering =
            thread::spawn(move || converse(&window, Instant::now() + deadline, &*answer));
        (Client::on(client), answering)
    }

    #[test]
    fn a_line_longer_than_the_cap_is_refused_and_the_connection_closed() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        // Written from a thread of its own, and more than the cap: the window
        // stops reading at the cap, and a write it no longer reads would
        // otherwise block the test on a small socket buffer.
        let mut writer = client.stream.try_clone().expect("a second handle");
        let writing = thread::spawn(move || {
            let _ = writer.write_all(&vec![b'x'; MAX_LINE * 2]);
        });

        let refusal = client.reply().expect("refused, not dropped");
        assert!(!refusal.ok);
        assert_eq!(refusal.v, protocol::VERSION);
        assert_eq!(refusal.error.expect("a refusal").code, code::TOO_LONG);
        assert!(client.reply().is_none(), "and the connection is closed");
        answering.join().expect("the connection's thread");
        writing.join().expect("the writer");
    }

    #[test]
    fn a_line_exactly_as_long_as_the_cap_is_answered() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        let request = r#"{"v":1,"verb":"pane.list"}"#;
        let line = format!("{request}{}\n", " ".repeat(MAX_LINE - request.len()));
        assert_eq!(line.len(), MAX_LINE + 1);
        client.send(&line);
        let reply = client.reply().expect("answered");
        assert!(reply.ok, "{reply:?}");
        // Read whole, newline and all: the connection is still there for the
        // next line, as it would not be after one that was cut short.
        client.send(&protocol::request_line(Verb::PaneList));
        assert!(client.reply().expect("the next line answered too").ok);
        drop(client);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn an_unknown_verb_is_refused_and_the_next_line_still_answered() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        client.send(
            "{\"v\":1,\"id\":\"a\",\"verb\":\"pane.frobnicate\"}\n\n{\"v\":1,\"id\":\"b\",\"verb\":\"pane.list\"}\n",
        );

        let refused = client.reply().expect("a refusal, not a closed connection");
        assert_eq!(refused.id, Some(json!("a")));
        assert_eq!(refused.error.expect("refused").code, code::UNKNOWN_VERB);
        let answered = client.reply().expect("the next line is still read");
        assert_eq!(answered.id, Some(json!("b")));
        assert_eq!(
            answered.result,
            Some(serde_json::to_value(two_panes()).expect("encodes"))
        );

        drop(client);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn a_connection_that_says_nothing_is_closed_at_its_deadline() {
        let (mut client, answering) =
            conversation(answering(two_panes()), Duration::from_millis(100));
        let started = Instant::now();
        assert!(
            client.reply().is_none(),
            "nothing is answered, and the line closes"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "closed at the deadline, not by the client's own timeout"
        );
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn the_socket_is_this_users_alone() {
        let root = TempDir::new();
        let directory = root.sockets();
        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");

        assert_eq!(mode(&directory), 0o700, "nobody else can reach inside");
        assert_eq!(socket.path(), first_name(&directory));
        assert!(
            fs::symlink_metadata(socket.path())
                .expect("there")
                .file_type()
                .is_socket()
        );
        assert_eq!(
            mode(socket.path()),
            0o600,
            "and the socket is locked as well"
        );

        let mut client = Client::connect(socket.path());
        client.send(&protocol::request_line(Verb::PaneList));
        assert!(client.reply().expect("answered").ok);
    }

    #[test]
    fn a_directory_anyone_else_could_have_written_is_refused() {
        let root = TempDir::new();

        let open = root.sockets();
        fs::create_dir(&open).expect("made");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("loosened");
        assert!(Socket::open_in(&open, answering(Vec::new())).is_err());
        assert_eq!(
            fs::read_dir(&open).expect("there").count(),
            0,
            "nothing is left in a directory that was refused"
        );

        let private = root.0.join("private");
        fs::create_dir(&private).expect("made");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("closed");
        let link = root.0.join("link");
        std::os::unix::fs::symlink(&private, &link).expect("linked");
        assert!(
            Socket::open_in(&link, answering(Vec::new())).is_err(),
            "a link is refused as a link, whatever it points at"
        );
        assert_eq!(fs::read_dir(&private).expect("there").count(), 0);

        let file = root.0.join("file");
        fs::write(&file, "").expect("written");
        assert!(Socket::open_in(&file, answering(Vec::new())).is_err());

        // Somebody else's: a test cannot give a directory away without being
        // root, so it asks as the next uid instead.
        assert!(server::check(&private, server::euid()).is_ok());
        let theirs = server::check(&private, server::euid() + 1).expect_err("not ours");
        assert!(theirs.to_string().contains("belongs to"), "{theirs}");
    }

    #[test]
    fn a_socket_left_by_a_crook_that_died_is_taken_back() {
        let root = TempDir::new();
        let directory = root.sockets();
        fs::create_dir(&directory).expect("made");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("closed");
        // Bound and dropped: the file stays and nothing listens, which is
        // what a Crook that was killed leaves.
        drop(UnixListener::bind(first_name(&directory)).expect("bound"));
        assert!(first_name(&directory).exists());

        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");
        assert_eq!(
            socket.path(),
            first_name(&directory),
            "the name is taken back"
        );
        let mut client = Client::connect(socket.path());
        client.send(&protocol::request_line(Verb::PaneList));
        assert!(client.reply().expect("and it is this window answering").ok);
    }

    #[test]
    fn a_socket_somebody_is_listening_on_is_not_taken() {
        let root = TempDir::new();
        let directory = root.sockets();
        fs::create_dir(&directory).expect("made");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("closed");
        let theirs = UnixListener::bind(first_name(&directory)).expect("bound");

        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");
        assert_eq!(
            socket.path(),
            directory.join(format!("{}-1.sock", std::process::id())),
            "the next name, and not the live one"
        );
        let _knocking = UnixStream::connect(first_name(&directory)).expect("still theirs");
        theirs
            .accept()
            .expect("the connection reached them, not this window");
    }

    #[test]
    fn the_socket_goes_when_the_window_does_and_only_its_own() {
        let root = TempDir::new();
        let directory = root.sockets();

        let socket = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let path = socket.path().to_owned();
        drop(socket);
        assert!(!path.exists(), "the file goes with the window");

        // A socket somebody else has since put at the same name is theirs.
        let socket = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let path = socket.path().to_owned();
        fs::remove_file(&path).expect("removed from under it");
        let _theirs = UnixListener::bind(&path).expect("bound at the same name");
        drop(socket);
        assert!(path.exists(), "a drop removes only the file it bound");
    }

    #[test]
    fn the_sweep_takes_dead_sockets_and_leaves_everything_else() {
        let root = TempDir::new();
        let directory = root.0.clone();
        let dead = directory.join("100.sock");
        drop(UnixListener::bind(&dead).expect("bound"));
        let live = directory.join("200.sock");
        let _listening = UnixListener::bind(&live).expect("bound");
        let file = directory.join("300.sock");
        fs::write(&file, "").expect("written");

        server::sweep(&directory, server::STALE_AFTER);
        assert!(
            dead.exists(),
            "a socket bound a moment ago may be about to listen"
        );

        server::sweep(&directory, Duration::ZERO);
        assert!(!dead.exists(), "nobody was listening");
        assert!(live.exists(), "somebody is");
        assert!(file.exists(), "not a socket, and not the sweep's");
    }

    #[test]
    fn the_runtime_directory_is_used_only_when_it_is_this_users() {
        let root = TempDir::new();
        let temp = Path::new("/tmp-of-this-test");
        let uid = server::euid();
        let fallback = temp.join(format!("crook-control-{uid}"));

        assert_eq!(
            server::directory_for(Some(root.0.clone().into()), temp, uid),
            root.0.join("crook-control")
        );
        for (runtime, why) in [
            (None, "unset"),
            (Some("run/user/1000".into()), "relative"),
            (Some(root.0.join("missing").into()), "not there"),
        ] {
            assert_eq!(server::directory_for(runtime, temp, uid), fallback, "{why}");
        }
        assert_eq!(
            server::directory_for(Some(root.0.clone().into()), temp, uid + 1),
            temp.join(format!("crook-control-{}", uid + 1)),
            "somebody else's runtime directory is not this user's"
        );
    }

    #[test]
    fn a_ninth_connection_is_turned_away_as_busy() {
        let root = TempDir::new();
        let socket = Socket::open_in(&root.sockets(), answering(Vec::new())).expect("opens");
        // Each of these holds its connection's thread until it closes or its
        // five seconds are up, and says nothing.
        let holding: Vec<UnixStream> = (0..server::MAX_CONNECTIONS)
            .map(|_| UnixStream::connect(socket.path()).expect("admitted"))
            .collect();

        let mut turned_away = Client::connect(socket.path());
        let refusal = turned_away.reply().expect("refused, not dropped");
        assert_eq!(refusal.error.expect("a refusal").code, code::BUSY);

        drop(holding);
    }

    #[test]
    fn the_command_line_prints_what_the_window_answered() {
        let root = TempDir::new();
        let socket = Socket::open_in(&root.sockets(), answering(two_panes())).expect("opens");

        let json = cli::unix::listing(socket.path(), true, None).expect("answered");
        let read: Value = serde_json::from_str(&json).expect("the output is JSON");
        assert_eq!(read, serde_json::to_value(two_panes()).expect("encodes"));

        let table = cli::unix::listing(socket.path(), false, Some(Path::new("/home/someone")))
            .expect("answered");
        assert_eq!(
            table,
            cli::table(&two_panes(), Some(Path::new("/home/someone")))
        );
    }

    #[test]
    fn the_command_line_says_what_a_refusal_said() {
        let root = TempDir::new();
        let socket = Socket::open_in(
            &root.sockets(),
            Arc::new(|_, _| {
                Err(Refusal::new(
                    code::TIMEOUT,
                    "the window did not answer in time",
                ))
            }),
        )
        .expect("opens");

        let refusal = cli::unix::listing(socket.path(), true, None).expect_err("refused");
        let said = refusal.to_string();
        assert!(
            said.contains("did not answer in time") && said.contains("timeout"),
            "{said}"
        );

        let missing = root.0.join("nobody.sock");
        let refusal = cli::unix::listing(&missing, true, None).expect_err("nothing there");
        assert!(
            refusal.to_string().contains("nothing is answering"),
            "{refusal}"
        );
    }

    #[test]
    fn outside_a_pane_the_command_line_asks_the_only_window_and_will_not_guess_among_several() {
        let root = TempDir::new();
        let directory = root.sockets();
        let socket_from = |socket: Option<&str>, pane: Option<&str>| {
            cli::unix::socket_from(socket.map(Into::into), pane.map(Into::into), &directory)
                .map_err(|error| error.to_string())
        };

        // Inside a pane, the pane's own window, whatever else is running.
        assert_eq!(
            socket_from(Some("/run/user/1000/crook-control/42.sock"), Some("3")),
            Ok(PathBuf::from("/run/user/1000/crook-control/42.sock"))
        );
        let empty = socket_from(Some(""), Some("3")).expect_err("that window has no socket");
        assert!(empty.contains("has no control socket"), "{empty}");
        let older = socket_from(None, Some("3")).expect_err("a Crook from before the socket");
        assert!(older.contains("older"), "{older}");

        let none = socket_from(None, None).expect_err("nothing running");
        assert!(none.contains("no Crook is running"), "{none}");

        let first = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        drop(UnixListener::bind(directory.join("1.sock")).expect("a dead one beside it"));
        assert_eq!(
            socket_from(None, None),
            Ok(first.path().to_owned()),
            "the one live window, and not the dead socket beside it"
        );

        let second = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let several = socket_from(None, None).expect_err("two windows");
        assert!(
            several.contains(&first.path().display().to_string())
                && several.contains(&second.path().display().to_string()),
            "the refusal names them: {several}"
        );

        drop((first, second));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("loosened");
        let _stranger = UnixListener::bind(directory.join("9.sock")).expect("bound");
        assert!(
            socket_from(None, None).is_err(),
            "a socket in a directory anyone could write to is nobody's to trust"
        );
    }

    #[test]
    fn a_script_asks_a_real_window_through_the_socket_and_the_command_line() {
        let root = TempDir::new();
        let (mut window, _) = busy_window();
        let control = Control::open_in(&root.sockets()).expect("opens");
        window
            .workspace
            .update(&mut window.app, |_, ctx| control.serve(ctx));
        let path = control.path().expect("a socket").to_owned();

        // Twice, one after the other, the way a script polling the window
        // would: the window has to be listening again after it answered.
        let asking = thread::spawn(move || {
            let json = cli::unix::listing(&path, true, None)?;
            let table = cli::unix::listing(&path, false, None)?;
            anyhow::Ok((json, table))
        });
        // The window's side runs here, on the thread that owns it, the way
        // the event loop would run it: between two frames.
        let started = Instant::now();
        while !asking.is_finished() {
            window.queue.run_until_parked();
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the window never answered"
            );
            thread::yield_now();
        }
        let (json, table) = asking.join().expect("the script").expect("answered");

        let read: Vec<PaneEntry> = serde_json::from_str(&json).expect("a list of panes");
        assert_eq!(read, window.panes());
        assert_eq!(read.len(), 4);
        assert_eq!(
            table,
            cli::table(&read, None),
            "and answered the second time"
        );
    }
}
