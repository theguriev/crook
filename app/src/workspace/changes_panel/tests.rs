//! The Changes column's state, with no window and no repository: what it
//! lists, what it asks git for and when, and which of the answers it takes.
//!
//! What a frame of it draws, and what a press on it does, is in the
//! workspace's own tests, which drive the real view tree.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;
use crate::git::changes::{Commits, FileChange};
use crate::tab::TabId;

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

/// A column that is up and has read `overview`, for a tab of its own.
fn showing(overview: Overview) -> ChangesPanelState {
    showing_for(TabId::next(), overview)
}

/// A column that is up and has read `overview`, for `tab`.
fn showing_for(tab: TabId, overview: Overview) -> ChangesPanelState {
    let mut state = ChangesPanelState::default();
    let epoch = state
        .open(Some(tab), Some(overview.repository.clone()), None)
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
    let tab = Some(TabId::next());
    let first = state
        .open(tab, Some(PathBuf::from("/work/one")), None)
        .expect("somewhere to read");
    let second = state
        .retarget(tab, Some(PathBuf::from("/work/two")))
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
        .retarget(state.tab, Some(PathBuf::from("/work/elsewhere")))
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

#[test]
fn a_nested_repository_is_shown_without_asking_git_for_a_diff() {
    let mut listed = overview(&["src/a.rs"]);
    listed.files.push(FileChange {
        status: Status::Repository,
        path: PathBuf::from("vendored/"),
        from: None,
    });
    let mut state = showing(listed);

    assert!(
        state.toggle(1).is_none(),
        "a diff was asked for a directory git does not look inside"
    );
    assert_eq!(state.hunk_reads(), 0);
    assert!(
        state
            .rows()
            .iter()
            .any(|row| matches!(row, Row::Note(note) if note.contains("repository of its own"))),
        "{:?}",
        state.rows()
    );
}

#[test]
fn an_editor_that_did_not_start_says_so_under_the_file() {
    let mut state = showing(overview(&["a.rs", "b.rs"]));
    let read = state.toggle(0).expect("asked");
    state.land_hunks(Path::new("a.rs"), read.ticket, Ok(one_line()));

    state.opened(
        Path::new("a.rs"),
        Err("Could not start code: program not found".to_owned()),
    );
    let said = Row::Note("Could not start code: program not found".to_owned());
    assert_eq!(
        state.rows()[3..5],
        [Row::Actions(0), said.clone()],
        "the failure is not under the file's buttons"
    );

    state.opened(Path::new("a.rs"), Ok(()));
    assert!(
        !state.rows().contains(&said),
        "a later start that worked left the failure up"
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
    assert_eq!(
        spelled("notepad++"),
        (
            "notepad++".to_owned(),
            words(&["-n42", "/work/repo/src/main.rs"])
        )
    );
}

#[test]
fn an_editor_nobody_taught_this_the_line_for_is_handed_the_path_alone() {
    // A wrapper — Omarchy's launcher starts VS Code — or an editor with a
    // convention of its own reads `+42` as a second file to open. The top of
    // the right file beats that.
    for named in ["omarchy-launch-editor --inline", "open -t", "xdg-open"] {
        let editor = Editor::from_variables(Some(named), None).expect("an editor");
        let (_, arguments) = editor.command_line(Path::new("/work/repo/src/main.rs"), 42);
        assert_eq!(
            arguments.last(),
            Some(&OsString::from("/work/repo/src/main.rs")),
            "{named}"
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.to_string_lossy().contains("42")),
            "{named} was told a line it may not read: {arguments:?}"
        );
    }
}

// --- review --------------------------------------------------------------------

/// A diff of one hunk: a line of context, one removed and two added.
fn a_hunk() -> FileDiff {
    diff_of(&[
        "@@ -10,3 +10,4 @@ fn main() {",
        "     let a = 1;",
        "-    let b = 2;",
        "+    let b = 3;",
        "+    let c = 4;",
        "     done();",
    ])
}

/// A diff whose drawn lines are `lines`.
fn diff_of(lines: &[&str]) -> FileDiff {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    FileDiff {
        patch: lines.join("\n"),
        first_line: None,
        lines,
        binary: false,
        cut: false,
    }
}

/// A column showing `path`'s diff, read from `diff`.
fn showing_diff(path: &str, diff: FileDiff) -> ChangesPanelState {
    let mut state = showing(overview(&[path]));
    let read = state.toggle(0).expect("the first press asks for the diff");
    assert!(state.land_hunks(Path::new(path), read.ticket, Ok(diff)));
    state
}

/// Leaves `text` as a comment on line `line` of file `file`, the way the
/// field would: a press on the line, the words, Enter.
fn comment(state: &mut ChangesPanelState, file: usize, line: usize, text: &str) {
    assert!(
        state.start_comment(file, line),
        "line {line} took no comment"
    );
    state
        .draft_input()
        .expect("a press on a line opens a field under it")
        .edit(|editor| editor.paste(text));
    assert!(state.add_comment(), "Enter did not add {text:?}");
}

/// The comments, as where they are and what they say.
fn said(state: &ChangesPanelState) -> Vec<(usize, String)> {
    state
        .comments()
        .iter()
        .map(|comment| (comment.anchor.at, comment.text.clone()))
        .collect()
}

/// The same file read again, as a refresh reads it.
fn refresh_with(state: &mut ChangesPanelState, paths: &[&str], diff: FileDiff) {
    let epoch = state.refresh().expect("nothing is being read");
    let again = state
        .land(epoch, Ok(overview(paths)))
        .expect("the answer is current");
    let read = again
        .into_iter()
        .find(|read| read.file.path == Path::new("a.rs"))
        .expect("a file shown or commented on is read again");
    state.land_hunks(Path::new("a.rs"), read.ticket, Ok(diff));
}

#[test]
fn lines_are_numbered_from_their_hunk_header_on_the_side_they_are_on() {
    let places = review::places(&a_hunk().lines);
    assert_eq!(
        places,
        [
            Some(review::Place::Hunk(10)),
            None,
            Some(review::Place::Removed(11)),
            Some(review::Place::Added(11)),
            Some(review::Place::Added(12)),
            None,
        ]
    );
}

#[test]
fn a_comment_is_drawn_under_its_line_and_its_cross_takes_it_away() {
    let mut state = showing_diff("a.rs", a_hunk());

    comment(&mut state, 0, 3, "why three?");
    assert_eq!(said(&state), [(3, "why three?".to_owned())]);
    let id = state.comments()[0].id;
    // The heading, the file, its buttons, then the hunk: line 3 is the
    // eighth row, and the comment is the row under it.
    assert_eq!(
        state.rows()[7..10],
        [Row::Line(0, 3), Row::Comment(0, id), Row::Line(0, 4)]
    );
    assert!(
        state.draft_input().is_none(),
        "the field stayed up after Enter"
    );

    assert!(state.remove_comment(id));
    assert!(state.comments().is_empty());
    assert!(!state.rows().contains(&Row::Comment(0, id)));
}

#[test]
fn a_comment_left_empty_or_cancelled_is_not_kept() {
    let mut state = showing_diff("a.rs", a_hunk());

    assert!(state.start_comment(0, 3));
    assert!(state.rows().contains(&Row::Draft(0)));
    assert!(!state.add_comment(), "an empty comment was added");
    assert!(state.comments().is_empty());

    assert!(state.start_comment(0, 3));
    state
        .draft_input()
        .expect("the field is up")
        .edit(|editor| editor.paste("never mind"));
    state.cancel_comment();
    assert!(state.comments().is_empty(), "Escape kept the comment");
    assert!(!state.rows().contains(&Row::Draft(0)));
}

#[test]
fn only_a_changed_line_or_a_hunk_header_takes_a_comment() {
    let mut state = showing_diff("a.rs", a_hunk());

    assert!(
        !state.start_comment(0, 1),
        "a line of context took a comment"
    );
    assert!(
        !state.start_comment(0, 5),
        "a line of context took a comment"
    );
    assert!(state.start_comment(0, 0), "the hunk's header took none");
    assert!(state.start_comment(0, 2), "a removed line took none");
}

#[test]
fn a_refresh_moves_a_comment_with_its_line() {
    let mut state = showing_diff("a.rs", a_hunk());
    comment(&mut state, 0, 3, "why three?");
    comment(&mut state, 0, 0, "the whole hunk");

    // The agent wrote two lines above the one commented on, which moves it
    // down the diff and down the file.
    refresh_with(
        &mut state,
        &["a.rs"],
        diff_of(&[
            "@@ -10,3 +10,6 @@ fn main() {",
            "     let a = 1;",
            "+    let x = 0;",
            "+    let y = 0;",
            "-    let b = 2;",
            "+    let b = 3;",
            "+    let c = 4;",
            "     done();",
        ]),
    );

    let why = &state.comments()[0];
    assert_eq!(why.anchor.at, 5, "the comment stayed where its line was");
    assert_eq!(why.anchor.place, review::Place::Added(13));
    // The header's numbers changed and its function did not: the hunk is
    // the same hunk.
    assert_eq!(state.comments()[1].anchor.at, 0);
    assert!(
        state.rows().contains(&Row::Comment(0, why.id)),
        "{:?}",
        state.rows()
    );
    assert_eq!(state.note(), None, "a comment that moved was said dropped");
}

#[test]
fn a_refresh_drops_a_comment_whose_line_is_gone_and_says_so() {
    let mut state = showing_diff("a.rs", a_hunk());
    comment(&mut state, 0, 3, "why three?");
    comment(&mut state, 0, 4, "and c?");

    // The agent took the comment's advice before it was sent.
    refresh_with(
        &mut state,
        &["a.rs"],
        diff_of(&[
            "@@ -10,3 +10,3 @@ fn main() {",
            "     let a = 1;",
            "-    let b = 2;",
            "+    let c = 4;",
            "     done();",
        ]),
    );

    assert_eq!(said(&state), [(3, "and c?".to_owned())]);
    assert_eq!(
        state.note(),
        Some("Dropped the comment on a.rs:11: that line is no longer in the diff.")
    );
}

#[test]
fn a_comment_being_typed_on_a_line_that_is_gone_is_taken_down_and_said() {
    let mut state = showing_diff("a.rs", a_hunk());
    assert!(state.start_comment(0, 4));

    refresh_with(
        &mut state,
        &["a.rs"],
        diff_of(&[
            "@@ -10,3 +10,3 @@ fn main() {",
            "-    let b = 2;",
            "+    let b = 3;",
        ]),
    );
    assert!(
        state.draft_input().is_none(),
        "the field is up under nothing"
    );
    assert!(!state.rows().contains(&Row::Draft(0)));
    assert!(
        state.note().is_some_and(|note| note.contains("not added")),
        "{:?}",
        state.note()
    );
}

#[test]
fn a_file_that_no_longer_differs_takes_its_comments_with_it() {
    let mut state = showing(overview(&["a.rs", "b.rs"]));
    let read = state.toggle(0).expect("asked");
    state.land_hunks(Path::new("a.rs"), read.ticket, Ok(a_hunk()));
    comment(&mut state, 0, 3, "why three?");

    let epoch = state.refresh().expect("nothing is being read");
    let again = state
        .land(epoch, Ok(overview(&["b.rs"])))
        .expect("the answer is current");

    assert!(again.is_empty(), "a file that is gone was read again");
    assert!(state.comments().is_empty());
    assert!(
        state.note().is_some_and(|note| note.contains("a.rs:11")),
        "{:?}",
        state.note()
    );
}

#[test]
fn a_folded_file_with_comments_is_read_again_on_a_refresh() {
    let mut state = showing_diff("a.rs", a_hunk());
    comment(&mut state, 0, 3, "why three?");
    state.toggle(0);

    let epoch = state.refresh().expect("nothing is being read");
    let again = state
        .land(epoch, Ok(overview(&["a.rs"])))
        .expect("the answer is current");
    assert_eq!(
        again
            .iter()
            .map(|read| read.file.path.as_path())
            .collect::<Vec<_>>(),
        [Path::new("a.rs")],
        "a comment on a folded file would be sent about a diff nobody read again"
    );
}

#[test]
fn each_tab_has_its_own_review_and_finds_it_again() {
    let first = TabId::next();
    let second = TabId::next();
    let mut state = showing_for(first, overview(&["a.rs"]));
    let read = state.toggle(0).expect("asked");
    state.land_hunks(Path::new("a.rs"), read.ticket, Ok(a_hunk()));
    comment(&mut state, 0, 3, "why three?");

    let epoch = state
        .retarget(Some(second), Some(PathBuf::from("/work/repo")))
        .expect("somewhere to read");
    state.land(epoch, Ok(overview(&["a.rs"])));
    assert!(
        state.comments().is_empty(),
        "another tab on the same repository showed the first tab's comments"
    );

    let epoch = state
        .retarget(Some(first), Some(PathBuf::from("/work/repo")))
        .expect("somewhere to read");
    state.land(epoch, Ok(overview(&["a.rs"])));
    assert_eq!(said(&state), [(3, "why three?".to_owned())]);

    state.forget_tabs(|tab| tab == second);
    assert!(
        state.comments().is_empty(),
        "a closed tab's comments were kept"
    );
}

#[test]
fn the_message_names_each_line_by_path_and_number_and_quotes_it() {
    let mut state = showing(overview(&["src/a.rs", "src/b.rs"]));
    for (index, path) in ["src/a.rs", "src/b.rs"].into_iter().enumerate() {
        let read = state.toggle(index).expect("asked");
        state.land_hunks(Path::new(path), read.ticket, Ok(a_hunk()));
    }
    // Made out of order: the message is in the order of the files and their
    // lines, which is the order a person reads the diff in.
    comment(&mut state, 1, 0, "  this whole hunk goes  ");
    comment(&mut state, 0, 4, "and c?");
    comment(&mut state, 0, 2, "put b back");

    assert_eq!(
        state.review(Some("feat/review")).as_deref(),
        Some(
            "Review of feat/review since main:\n\
             \n\
             src/a.rs:11 (removed; numbered as before the change)\n\
             > -    let b = 2;\n\
             put b back\n\
             \n\
             src/a.rs:12\n\
             > +    let c = 4;\n\
             and c?\n\
             \n\
             src/b.rs:10\n\
             > @@ -10,3 +10,4 @@ fn main() {\n\
             this whole hunk goes"
        )
    );
}

#[test]
fn there_is_no_review_without_a_comment() {
    let state = showing_diff("a.rs", a_hunk());
    assert_eq!(state.review(Some("feat/review")), None);
}

#[test]
fn a_review_that_went_is_cleared_and_one_that_did_not_is_kept() {
    let mut state = showing_diff("a.rs", a_hunk());
    comment(&mut state, 0, 3, "why three?");

    state.say("Nothing is running there.".to_owned());
    assert_eq!(
        state.comments().len(),
        1,
        "a refused send took the comments"
    );

    state.sent();
    assert!(state.comments().is_empty());
}
