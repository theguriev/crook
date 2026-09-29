//! The Changes column's state, with no window and no repository: what it
//! lists, what it asks git for and when, and which of the answers it takes.
//!
//! What a frame of it draws, and what a press on it does, is in the
//! workspace's own tests, which drive the real view tree.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;
use crate::git::changes::{Commits, FileChange};

/// An overview of a task on `main` whose files are `paths`, all modified.
fn overview(paths: &[&str]) -> Overview {
    Overview {
        repository: PathBuf::from("/work/repo"),
        base: Base {
            name: "main".to_owned(),
            fork: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            against: Against::Branch,
        },
        commits: Commits::default(),
        files: paths
            .iter()
            .map(|path| FileChange {
                status: Status::Modified,
                path: PathBuf::from(path),
                from: None,
            })
            .collect(),
        more_files: false,
    }
}

/// A column that is up and has read `overview`.
fn showing(overview: Overview) -> ChangesPanelState {
    let mut state = ChangesPanelState::default();
    let epoch = state
        .open(Some(overview.repository.clone()), None)
        .expect("there is somewhere to read");
    state
        .land(epoch, Ok(overview))
        .expect("the answer is current");
    state
}

/// A diff of one changed line.
fn one_line() -> FileDiff {
    FileDiff {
        patch: "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n".to_owned(),
        lines: vec![
            "@@ -1 +1 @@".to_owned(),
            "-old".to_owned(),
            "+new".to_owned(),
        ],
        binary: false,
        cut: false,
        first_line: Some(1),
    }
}

// --- reading -------------------------------------------------------------------

#[test]
fn a_file_shown_hidden_and_shown_again_is_read_once() {
    let mut state = showing(overview(&["a.rs", "b.rs"]));

    let read = state.toggle(0).expect("the first press asks for the diff");
    assert_eq!(read.file.path, Path::new("a.rs"));
    assert!(state.land_hunks(Path::new("a.rs"), read.ticket, Ok(one_line())));

    assert!(state.toggle(0).is_none(), "hiding it asked git for it");
    assert!(
        state.toggle(0).is_none(),
        "showing it again asked git for it again"
    );
    assert_eq!(state.hunk_reads(), 1);
    assert!(
        state.rows().contains(&Row::Line(0, 2)),
        "{:?}",
        state.rows()
    );
}

#[test]
fn a_file_pressed_again_before_its_diff_arrives_is_not_asked_for_twice() {
    let mut state = showing(overview(&["a.rs"]));

    let read = state.toggle(0).expect("the first press asks for the diff");
    state.toggle(0);
    assert!(
        state.toggle(0).is_none(),
        "a second read was started while the first was on its way"
    );
    assert!(state.land_hunks(Path::new("a.rs"), read.ticket, Ok(one_line())));
    assert_eq!(state.hunk_reads(), 1);
}

#[test]
fn a_diff_that_could_not_be_read_is_asked_for_again_next_time() {
    let mut state = showing(overview(&["a.rs"]));

    let read = state.toggle(0).expect("asked");
    state.land_hunks(
        Path::new("a.rs"),
        read.ticket,
        Err("git fell over".to_owned()),
    );
    assert!(
        state
            .rows()
            .iter()
            .any(|row| matches!(row, Row::Note(note) if note.contains("git fell over"))),
        "the failure is not said: {:?}",
        state.rows()
    );

    state.toggle(0);
    assert!(
        state.toggle(0).is_some(),
        "a failed read is never tried again"
    );
}

#[test]
fn an_answer_to_a_question_nobody_is_asking_any_more_is_dropped() {
    let mut state = ChangesPanelState::default();
    let first = state
        .open(Some(PathBuf::from("/work/one")), None)
        .expect("somewhere to read");
    let second = state
        .retarget(Some(PathBuf::from("/work/two")))
        .expect("somewhere to read");

    assert!(
        state.land(first, Ok(overview(&["stale.rs"]))).is_none(),
        "the first repository's answer was taken for the second's"
    );
    assert!(state.overview().is_none());
    assert!(state.land(second, Ok(overview(&["fresh.rs"]))).is_some());
}

#[test]
fn a_diff_asked_for_in_another_repository_is_not_shown_in_this_one() {
    let mut state = showing(overview(&["a.rs"]));
    let read = state.toggle(0).expect("asked");

    let epoch = state
        .retarget(Some(PathBuf::from("/work/elsewhere")))
        .expect("somewhere to read");
    state.land(epoch, Ok(overview(&["a.rs"])));

    assert!(!state.land_hunks(Path::new("a.rs"), read.ticket, Ok(one_line())));
    assert!(!state.is_expanded(Path::new("a.rs")));
}

#[test]
fn a_refresh_reads_again_the_diffs_that_are_showing_and_forgets_the_rest() {
    let mut state = showing(overview(&["a.rs", "b.rs"]));
    let shown = state.toggle(0).expect("asked");
    state.land_hunks(Path::new("a.rs"), shown.ticket, Ok(one_line()));
    let hidden = state.toggle(1).expect("asked");
    state.land_hunks(Path::new("b.rs"), hidden.ticket, Ok(one_line()));
    state.toggle(1);

    let epoch = state.refresh().expect("nothing is being read");
    assert!(state.refresh().is_none(), "a refresh doubled up on a read");
    let again = state
        .land(epoch, Ok(overview(&["a.rs", "b.rs"])))
        .expect("the answer is current");

    let paths: Vec<&Path> = again.iter().map(|read| read.file.path.as_path()).collect();
    assert_eq!(paths, [Path::new("a.rs")]);
    // Still drawn while the new one is read, rather than blinking to
    // "Reading…" every fifteen seconds.
    assert!(
        state.rows().contains(&Row::Line(0, 2)),
        "{:?}",
        state.rows()
    );
    assert!(
        state.toggle(1).is_some(),
        "a hidden file's old diff was kept"
    );
}

// --- the rows ------------------------------------------------------------------

#[test]
fn the_rows_are_the_base_the_commits_and_the_files_in_that_order() {
    let mut listed = overview(&["a.rs"]);
    listed.commits = Commits {
        commits: vec![crate::git::changes::Commit {
            sha: "abc1234".to_owned(),
            subject: "the work".to_owned(),
            when: "an hour ago".to_owned(),
        }],
        more: false,
    };
    let state = showing(listed);

    assert_eq!(
        state.rows(),
        [
            Row::Heading("1 commit since main".to_owned()),
            Row::Commit(0),
            Row::Heading("1 file changed".to_owned()),
            Row::File(0),
        ]
    );
}

#[test]
fn a_binary_file_says_so_where_its_lines_would_be() {
    let mut state = showing(overview(&["picture.png"]));
    let read = state.toggle(0).expect("asked");
    state.land_hunks(
        Path::new("picture.png"),
        read.ticket,
        Ok(FileDiff {
            binary: true,
            ..FileDiff::default()
        }),
    );

    // Under "No commits since main", "1 file changed" and the file itself.
    assert_eq!(
        state.rows()[3..],
        [
            Row::Actions(0),
            Row::Note("A binary file: no lines to show.".to_owned())
        ]
    );
}

// --- the window ----------------------------------------------------------------

#[test]
fn the_rows_in_view_are_the_ones_under_the_offset_however_long_the_list() {
    let paths: Vec<String> = (0..5_000).map(|n| format!("src/file-{n:04}.rs")).collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    let state = showing(overview(&paths));
    // The two headings, then five thousand files.
    assert_eq!(state.rows().len(), 5_002);

    let top = state.visible(0., 480., 480.);
    assert_eq!(top.start, 0);
    assert!(
        top.len() <= 480. as usize / height::FILE as usize + 2,
        "{top:?} is more than a screenful"
    );

    // A thousand files down: a binary search, not a walk from the top.
    let offset = 2. * height::HEADING + 1_000. * height::FILE;
    let middle = state.visible(offset, 480., 480.);
    assert_eq!(
        middle.start, 1_002,
        "the first row in view is not file 1000"
    );
    assert_eq!(middle.len(), top.len());

    // Past the end — the list got shorter under the offset — builds the
    // last screenful rather than nothing.
    let bottom = state.visible(state.total() * 2., 480., 480.);
    assert_eq!(bottom.end, 5_002);
    assert!(!bottom.is_empty());
}

// --- the editor ----------------------------------------------------------------

#[test]
fn visual_is_read_before_editor() {
    let editor = Editor::from_variables(Some("code -w"), Some("gedit")).expect("an editor");
    assert_eq!(editor.name(), "code");
}

#[test]
fn an_empty_visual_leaves_it_to_editor() {
    let editor = Editor::from_variables(Some("  "), Some("gedit")).expect("an editor");
    assert_eq!(editor.name(), "gedit");
}

#[test]
fn neither_set_names_no_editor_and_the_button_is_not_offered() {
    assert_eq!(Editor::from_variables(None, None), None);
    assert_eq!(Editor::from_variables(Some(""), Some("")), None);
}

#[test]
fn an_editor_that_needs_a_terminal_is_not_offered() {
    // Started with no terminal, Neovim waits for one for ever: a press would
    // do nothing on screen and leave a process behind.
    for named in [
        "nvim",
        "/usr/bin/vim",
        "vi",
        "nano",
        "hx",
        "emacs -nw",
        "emacsclient -t",
    ] {
        assert_eq!(
            Editor::from_variables(Some(named), None),
            None,
            "{named} was offered"
        );
    }
    // And the variable that decides is the first one set: an `$EDITOR` set
    // for some other tool is not the editor somebody who set `$VISUAL=vim`
    // asked for.
    assert_eq!(Editor::from_variables(Some("vim"), Some("code")), None);
    assert!(Editor::from_variables(Some("emacs"), None).is_some());
}

#[test]
fn each_family_of_editor_is_told_the_line_its_own_way() {
    let path = Path::new("/work/repo/src/main.rs");
    let spelled = |named: &str| {
        let (program, arguments) = Editor::from_variables(Some(named), None)
            .expect("an editor")
            .command_line(path, 42);
        (program, arguments)
    };
    let words = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();

    assert_eq!(
        spelled("code -w"),
        (
            "code".to_owned(),
            words(&["-w", "--goto", "/work/repo/src/main.rs:42"])
        )
    );
    assert_eq!(
        spelled("zed"),
        ("zed".to_owned(), words(&["/work/repo/src/main.rs:42"]))
    );
    assert_eq!(
        spelled("idea"),
        (
            "idea".to_owned(),
            words(&["--line", "42", "/work/repo/src/main.rs"])
        )
    );
    assert_eq!(
        spelled("gvim --remote-silent"),
        (
            "gvim".to_owned(),
            words(&["--remote-silent", "+42", "/work/repo/src/main.rs"])
        )
    );
}
