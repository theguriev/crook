//! Executable specification for what a task changed.
//!
//! Every case builds a real repository and changes it the way an agent does —
//! commits on a branch, edits it has not committed, a file it made and never
//! added — because the whole question is what git's own `diff` and `log` say
//! about a tree, and a fixture written by hand would answer it for a git that
//! does not exist. A machine with no git skips them with a message rather
//! than failing.
//!
//! The scratch harness is a copy of `merged_tests.rs`'s, for the reason that
//! file gives: thirty lines duplicated beat a helper that has to be understood
//! from another module.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::process::command;

// --- scratch directories -----------------------------------------------------

/// A directory under the system temp directory, removed when it drops.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!(
            "crook-changes-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");
        Self {
            path: crate::git::as_git_prints_it(path),
        }
    }

    /// An empty directory inside this one.
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::create_dir_all(&path).expect("the scratch directory is writable");
        path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// --- running git ---------------------------------------------------------------

/// Whether to skip a test that needs git, saying so.
fn without_git(test: &str) -> bool {
    let installed = command("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok();
    if !installed {
        eprintln!("skipping {test}: git is not installed on this machine");
    }
    !installed
}

/// Runs git in `dir`, isolated from the machine's own configuration, and
/// returns its trimmed stdout. See `worktree_tests.rs` for why the isolation.
fn git(dir: &Path, args: &[&str]) -> String {
    let nowhere = dir.join("no-such-gitconfig");
    let output = command("git")
        .args(["-c", "init.defaultBranch=main"])
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &nowhere)
        .env("GIT_CONFIG_SYSTEM", &nowhere)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Crook Tests")
        .env("GIT_AUTHOR_EMAIL", "tests@crook.invalid")
        .env("GIT_COMMITTER_NAME", "Crook Tests")
        .env("GIT_COMMITTER_EMAIL", "tests@crook.invalid")
        .stdin(Stdio::null())
        .output()
        .expect("git is installed; the caller checked");

    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository at `<scratch>/<name>` on `main`, with one commit.
fn repository(scratch: &ScratchDir, name: &str) -> PathBuf {
    let repo = scratch.dir(name);
    git(&repo, &["init"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\n", "initial");
    repo
}

/// Writes `file` in `repo` and commits it on whatever is checked out.
fn commit(repo: &Path, file: &str, contents: &str, message: &str) {
    write(repo, file, contents);
    git(repo, &["add", file]);
    git(repo, &["commit", "--no-verify", "-m", message]);
}

/// Writes `file` in `repo` and leaves it where it is.
fn write(repo: &Path, file: &str, contents: impl AsRef<[u8]>) {
    std::fs::write(repo.join(file), contents).expect("the scratch directory is writable");
}

/// Dates `file` in `repo` an hour back, so the stat data the index holds for
/// it — taken whenever this test happened to run — cannot match.
fn backdate(repo: &Path, file: &str) {
    let an_hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(repo.join(file))
        .and_then(|opened| opened.set_modified(an_hour_ago))
        .expect("the scratch file can be dated");
}

/// Writes `patch` to a file beside `repo` and asks git whether it would take
/// it back out of the working tree — which it does only if the patch is one
/// `git apply` reads as the change that was made.
fn applies_in_reverse(repo: &Path, patch: &str) -> bool {
    let file = repo.with_extension("patch");
    std::fs::write(&file, patch).expect("the scratch directory is writable");
    command("git")
        .args(["apply", "--check", "-R"])
        .arg(&file)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", repo.join("no-such-gitconfig"))
        .env("GIT_CONFIG_SYSTEM", repo.join("no-such-gitconfig"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A repository on a branch `task` cut from `main`, which is what an agent is
/// handed.
fn task(scratch: &ScratchDir) -> PathBuf {
    let repo = repository(scratch, "repo");
    git(&repo, &["switch", "-c", "task"]);
    repo
}

/// The file `path` in `changes`, or a failure naming what was there instead.
fn file<'a>(changes: &'a [FileChange], path: &str) -> &'a FileChange {
    changes
        .iter()
        .find(|change| change.path == Path::new(path))
        .unwrap_or_else(|| {
            let listed: Vec<_> = changes
                .iter()
                .map(|change| (change.status, change.path.display().to_string()))
                .collect();
            panic!("{path} is not among the changes: {listed:?}")
        })
}

// --- the base ------------------------------------------------------------------

#[test]
fn a_task_branch_is_compared_with_where_it_left_main() {
    if without_git("a_task_branch_is_compared_with_where_it_left_main") {
        return;
    }
    let scratch = ScratchDir::new("base");
    let repo = task(&scratch);
    let fork = git(&repo, &["rev-parse", "HEAD"]);
    commit(&repo, "work.txt", "work\n", "work");
    // main moving on after the fork is not the task's change, and comparing
    // with main's tip rather than with the fork would show it as undone.
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "later\n", "later on main");
    git(&repo, &["switch", "task"]);

    let base = base(&repo).expect("the repository can be read");

    assert_eq!(base.name, "main");
    assert_eq!(base.fork, fork);
    assert_eq!(base.against, Against::Branch);
}

// --- the commits -----------------------------------------------------------------

#[test]
fn the_commits_since_the_base_are_listed_newest_first() {
    if without_git("the_commits_since_the_base_are_listed_newest_first") {
        return;
    }
    let scratch = ScratchDir::new("commits");
    let repo = task(&scratch);
    commit(&repo, "a.txt", "a\n", "first on the task");
    commit(&repo, "b.txt", "b\n", "second on the task");
    commit(&repo, "c.txt", "c\n", "third on the task");
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "later\n", "main moves on");
    git(&repo, &["switch", "task"]);

    let base = base(&repo).expect("the repository can be read");
    let listed = commits(&repo, &base).expect("the log can be read");

    let subjects: Vec<&str> = listed
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(
        subjects,
        [
            "third on the task",
            "second on the task",
            "first on the task"
        ],
        "not the task's own commits, newest first — main's, or the initial one, crept in"
    );
    assert!(!listed.more);
    let newest = &listed.commits[0];
    assert!(
        git(&repo, &["rev-parse", "HEAD"]).starts_with(&newest.sha),
        "{} is not the tip's short sha",
        newest.sha
    );
    assert!(!newest.when.is_empty(), "the commit carries no date");
}

// --- the files -------------------------------------------------------------------

#[test]
fn every_kind_of_change_is_in_the_file_list() {
    // Committed, staged and neither: the column is about what the task did,
    // and an agent's work is in all three places at once.
    if without_git("every_kind_of_change_is_in_the_file_list") {
        return;
    }
    let scratch = ScratchDir::new("files");
    let repo = task(&scratch);
    commit(&repo, "doomed.txt", "going\n", "a file main never had");
    commit(
        &repo,
        "moved.txt",
        "one\ntwo\nthree\nfour\n",
        "a file to move",
    );
    git(&repo, &["switch", "main"]);
    commit(&repo, "doomed.txt", "going\n", "the same file, on main");
    commit(
        &repo,
        "moved.txt",
        "one\ntwo\nthree\nfour\n",
        "the same file, on main",
    );
    git(&repo, &["switch", "task"]);
    git(&repo, &["rebase", "main"]);

    write(&repo, "tracked.txt", "one\nTWO\nthree\n");
    commit(&repo, "added.txt", "new\n", "a new file");
    git(&repo, &["rm", "--quiet", "doomed.txt"]);
    git(&repo, &["mv", "moved.txt", "renamed.txt"]);
    write(&repo, "untracked.txt", "nobody added me\n");

    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");
    let changes = &listed.files;

    assert_eq!(file(changes, "tracked.txt").status, Status::Modified);
    assert_eq!(file(changes, "added.txt").status, Status::Added);
    assert_eq!(file(changes, "doomed.txt").status, Status::Deleted);
    let renamed = file(changes, "renamed.txt");
    assert_eq!(renamed.status, Status::Renamed);
    assert_eq!(renamed.from.as_deref(), Some(Path::new("moved.txt")));
    assert_eq!(file(changes, "untracked.txt").status, Status::Untracked);
    assert_eq!(
        changes.len(),
        5,
        "more than the five changes were listed: {changes:?}"
    );
    assert!(!listed.more);
}

#[test]
fn a_file_rewritten_with_the_bytes_it_had_is_not_a_change() {
    // A formatter that changed nothing, `touch`, a generator writing the
    // same output: the contents are what they were and only the stat data
    // moved. Crook's reads never refresh the index, so git's own list says
    // "modified" for as long as nothing else refreshes it.
    if without_git("a_file_rewritten_with_the_bytes_it_had_is_not_a_change") {
        return;
    }
    let scratch = ScratchDir::new("same-bytes");
    // On main, so that at the base they are what they are now.
    let repo = repository(&scratch, "repo");
    commit(&repo, "kept.txt", "the same\n", "a file the agent rewrites");
    commit(
        &repo,
        "run.sh",
        "echo hi\n",
        "a file that becomes a program",
    );
    git(&repo, &["switch", "-c", "task"]);
    write(&repo, "kept.txt", "the same\n");
    backdate(&repo, "kept.txt");
    write(&repo, "tracked.txt", "one\nTWO\nthree\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(repo.join("run.sh"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }
    // That this is the case being tested, and not one git already hides: the
    // raw list, read the way `files` reads it, does name the file.
    let raw = git(
        &repo,
        &[
            "--no-optional-locks",
            "-c",
            "diff.autoRefreshIndex=false",
            "diff",
            "--raw",
            "HEAD",
        ],
    );
    assert!(raw.contains("kept.txt"), "git no longer lists it: {raw}");

    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    assert!(
        !listed
            .files
            .iter()
            .any(|file| file.path == Path::new("kept.txt")),
        "a file holding what it held was listed as changed: {:?}",
        listed.files
    );
    assert_eq!(file(&listed.files, "tracked.txt").status, Status::Modified);
    #[cfg(unix)]
    assert_eq!(
        file(&listed.files, "run.sh").status,
        Status::Modified,
        "a mode that changed is a change"
    );
}

#[test]
fn a_repository_inside_the_repository_is_listed_as_one() {
    // An agent that clones something into its worktree, or a checkout under
    // `.claude/worktrees/` in a repository that does not ignore it: git lists
    // the directory and never looks inside, and there is no diff to read.
    if without_git("a_repository_inside_the_repository_is_listed_as_one") {
        return;
    }
    let scratch = ScratchDir::new("nested-repository");
    let repo = task(&scratch);
    let inner = repo.join("vendored");
    std::fs::create_dir_all(&inner).expect("writable");
    git(&inner, &["init"]);
    write(&inner, "inside.txt", "not this repository's\n");

    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let nested = file(&listed.files, "vendored");
    assert_eq!(nested.status, Status::Repository);
    assert!(
        !listed
            .files
            .iter()
            .any(|file| file.path.starts_with("vendored/inside.txt")),
        "a file inside the nested repository was listed: {:?}",
        listed.files
    );
    assert!(
        hunks(&repo, &base, nested).is_err(),
        "a nested repository was read as a diff with nothing in it"
    );
}

#[test]
fn a_file_in_a_subdirectory_is_listed_from_the_top_of_the_repository() {
    // `ls-files` answers relative to where it runs, and a pane is very often
    // somewhere below the top: a path that meant something only from there
    // would open nothing and diff nothing.
    if without_git("a_file_in_a_subdirectory_is_listed_from_the_top_of_the_repository") {
        return;
    }
    let scratch = ScratchDir::new("nested");
    let repo = task(&scratch);
    std::fs::create_dir_all(repo.join("src/deep")).expect("writable");
    write(&repo, "src/deep/new.rs", "fn main() {}\n");

    let found = overview(&repo.join("src/deep")).expect("the repository can be read");

    assert_eq!(found.repository, repo);
    assert_eq!(
        file(&found.files, "src/deep/new.rs").status,
        Status::Untracked
    );
}

// --- the hunks -------------------------------------------------------------------

#[test]
fn a_modified_file_s_hunks_are_its_changed_lines() {
    if without_git("a_modified_file_s_hunks_are_its_changed_lines") {
        return;
    }
    let scratch = ScratchDir::new("hunks");
    let repo = task(&scratch);
    write(&repo, "tracked.txt", "one\nTWO\nthree\n");
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "tracked.txt")).expect("the diff reads");

    assert!(!diff.binary);
    assert!(!diff.cut);
    assert!(
        diff.lines
            .first()
            .is_some_and(|line| line.starts_with("@@")),
        "the lines do not start at the first hunk: {:?}",
        diff.lines
    );
    assert!(diff.lines.iter().any(|line| line == "-two"), "{diff:?}");
    assert!(diff.lines.iter().any(|line| line == "+TWO"), "{diff:?}");
    assert_eq!(diff.first_line, Some(2), "the first change is on line 2");
    assert!(
        diff.patch.starts_with("diff --git"),
        "the patch is not one `git apply` would take: {}",
        diff.patch
    );
}

#[test]
fn an_untracked_file_s_hunks_are_every_line_added() {
    if without_git("an_untracked_file_s_hunks_are_every_line_added") {
        return;
    }
    let scratch = ScratchDir::new("untracked");
    let repo = task(&scratch);
    write(&repo, "fresh.txt", "alpha\nbeta\n");
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "fresh.txt")).expect("the diff reads");

    let changed: Vec<&str> = diff
        .lines
        .iter()
        .filter(|line| !line.starts_with("@@"))
        .map(String::as_str)
        .collect();
    assert_eq!(changed, ["+alpha", "+beta"]);
    assert_eq!(diff.first_line, Some(1));
}

#[test]
fn an_untracked_file_gone_before_it_was_read_is_a_failure_and_not_an_empty_diff() {
    // `diff --no-index` exits 1 for "there were differences" and for "could
    // not read that": the list was read, the agent deleted its scratch file,
    // then somebody pressed it.
    if without_git("an_untracked_file_gone_before_it_was_read_is_a_failure_and_not_an_empty_diff") {
        return;
    }
    let scratch = ScratchDir::new("gone");
    let repo = task(&scratch);
    write(&repo, "scratch.txt", "for a moment\n");
    write(&repo, "empty.txt", "");
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");
    std::fs::remove_file(repo.join("scratch.txt")).expect("removable");

    let gone = hunks(&repo, &base, file(&listed.files, "scratch.txt"));
    assert!(
        matches!(&gone, Err(Error::Failed(message)) if message.contains("scratch.txt")),
        "a file that could not be read came back as {gone:?}"
    );

    // And the one answer that has no lines and is not a failure: a new file
    // with nothing in it, which git still prints a header for.
    let empty = hunks(&repo, &base, file(&listed.files, "empty.txt")).expect("an empty file reads");
    assert!(empty.lines.is_empty(), "{empty:?}");
    assert!(!empty.patch.is_empty(), "{empty:?}");
}

#[cfg(unix)]
#[test]
fn a_file_that_became_a_link_draws_only_the_lines_of_its_two_hunks() {
    // git prints a type change as a deletion and then a creation, each with
    // headers of its own; the second one's `---` and `+++` are not a line
    // anybody removed or added.
    if without_git("a_file_that_became_a_link_draws_only_the_lines_of_its_two_hunks") {
        return;
    }
    let scratch = ScratchDir::new("type-change");
    let repo = task(&scratch);
    // `tracked.txt` is on main, so at the base it is a file.
    std::fs::remove_file(repo.join("tracked.txt")).expect("removable");
    std::os::unix::fs::symlink("elsewhere.txt", repo.join("tracked.txt")).expect("a link");
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");
    let changed = file(&listed.files, "tracked.txt");
    assert_eq!(changed.status, Status::TypeChanged);

    let diff = hunks(&repo, &base, changed).expect("the diff reads");

    let headers: Vec<&String> = diff
        .lines
        .iter()
        .filter(|line| {
            line.starts_with("---")
                || line.starts_with("+++")
                || line.starts_with("diff ")
                || line.starts_with("new file")
                || line.starts_with("index ")
        })
        .collect();
    assert!(
        headers.is_empty(),
        "a patch's headers were drawn as its lines: {headers:?}"
    );
    assert!(diff.lines.iter().any(|line| line == "-two"), "{diff:?}");
    assert!(
        diff.lines.iter().any(|line| line == "+elsewhere.txt"),
        "{diff:?}"
    );
}

#[test]
fn a_line_too_long_to_draw_is_cut_for_the_column_and_copied_whole() {
    // One line of a minified bundle is shaped whole on every frame it is on
    // screen, for a column that shows its first fifty characters.
    if without_git("a_line_too_long_to_draw_is_cut_for_the_column_and_copied_whole") {
        return;
    }
    let scratch = ScratchDir::new("long-line");
    let repo = task(&scratch);
    // Multi-byte, so the cut has to find where a character ends.
    let long = "é".repeat(25_000);
    write(&repo, "bundle.min.js", format!("{long}\n"));
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "bundle.min.js")).expect("the diff reads");

    let added = diff
        .lines
        .iter()
        .find(|line| line.starts_with('+'))
        .expect("the line is there");
    assert!(
        added.len() <= MAX_LINE_BYTES + '\u{2026}'.len_utf8(),
        "a {}-byte line was kept to draw",
        added.len()
    );
    assert!(added.ends_with('\u{2026}'), "the cut does not say so");
    assert!(!diff.cut, "the diff itself was not cut");
    assert!(
        diff.patch.contains(&format!("+{long}\n")),
        "the patch Copy diff copies lost the line's end"
    );
}

#[test]
fn a_repository_s_diff_settings_change_neither_the_patch_nor_the_line_to_open() {
    // Settings somebody may well have in their global config: without the
    // prefixes pinned, `diff.noprefix` makes a patch `git apply` reads with
    // the directory stripped off; `diff.suppressBlankEmpty` prints a blank
    // context line as nothing, which the count to the first change skipped.
    if without_git("a_repository_s_diff_settings_change_neither_the_patch_nor_the_line_to_open") {
        return;
    }
    let scratch = ScratchDir::new("settings");
    let repo = repository(&scratch, "repo");
    std::fs::create_dir_all(repo.join("src")).expect("writable");
    commit(&repo, "src/blank.txt", "a\n\n\n\nb\nc\n", "blank lines");
    git(&repo, &["switch", "-c", "task"]);
    git(&repo, &["config", "diff.noprefix", "true"]);
    git(&repo, &["config", "diff.suppressBlankEmpty", "true"]);
    write(&repo, "src/blank.txt", "a\n\n\n\nB\nc\n");
    write(&repo, "src/fresh.txt", "new\n");
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "src/blank.txt")).expect("the diff reads");
    assert_eq!(diff.first_line, Some(5), "{:?}", diff.lines);
    assert!(
        diff.patch
            .contains("--- a/src/blank.txt\n+++ b/src/blank.txt\n"),
        "{}",
        diff.patch
    );
    assert!(
        applies_in_reverse(&repo, &diff.patch),
        "git apply refuses the patch:\n{}",
        diff.patch
    );

    let fresh = hunks(&repo, &base, file(&listed.files, "src/fresh.txt")).expect("the diff reads");
    assert!(
        fresh.patch.contains("+++ b/src/fresh.txt\n"),
        "{}",
        fresh.patch
    );
    assert!(
        applies_in_reverse(&repo, &fresh.patch),
        "git apply refuses the patch:\n{}",
        fresh.patch
    );
}

#[test]
fn a_binary_file_is_marked_binary_and_has_no_lines() {
    if without_git("a_binary_file_is_marked_binary_and_has_no_lines") {
        return;
    }
    let scratch = ScratchDir::new("binary");
    let repo = task(&scratch);
    let mut picture = b"\x89PNG\r\n\x1a\n".to_vec();
    picture.extend([0u8; 64]);
    commit(&repo, "picture.png", "", "an empty picture");
    write(&repo, "picture.png", &picture);
    write(&repo, "new.bin", [0u8, 1, 2, 0, 3]);
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    for path in ["picture.png", "new.bin"] {
        let diff = hunks(&repo, &base, file(&listed.files, path)).expect("the diff reads");
        assert!(diff.binary, "{path} was not marked binary: {diff:?}");
        assert!(diff.lines.is_empty(), "{path} has lines: {:?}", diff.lines);
    }
}

#[test]
fn a_diff_over_the_byte_cap_is_cut_and_says_so() {
    if without_git("a_diff_over_the_byte_cap_is_cut_and_says_so") {
        return;
    }
    let scratch = ScratchDir::new("cut");
    let repo = task(&scratch);
    // Past the real cap rather than a test-sized one, so the constant the
    // column runs with is the one being held to it.
    // Long lines, so that the bytes run out well before the line cap does and
    // it is the byte cap being held to account — but short enough to be kept
    // whole for drawing, so the last one says whether the cut fell mid-line.
    let line = format!("{}\n", "generated ".repeat(20));
    let lines = MAX_DIFF_BYTES / line.len() + 100;
    assert!(lines < MAX_DIFF_LINES, "the line cap would cut this first");
    assert!(line.len() < MAX_LINE_BYTES, "a line is cut for drawing");
    write(&repo, "generated.lock", line.repeat(lines));
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "generated.lock")).expect("the diff reads");

    assert!(diff.cut, "a diff past the cap was not marked cut");
    assert!(
        diff.patch.len() <= MAX_DIFF_BYTES,
        "{} bytes were kept, past the cap of {MAX_DIFF_BYTES}",
        diff.patch.len()
    );
    assert!(
        diff.lines.len() < lines,
        "all {lines} lines were kept, so nothing was cut"
    );
    assert!(
        diff.lines
            .last()
            .is_some_and(|last| last == &format!("+{}", line.trim_end_matches('\n'))),
        "the last line kept is a fragment: {:?}",
        diff.lines.last()
    );
}

#[test]
fn a_diff_over_the_line_cap_is_cut_too() {
    if without_git("a_diff_over_the_line_cap_is_cut_too") {
        return;
    }
    let scratch = ScratchDir::new("lines");
    let repo = task(&scratch);
    let text: String = (0..MAX_DIFF_LINES + 50).map(|n| format!("{n}\n")).collect();
    write(&repo, "numbers.txt", text);
    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");

    let diff = hunks(&repo, &base, file(&listed.files, "numbers.txt")).expect("the diff reads");

    assert!(diff.cut, "a diff past the line cap was not marked cut");
    assert_eq!(diff.lines.len(), MAX_DIFF_LINES);
}

#[cfg(unix)]
#[test]
fn a_repository_s_own_diff_programs_are_never_run() {
    // `diff.external` and a `textconv` driver are a program the repository's
    // configuration names, and git runs it for every file it diffs. A column
    // that only reads must not start one: a repository an agent is working in
    // is not a repository anybody vetted.
    if without_git("a_repository_s_own_diff_programs_are_never_run") {
        return;
    }
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = ScratchDir::new("external");
    let repo = task(&scratch);
    let marker = scratch.path.join("ran");
    let program = scratch.path.join("program.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\ntouch '{}'\ncat \"$1\" 2>/dev/null\n",
            marker.display()
        ),
    )
    .expect("writable");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let program = program.display().to_string();
    git(&repo, &["config", "diff.external", &program]);
    git(&repo, &["config", "diff.shout.textconv", &program]);
    write(&repo, ".gitattributes", "*.txt diff=shout\n");
    write(&repo, "tracked.txt", "one\nTWO\nthree\n");
    write(&repo, "fresh.txt", "fresh\n");

    let base = base(&repo).expect("the repository can be read");
    let listed = files(&repo, &base).expect("the diff can be read");
    for path in ["tracked.txt", "fresh.txt"] {
        let diff = hunks(&repo, &base, file(&listed.files, path)).expect("the diff reads");
        assert!(
            diff.lines.iter().any(|line| line.starts_with('+')),
            "{path}'s diff is not git's own: {diff:?}"
        );
    }

    assert!(
        !marker.exists(),
        "a program the repository's configuration names was run"
    );
}

#[test]
fn a_directory_outside_any_repository_is_said_to_be_one() {
    let scratch = ScratchDir::new("nowhere");
    let outside = scratch.dir("plain");

    assert!(matches!(overview(&outside), Err(Error::NotARepository)));
}
