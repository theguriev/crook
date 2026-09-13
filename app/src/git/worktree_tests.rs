//! Executable specification for the worktree commands.
//!
//! The parsing cases are pure and run anywhere. The rest build real
//! repositories in the system temp directory and ask git to add and remove real
//! checkouts — because every bug this module can have is a bug about what git
//! prints, and a hand-written fixture would get it subtly and confidently
//! wrong. A machine with no git skips those with a message rather than failing.
//!
//! The scratch harness is deliberately a copy of the one in the git module's
//! own `tests.rs` rather than a shared one: it is thirty lines, the files are
//! separately, and a test helper that has to be understood from another module
//! is a worse trade than a little duplication.

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
            "crook-worktree-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");

        // Resolved now, once, on macOS alone: it reaches the temp directory
        // through /var -> /private/var, and git prints the resolved form, so
        // comparing the two spellings would fail every path assertion in this
        // file. Nowhere else is the temporary directory a link, and on Windows
        // the canonical form is the `\\?\` spelling, which git cannot be
        // handed — `worktree add` refuses to create leading directories under
        // it — and which nothing else here writes.
        let path = if cfg!(target_os = "macos") {
            std::fs::canonicalize(&path).expect("the directory was just created")
        } else {
            path
        };
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// An empty directory inside this one.
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::create_dir_all(&path).expect("the scratch directory is writable");
        path
    }

    /// A path inside this one that nothing has created yet.
    fn spot(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the scratch directory is writable");
    }
    std::fs::write(path, contents).expect("the scratch directory is writable");
}

// --- running git ---------------------------------------------------------------

fn git_is_installed() -> bool {
    command("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok()
}

/// Whether to skip a test that needs git, saying so.
fn without_git(test: &str) -> bool {
    if git_is_installed() {
        return false;
    }
    eprintln!("skipping {test}: git is not installed on this machine");
    true
}

/// Runs git in `dir`, isolated from whatever the machine's own git
/// configuration says, and returns its trimmed stdout.
///
/// The isolation matters: a developer with `commit.gpgsign`, a global
/// `core.hooksPath` or `core.autocrlf` set would otherwise see these tests fail
/// for reasons that have nothing to do with the code.
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
fn repo_with_a_commit(scratch: &ScratchDir, name: &str) -> PathBuf {
    let repo = scratch.dir(name);
    git(&repo, &["init"]);
    write(&repo.join("tracked.txt"), "one\ntwo\nthree\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--no-verify", "-m", "initial"]);
    repo
}

/// The nearest `.git` at or above `path`, so a test that asserts "not a
/// repository" can tell the difference between a bug and a temp directory that
/// happens to live inside someone's checkout.
fn repository_above(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// The worktree at `path`, which the test expects to be listed.
fn entry<'a>(worktrees: &'a [Worktree], path: &Path) -> &'a Worktree {
    worktrees
        .iter()
        .find(|worktree| worktree.path == path)
        .unwrap_or_else(|| {
            panic!(
                "{} is not among {:?}",
                path.display(),
                worktrees.iter().map(|w| &w.path).collect::<Vec<_>>()
            )
        })
}

// --- reading a listing ---------------------------------------------------------

#[test]
fn every_attribute_of_a_worktree_record_is_read() {
    let stdout: &[u8] = b"worktree /home/eugen/Work/crook\0\
        HEAD 439ec816dcb16fb024ba36800de6e61969c8a0bb\0\
        branch refs/heads/main\0\0\
        worktree /home/eugen/Work/crook/.claude/worktrees/terminal-features\0\
        HEAD 439ec816dcb16fb024ba36800de6e61969c8a0bb\0\
        detached\0\
        locked claude session terminal-features (pid 1415238 start 20712995)\0\
        prunable gitdir file points to non-existent location\0\0";

    let worktrees = parse_list(stdout);

    assert_eq!(worktrees.len(), 2);
    assert_eq!(
        worktrees[0],
        Worktree {
            path: PathBuf::from("/home/eugen/Work/crook"),
            head: Some("439ec81".to_owned()),
            branch: Some("main".to_owned()),
            is_main: true,
            is_bare: false,
            locked: None,
            prunable: None,
        }
    );
    // A detached checkout is one with no branch attribute, which is what
    // `branch: None` says without needing a flag of its own.
    assert_eq!(worktrees[1].branch, None);
    assert!(!worktrees[1].is_main);
    assert_eq!(
        worktrees[1].locked.as_deref(),
        Some("claude session terminal-features (pid 1415238 start 20712995)")
    );
    assert_eq!(
        worktrees[1].prunable.as_deref(),
        Some("gitdir file points to non-existent location")
    );
}

#[test]
fn a_lock_with_no_reason_is_still_a_lock() {
    let stdout: &[u8] = b"worktree /w\0HEAD 1234567890abcdef\0branch refs/heads/x\0locked\0\0";

    let worktrees = parse_list(stdout);

    // `Some("")` and `None` are the whole distinction between a worktree locked
    // for a reason nobody wrote down and one that is not locked at all.
    assert_eq!(worktrees[0].locked.as_deref(), Some(""));
}

#[test]
fn an_attribute_from_a_newer_git_is_skipped_rather_than_refused() {
    let stdout: &[u8] =
        b"worktree /w\0HEAD 1234567890abcdef\0branch refs/heads/x\0invented-in-2031 yes\0\0";

    let worktrees = parse_list(stdout);

    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].branch.as_deref(), Some("x"));
}

#[test]
fn a_fresh_repository_lists_exactly_one_checkout_and_it_is_the_main_one() {
    if without_git("a_fresh_repository_lists_exactly_one_checkout_and_it_is_the_main_one") {
        return;
    }
    let scratch = ScratchDir::new("list-one");
    let repo = repo_with_a_commit(&scratch, "repo");

    let worktrees = list(&repo).expect("git answered");

    assert_eq!(worktrees.len(), 1);
    let main = &worktrees[0];
    assert_eq!(main.path, repo);
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert_eq!(
        main.head.as_deref(),
        Some(&*git(&repo, &["rev-parse", "--short=7", "HEAD"]))
    );
    assert!(main.is_main);
    assert!(!main.is_bare);
    assert_eq!(main.locked, None);
    assert_eq!(main.prunable, None);
}

#[test]
fn an_added_worktree_is_listed_after_the_main_one_with_its_own_branch() {
    if without_git("an_added_worktree_is_listed_after_the_main_one_with_its_own_branch") {
        return;
    }
    let scratch = ScratchDir::new("list-two");
    let repo = repo_with_a_commit(&scratch, "repo");
    let side = scratch.spot("side");
    git(&repo, &["worktree", "add", "-b", "side", "../side", "main"]);

    let worktrees = list(&repo).expect("git answered");

    assert_eq!(worktrees.len(), 2);
    assert!(worktrees[0].is_main);
    let added = entry(&worktrees, &side);
    assert_eq!(added.branch.as_deref(), Some("side"));
    assert!(!added.is_main);
    // Asking from inside the new checkout finds the same repository, and the
    // main worktree is still the one git prints first.
    assert_eq!(list(&side).expect("git answered"), worktrees);
}

#[test]
fn a_detached_worktree_has_a_commit_and_no_branch() {
    if without_git("a_detached_worktree_has_a_commit_and_no_branch") {
        return;
    }
    let scratch = ScratchDir::new("list-detached");
    let repo = repo_with_a_commit(&scratch, "repo");
    let loose = scratch.spot("loose");
    git(&repo, &["worktree", "add", "--detach", "../loose", "main"]);

    let worktrees = list(&repo).expect("git answered");
    let detached = entry(&worktrees, &loose);

    assert_eq!(detached.branch, None);
    assert_eq!(
        detached.head.as_deref(),
        Some(&*git(&repo, &["rev-parse", "--short=7", "HEAD"]))
    );
}

#[test]
fn a_locked_worktree_carries_the_reason_it_was_locked_with() {
    if without_git("a_locked_worktree_carries_the_reason_it_was_locked_with") {
        return;
    }
    let scratch = ScratchDir::new("list-locked");
    let repo = repo_with_a_commit(&scratch, "repo");
    let held = scratch.spot("held");
    git(&repo, &["worktree", "add", "-b", "held", "../held", "main"]);
    git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "claude session held",
            "../held",
        ],
    );

    let worktrees = list(&repo).expect("git answered");

    assert_eq!(
        entry(&worktrees, &held).locked.as_deref(),
        Some("claude session held")
    );
    assert_eq!(worktrees[0].locked, None);
}

#[test]
fn a_bare_repository_lists_one_entry_with_no_commit_on_it() {
    if without_git("a_bare_repository_lists_one_entry_with_no_commit_on_it") {
        return;
    }
    let scratch = ScratchDir::new("list-bare");
    let mirror = scratch.dir("mirror.git");
    git(&mirror, &["init", "--bare"]);

    let worktrees = list(&mirror).expect("git answered");

    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].is_bare);
    assert!(worktrees[0].is_main);
    // The one entry git prints without a HEAD line at all.
    assert_eq!(worktrees[0].head, None);
    assert_eq!(worktrees[0].branch, None);
}

#[cfg(unix)]
#[test]
fn a_path_with_a_newline_in_it_comes_back_whole() {
    if without_git("a_path_with_a_newline_in_it_comes_back_whole") {
        return;
    }
    let scratch = ScratchDir::new("list-newline");
    let repo = repo_with_a_commit(&scratch, "repo");
    // The case that decides `-z`: git 2.55.0 prints this path raw in the
    // newline-separated porcelain, so a line-based parser reads `.../new` and
    // then a record called `line`.
    let awkward = scratch.spot("new\nline");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "awkward",
            awkward.to_str().expect("the scratch path is utf-8"),
            "main",
        ],
    );

    let worktrees = list(&repo).expect("git answered");

    assert_eq!(worktrees.len(), 2);
    assert_eq!(
        entry(&worktrees, &awkward).branch.as_deref(),
        Some("awkward")
    );
}

#[test]
fn a_directory_outside_a_repository_cannot_be_listed() {
    if without_git("a_directory_outside_a_repository_cannot_be_listed") {
        return;
    }
    let scratch = ScratchDir::new("list-no-repo");
    if let Some(repo) = repository_above(scratch.path()) {
        eprintln!(
            "skipping a_directory_outside_a_repository_cannot_be_listed: the system temp \
             directory is inside the repository at {}",
            repo.display()
        );
        return;
    }

    assert!(matches!(list(scratch.path()), Err(Error::NotARepository)));
    // A directory that is not there is not a repository either, and finding
    // that out must not cost a spawn that would look like a missing git.
    assert!(matches!(
        list(&scratch.spot("never-created")),
        Err(Error::NotARepository)
    ));
}

// --- adding --------------------------------------------------------------------

#[test]
fn an_added_worktree_is_on_its_own_branch_from_the_base_it_was_given() {
    if without_git("an_added_worktree_is_on_its_own_branch_from_the_base_it_was_given") {
        return;
    }
    let scratch = ScratchDir::new("add");
    let repo = repo_with_a_commit(&scratch, "repo");
    let checkout = scratch.spot("checkouts/feature");

    add(&repo, &checkout, "eugen/feature", Some("main")).expect("git added the worktree");

    let worktrees = list(&repo).expect("git answered");
    assert_eq!(worktrees.len(), 2);
    assert_eq!(
        entry(&worktrees, &checkout).branch.as_deref(),
        Some("eugen/feature")
    );
    // Leading directories that did not exist are git's to create.
    assert!(checkout.join("tracked.txt").is_file());
}

#[test]
fn adding_a_branch_that_is_already_checked_out_says_where_it_is_checked_out() {
    if without_git("adding_a_branch_that_is_already_checked_out_says_where_it_is_checked_out") {
        return;
    }
    let scratch = ScratchDir::new("add-taken");
    let repo = repo_with_a_commit(&scratch, "repo");

    let error = add(&repo, &scratch.spot("second"), "main", Some("main"))
        .expect_err("main is checked out in the repository itself");

    // The path is the point: it is what a UI offers to jump to. git itself
    // names only the branch when the name is taken, so this is the listing
    // `add` does to fill the other half in.
    match error {
        Error::AlreadyCheckedOut { branch, at } => {
            assert_eq!(branch, "main");
            assert_eq!(at, repo);
        }
        other => panic!("expected AlreadyCheckedOut, got {other:?}"),
    }
    assert!(!scratch.spot("second").exists());
}

#[test]
fn adding_onto_a_branch_nobody_has_checked_out_says_only_that_the_name_is_taken() {
    if without_git("adding_onto_a_branch_nobody_has_checked_out_says_only_that_the_name_is_taken") {
        return;
    }
    let scratch = ScratchDir::new("add-name-taken");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["branch", "spare", "main"]);

    let error = add(&repo, &scratch.spot("spare"), "spare", Some("main"))
        .expect_err("the branch already exists");

    // Nobody is standing on it, so there is nowhere to offer to jump to — the
    // way out is a different name, not a different window.
    match error {
        Error::BranchExists { branch } => assert_eq!(branch, "spare"),
        other => panic!("expected BranchExists, got {other:?}"),
    }
}

#[test]
fn adding_where_something_already_lives_names_the_path() {
    if without_git("adding_where_something_already_lives_names_the_path") {
        return;
    }
    let scratch = ScratchDir::new("add-occupied");
    let repo = repo_with_a_commit(&scratch, "repo");
    let occupied = scratch.spot("occupied");
    write(&occupied.join("something.txt"), "in the way\n");

    let error =
        add(&repo, &occupied, "occupied", Some("main")).expect_err("the directory is not empty");

    match error {
        Error::PathExists { path } => assert_eq!(path, occupied),
        other => panic!("expected PathExists, got {other:?}"),
    }
}

#[test]
fn adding_into_an_empty_directory_is_allowed() {
    if without_git("adding_into_an_empty_directory_is_allowed") {
        return;
    }
    let scratch = ScratchDir::new("add-empty");
    let repo = repo_with_a_commit(&scratch, "repo");
    // An empty directory is not "in the way" as far as git is concerned, which
    // matters because a file picker leaves them behind all the time.
    let empty = scratch.dir("empty");

    add(&repo, &empty, "into-empty", Some("main")).expect("an empty directory is acceptable");

    assert_eq!(
        entry(&list(&repo).expect("git answered"), &empty)
            .branch
            .as_deref(),
        Some("into-empty")
    );
}

// --- removing ------------------------------------------------------------------

#[test]
fn a_clean_worktree_is_removed_without_force_and_leaves_its_branch_behind() {
    if without_git("a_clean_worktree_is_removed_without_force_and_leaves_its_branch_behind") {
        return;
    }
    let scratch = ScratchDir::new("remove-clean");
    let repo = repo_with_a_commit(&scratch, "repo");
    let checkout = scratch.spot("clean");
    add(&repo, &checkout, "clean", Some("main")).expect("git added the worktree");

    remove(&repo, &checkout, false).expect("a clean worktree needs no force");

    assert_eq!(list(&repo).expect("git answered").len(), 1);
    assert!(!checkout.exists());
    // The branch is the work. Removing the directory it was done in does not
    // get to remove it.
    assert_eq!(git(&repo, &["branch", "--list", "clean"]).trim(), "clean");
}

#[test]
fn a_worktree_with_an_untracked_file_is_removed_only_with_force() {
    if without_git("a_worktree_with_an_untracked_file_is_removed_only_with_force") {
        return;
    }
    let scratch = ScratchDir::new("remove-dirty");
    let repo = repo_with_a_commit(&scratch, "repo");
    let checkout = scratch.spot("dirty");
    add(&repo, &checkout, "dirty", Some("main")).expect("git added the worktree");
    write(&checkout.join("scratch.txt"), "not committed\n");

    match remove(&repo, &checkout, false).expect_err("there is loose work in it") {
        Error::HoldsLocalWork { path } => assert_eq!(path, checkout),
        other => panic!("expected HoldsLocalWork, got {other:?}"),
    }
    assert!(checkout.exists());

    remove(&repo, &checkout, true).expect("force throws the loose work away");
    assert!(!checkout.exists());
}

#[test]
fn a_locked_worktree_survives_a_forced_removal() {
    if without_git("a_locked_worktree_survives_a_forced_removal") {
        return;
    }
    let scratch = ScratchDir::new("remove-locked");
    let repo = repo_with_a_commit(&scratch, "repo");
    let checkout = scratch.spot("held");
    add(&repo, &checkout, "held", Some("main")).expect("git added the worktree");
    git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "claude session held",
            checkout.to_str().expect("the scratch path is utf-8"),
        ],
    );

    // git wants `-f -f` for a lock and this module never gives it the second
    // one: forcing means "throw away my uncommitted work", never "take the
    // checkout another agent is working in".
    match remove(&repo, &checkout, true).expect_err("a lock is not overridden by force") {
        Error::Locked { reason } => assert_eq!(reason, "claude session held"),
        other => panic!("expected Locked, got {other:?}"),
    }
    assert!(checkout.exists());
}

#[test]
fn neither_the_main_worktree_nor_a_path_that_is_not_one_can_be_removed() {
    if without_git("neither_the_main_worktree_nor_a_path_that_is_not_one_can_be_removed") {
        return;
    }
    let scratch = ScratchDir::new("remove-refused");
    let repo = repo_with_a_commit(&scratch, "repo");

    assert!(matches!(
        remove(&repo, &repo, true),
        Err(Error::MainWorktree { .. })
    ));
    assert!(matches!(
        remove(&repo, &scratch.spot("never-existed"), false),
        Err(Error::NotAWorktree { .. })
    ));
    assert!(repo.join("tracked.txt").is_file());
}

// --- what is loose in a worktree -----------------------------------------------

#[test]
fn a_clean_worktree_holds_nothing_that_would_be_lost() {
    if without_git("a_clean_worktree_holds_nothing_that_would_be_lost") {
        return;
    }
    let scratch = ScratchDir::new("local-clean");
    let repo = repo_with_a_commit(&scratch, "repo");

    let local = local_work(&repo).expect("git answered");

    assert!(local.is_empty());
    assert!(!local.blocks_removal());
    assert_eq!(local, Local::default());
}

#[test]
fn modified_untracked_and_ignored_files_are_counted_in_separate_buckets() {
    if without_git("modified_untracked_and_ignored_files_are_counted_in_separate_buckets") {
        return;
    }
    let scratch = ScratchDir::new("local-buckets");
    let repo = repo_with_a_commit(&scratch, "repo");
    write(&repo.join(".gitignore"), "build/\n*.tmp\n");
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "--no-verify", "-m", "ignore"]);

    write(&repo.join("tracked.txt"), "one\ntwo\nthree\nfour\n");
    write(&repo.join("notes.txt"), "loose\n");
    write(&repo.join("scratch.tmp"), "throwaway\n");
    write(&repo.join("build/debug/binary"), "artefact\n");
    write(&repo.join("build/debug/other"), "artefact\n");

    let local = local_work(&repo).expect("git answered");

    assert!(!local.is_empty());
    // Modified and untracked are the two git actually refuses over.
    assert!(local.blocks_removal());
    assert_eq!(local.modified, 1);
    assert_eq!(local.untracked, 1);
    // Two: the loose `scratch.tmp`, and `build/` as a single collapsed entry
    // however many files are underneath it. Counting the tree would cost the
    // walk that collapsing exists to avoid, and the answer to "is there
    // anything here" is the same either way.
    assert_eq!(local.ignored, 2);
}

#[test]
fn an_ignored_directory_is_deleted_by_a_removal_git_never_objects_to() {
    if without_git("an_ignored_directory_is_deleted_by_a_removal_git_never_objects_to") {
        return;
    }
    let scratch = ScratchDir::new("local-ignored");
    let repo = repo_with_a_commit(&scratch, "repo");
    write(&repo.join(".gitignore"), "build/\n");
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["commit", "--no-verify", "-m", "ignore"]);
    let checkout = scratch.spot("built");
    add(&repo, &checkout, "built", Some("main")).expect("git added the worktree");
    write(&checkout.join("build/debug/binary"), "artefact\n");

    let local = local_work(&checkout).expect("git answered");

    assert_eq!(
        local,
        Local {
            modified: 0,
            untracked: 0,
            ignored: 1,
        }
    );
    // Not empty, and yet nothing git will stand in the way of: a plain removal
    // takes the whole build directory and says nothing about it. That gap
    // between "there is something here" and "git will stop you" is the entire
    // reason the ignored bucket is counted apart from the other two.
    assert!(!local.is_empty());
    assert!(!local.blocks_removal());
    remove(&repo, &checkout, false).expect("git does not refuse over ignored files");
    assert!(!checkout.exists());
}

#[test]
fn more_output_than_fits_in_a_pipe_is_still_read_to_the_end() {
    if without_git("more_output_than_fits_in_a_pipe_is_still_read_to_the_end") {
        return;
    }
    let scratch = ScratchDir::new("local-flood");
    let repo = repo_with_a_commit(&scratch, "repo");
    // A pipe buffer is 64KB on Linux and smaller elsewhere. Five thousand
    // entries is comfortably past it, which is the whole point: a `run` that
    // polled for the exit while leaving the output unread would block git on a
    // full pipe, wait out its deadline, and then report a timeout for a
    // repository that answered instantly.
    let flooded = 5_000;
    for index in 0..flooded {
        write(&repo.join(format!("untracked-{index:05}.txt")), "loose\n");
    }

    let local = local_work(&repo).expect("git answered");

    assert_eq!(local.untracked, flooded);
    assert!(local.blocks_removal());
}

#[test]
fn a_renamed_file_is_one_change_rather_than_two() {
    // Porcelain v1 with `-z` prints a rename's origin as a second field, and
    // counting it would report one moved file as two changed ones.
    let stdout: &[u8] = b"RM renamed.txt\0tracked.txt\0?? newdir/\0!! target/\0";

    assert_eq!(
        Local::parse_status(stdout),
        Local {
            modified: 1,
            untracked: 1,
            ignored: 1,
        }
    );
}

// --- the deadline --------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_process_that_outlives_its_deadline_is_killed_and_reaped() {
    // `sleep` stands in for the git this exists to survive: one that has taken
    // a lock nobody will release, or is running a `post-checkout` hook waiting
    // on something that will never arrive. Fifty milliseconds against thirty
    // seconds leaves no doubt about which of the two ended the call.
    let mut child = command("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sleep is on PATH");

    let started = std::time::Instant::now();
    let timeout = Duration::from_millis(50);
    let outcome = wait_for(&mut child, started + timeout, timeout);

    match outcome {
        Err(Error::TimedOut { after }) => assert_eq!(after, timeout),
        other => panic!("expected TimedOut, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    // Killed *and* waited for. A process that is never reaped stays a zombie,
    // and a process id somebody else reaps can be handed out again to a
    // stranger — which is what makes a later kill dangerous rather than
    // useless.
    let reaped = child
        .try_wait()
        .expect("the child was waited for inside wait_for");
    assert!(matches!(reaped, Some(status) if !status.success()));
}

#[cfg(unix)]
#[test]
fn a_process_that_finishes_in_time_is_not_killed() {
    let mut child = command("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("true is on PATH");

    let timeout = Duration::from_secs(30);
    let status =
        wait_for(&mut child, Instant::now() + timeout, timeout).expect("it exits immediately");

    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn a_hook_that_backgrounds_something_does_not_hold_add_past_its_deadline() {
    if without_git("a_hook_that_backgrounds_something_does_not_hold_add_past_its_deadline") {
        return;
    }
    let scratch = ScratchDir::new("hook-daemon");
    let repo = repo_with_a_commit(&scratch, "repo");

    // What a real `post-checkout` hook does when it starts `direnv reload &`, a
    // file watcher or a dev server: git exits immediately and hands something
    // that outlives it the write end of both pipes. Killing git would not help
    // — it is not git that is holding them — so a `run` that joined its readers
    // would return when the `sleep` did and not before.
    let marker = scratch.spot("the-hook-ran");
    let hook = repo.join(".git/hooks/post-checkout");
    write(
        &hook,
        &format!(
            "#!/bin/sh\ntouch '{}'\nsleep 5 &\nexit 0\n",
            marker.display()
        ),
    );
    executable(&hook);

    // Five seconds of `sleep` against a call that should come back in about
    // one leaves no doubt about which of the two ended it, and five is as long
    // as this test is willing to leave a stray process behind. That matters
    // more than the margin does: the `sleep` inherits every descriptor git had
    // — which is why the call would block on it — and the suite runs its tests
    // in parallel, so a background process that lives for twenty seconds is
    // twenty seconds of it holding descriptors while a *different* test drives
    // a real shell through a pty. That is not a hypothetical; at twenty
    // seconds this test reliably starved the shell test three tests away.
    //
    // The deadline is deliberately longer than the grace `run` gives a reader
    // once git is reaped, so what this proves is the abandoning and not the
    // deadline: git exits in milliseconds here, and the call is timed against
    // the reader that will never finish.
    let deadline = Duration::from_secs(2);
    WRITE_DEADLINE.with(|budget| budget.set(deadline));

    let path = scratch.spot("hooked");
    let started = Instant::now();
    let outcome = add(&repo, &path, "hooked", Some("main"));
    let took = started.elapsed();

    if !marker.is_file() {
        // A global `core.hooksPath` — plenty of people have one — sends git
        // somewhere else for its hooks, and then there is no hook and nothing
        // to prove. Saying so beats asserting something the run did not test.
        eprintln!(
            "skipping a_hook_that_backgrounds_something_does_not_hold_add_past_its_deadline: \
             git did not run the repository's own post-checkout hook"
        );
        return;
    }

    // Bounded by the grace rather than by the daemon. Before this was fixed the
    // call came back when the `sleep` did, seconds after git had gone, with a
    // worker of the background pool parked for every one of them — and for a
    // hook that starts something that never exits, for ever.
    assert!(
        took < Duration::from_secs(3),
        "add waited {took:?} on a process git left behind"
    );
    // And honest about what happened. git exited zero and the checkout is on
    // disk, so however little of its output arrived this is not a failure:
    // reporting one would tell a person the directory they are looking at was
    // never made.
    outcome.expect("git made the worktree");
    assert!(path.join("tracked.txt").is_file());
    assert_eq!(
        entry(&list(&repo).expect("git answered"), &path)
            .branch
            .as_deref(),
        Some("hooked")
    );
}

/// Makes `path` runnable, for the hook the test above installs.
#[cfg(unix)]
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("the scratch directory is writable");
}

// --- reading what git said when it refused ------------------------------------

#[test]
fn each_fatal_git_prints_for_a_worktree_is_recognised() {
    // Every one of these is a real message from git 2.55.0, kept verbatim.
    assert!(matches!(
        classify("fatal: not a git repository (or any of the parent directories): .git\n"),
        Error::NotARepository
    ));
    assert!(matches!(
        classify("fatal: not a git repository (or any parent up to mount point /)\n"),
        Error::NotARepository
    ));

    match classify("fatal: 'main' is already used by worktree at '/home/eugen/Work/crook'\n") {
        Error::AlreadyCheckedOut { branch, at } => {
            assert_eq!(branch, "main");
            assert_eq!(at, PathBuf::from("/home/eugen/Work/crook"));
        }
        other => panic!("expected AlreadyCheckedOut, got {other:?}"),
    }
    match classify("fatal: a branch named 'main' already exists\n") {
        Error::BranchExists { branch } => assert_eq!(branch, "main"),
        other => panic!("expected BranchExists, got {other:?}"),
    }
    match classify("fatal: '../existing' already exists\n") {
        Error::PathExists { path } => assert_eq!(path, PathBuf::from("../existing")),
        other => panic!("expected PathExists, got {other:?}"),
    }
    match classify(
        "fatal: '../gone' is a missing but already registered worktree;\n\
         use 'add -f' to override, or 'prune' or 'remove' to clear\n",
    ) {
        Error::MissingButRegistered { path } => assert_eq!(path, PathBuf::from("../gone")),
        other => panic!("expected MissingButRegistered, got {other:?}"),
    }
    match classify(
        "fatal: '../w2' contains modified or untracked files, use --force to delete it\n",
    ) {
        Error::HoldsLocalWork { path } => assert_eq!(path, PathBuf::from("../w2")),
        other => panic!("expected HoldsLocalWork, got {other:?}"),
    }
    match classify(
        "fatal: cannot remove a locked working tree, lock reason: claude session x\n\
         use 'remove -f -f' to override or unlock first\n",
    ) {
        Error::Locked { reason } => assert_eq!(reason, "claude session x"),
        other => panic!("expected Locked, got {other:?}"),
    }
    match classify(
        "fatal: cannot remove a locked working tree;\n\
         use 'remove -f -f' to override or unlock first\n",
    ) {
        Error::Locked { reason } => assert_eq!(reason, ""),
        other => panic!("expected Locked, got {other:?}"),
    }
    assert!(matches!(
        classify("fatal: '.' is a main working tree\n"),
        Error::MainWorktree { .. }
    ));
    assert!(matches!(
        classify("fatal: '../nope' is not a working tree\n"),
        Error::NotAWorktree { .. }
    ));
}

#[test]
fn the_progress_git_prints_before_a_fatal_is_not_mistaken_for_it() {
    // `add` writes this to stderr even when it is about to fail, and it has a
    // quoted name in it — the name of the branch being *created*, which is the
    // last thing to report as the branch that was already taken.
    let stderr = "Preparing worktree (new branch 'nb1')\nfatal: '../existing' already exists\n";

    match classify(stderr) {
        Error::PathExists { path } => assert_eq!(path, PathBuf::from("../existing")),
        other => panic!("expected PathExists, got {other:?}"),
    }
}

#[test]
fn an_unrecognised_failure_keeps_both_of_gits_lines_and_none_of_its_hints() {
    let stderr = "Preparing worktree (new branch 'bad..name')\n\
                  fatal: 'bad..name' is not a valid branch name\n\
                  hint: See 'git help check-ref-format'\n\
                  hint: Disable this message with \"git config set advice.refSyntax false\"\n";

    match classify(stderr) {
        Error::Failed { message } => assert_eq!(message, "'bad..name' is not a valid branch name"),
        other => panic!("expected Failed, got {other:?}"),
    }

    match classify("fatal: first line;\nsecond line\n") {
        Error::Failed { message } => assert_eq!(message, "first line; second line"),
        other => panic!("expected Failed, got {other:?}"),
    }
    match classify("") {
        Error::Failed { message } => assert_eq!(message, "git failed and gave no reason"),
        other => panic!("expected Failed, got {other:?}"),
    }
}

// --- naming the next one -------------------------------------------------------

#[test]
fn a_suggested_branch_avoids_every_branch_already_checked_out() {
    let first = suggested_branch(&[], &[]);
    let taken = vec![worktree_on(&first)];
    let second = suggested_branch(&taken, &[]);
    let third = suggested_branch(&[worktree_on(&first), worktree_on(&second)], &[]);

    assert_ne!(first, second);
    assert_ne!(second, third);
    assert_ne!(first, third);
    assert!(first.starts_with("worktree/"));
    // Deterministic: the same set of worktrees always suggests the same name,
    // which is what makes a retry after a failed add predictable.
    assert_eq!(suggested_branch(&[], &[]), first);
    assert_eq!(suggested_branch(&taken, &[]), second);
}

#[test]
fn a_suggested_branch_avoids_a_branch_nothing_has_checked_out() {
    // The half a worktree listing cannot see. `remove` never deletes a branch,
    // so a checkout that has been made and unmade leaves its name taken with
    // nothing holding it — and `add` refuses a name that merely exists.
    let first = suggested_branch(&[], &[]);
    let second = suggested_branch(&[], std::slice::from_ref(&first));

    assert_ne!(second, first);
    // Both sources are consulted, not one or the other: a name in either list
    // is a name that is spoken for.
    let third = suggested_branch(&[worktree_on(&second)], std::slice::from_ref(&first));
    assert_ne!(third, first);
    assert_ne!(third, second);
}

#[test]
fn a_suggested_branch_ignores_a_detached_worktree_that_has_no_branch_at_all() {
    let mut detached = worktree_on("unused");
    detached.branch = None;

    assert_eq!(
        suggested_branch(&[detached], &[]),
        suggested_branch(&[], &[])
    );
}

#[test]
fn a_suggested_branch_is_a_name_git_will_accept() {
    if without_git("a_suggested_branch_is_a_name_git_will_accept") {
        return;
    }
    let scratch = ScratchDir::new("suggest");
    let repo = repo_with_a_commit(&scratch, "repo");

    let name = suggested_branch(
        &list(&repo).expect("git answered"),
        &branches(&repo).expect("git answered"),
    );

    // Two ways of asking, because the second is the one that matters: git's own
    // ref grammar, and git actually taking the name.
    git(&repo, &["check-ref-format", &format!("refs/heads/{name}")]);
    add(&repo, &scratch.spot("suggested"), &name, Some("main")).expect("the name is usable");

    let worktrees = list(&repo).expect("git answered");
    assert_eq!(
        entry(&worktrees, &scratch.spot("suggested"))
            .branch
            .as_deref(),
        Some(name.as_str())
    );
    // And the next suggestion steps over the one just taken.
    assert_ne!(
        suggested_branch(&worktrees, &branches(&repo).expect("git answered")),
        name
    );
}

#[test]
fn a_suggested_branch_steps_over_the_branch_a_removed_worktree_left_behind() {
    if without_git("a_suggested_branch_steps_over_the_branch_a_removed_worktree_left_behind") {
        return;
    }
    let scratch = ScratchDir::new("suggest-after-remove");
    let repo = repo_with_a_commit(&scratch, "repo");
    let first_path = scratch.spot("first");

    // The whole loop a person takes: accept the offered name, make the
    // worktree, remove it again. Before this was fixed the next suggestion was
    // the same name, `add` failed with "a branch named … already exists", and
    // it failed that way for ever, because the walk is deterministic and the
    // branch is never deleted.
    let first = suggested(&repo);
    add(&repo, &first_path, &first, Some("main")).expect("the offered name is free");
    remove(&repo, &first_path, false).expect("nothing loose in it");

    assert!(
        branches(&repo)
            .expect("git answered")
            .contains(&first.clone()),
        "remove deleted the branch, which it is documented never to do"
    );
    assert!(
        !list(&repo)
            .expect("git answered")
            .iter()
            .any(|worktree| worktree.branch.as_deref() == Some(first.as_str())),
        "the checkout survived its own removal"
    );

    let second = suggested(&repo);

    assert_ne!(second, first);
    add(&repo, &scratch.spot("second"), &second, Some("main")).expect("the next name is free too");
}

#[test]
fn every_branch_is_listed_whether_or_not_it_is_checked_out() {
    if without_git("every_branch_is_listed_whether_or_not_it_is_checked_out") {
        return;
    }
    let scratch = ScratchDir::new("branches");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["branch", "sitting/idle"]);
    git(&repo, &["worktree", "add", "-b", "busy", "../busy", "main"]);
    // A tag by the same name as a branch, which is what `%(refname:short)`
    // would have printed as `heads/busy` — a name neither `add` nor the
    // worktree listing ever spells that way.
    git(&repo, &["tag", "busy", "main"]);

    let mut listed = branches(&repo).expect("git answered");
    listed.sort();

    assert_eq!(listed, ["busy", "main", "sitting/idle"]);
}

/// The name Crook would offer for the next worktree of `repo`, read the way
/// the menu reads it.
fn suggested(repo: &Path) -> String {
    suggested_branch(
        &list(repo).expect("git answered"),
        &branches(repo).expect("git answered"),
    )
}

/// A worktree that exists only to say a branch name is taken.
fn worktree_on(branch: &str) -> Worktree {
    Worktree {
        path: PathBuf::from("/nowhere"),
        head: None,
        branch: Some(branch.to_owned()),
        is_main: false,
        is_bare: false,
        locked: None,
        prunable: None,
    }
}

#[test]
fn a_slug_keeps_alphanumerics_lowercases_them_and_collapses_everything_else() {
    assert_eq!(slug("main"), "main");
    assert_eq!(slug("eugen/Fix Tab Bar"), "eugen-fix-tab-bar");
    assert_eq!(slug("release/2.0"), "release-2-0");
    // Runs collapse to one dash and the edges are trimmed, so no slug ever
    // starts or ends with a separator.
    assert_eq!(slug("--a///b--"), "a-b");
    assert_eq!(slug("  spaced  out  "), "spaced-out");
    // Non-ASCII is not transliterated, it is a separator like any other.
    assert_eq!(slug("caf\u{e9}-refactor"), "caf-refactor");
}

#[test]
fn a_slug_with_nothing_left_in_it_falls_back_to_a_name() {
    assert_eq!(slug(""), SLUG_FALLBACK);
    assert_eq!(slug("///"), SLUG_FALLBACK);
    assert_eq!(slug("\u{4e16}\u{754c}"), SLUG_FALLBACK);
}

#[test]
fn a_slug_is_short_enough_to_be_a_directory_on_every_filesystem() {
    let slugged = slug(&"very-long-branch-name-".repeat(20));

    assert!(slugged.len() <= SLUG_LIMIT);
    assert!(!slugged.ends_with('-'));
}

#[test]
fn a_checkout_path_is_the_store_then_the_repository_then_the_branch() {
    let store = Path::new("/home/eugen/.crook/worktrees");

    assert_eq!(
        checkout_path(store, "crook", "eugen/feature"),
        store.join("crook").join("eugen-feature")
    );
    // The repository name is slugged too: one store holds every repository a
    // person has open, and a directory called `Crook UI` would otherwise put a
    // space in every path underneath it.
    assert_eq!(
        checkout_path(store, "Crook UI", "main"),
        store.join("crook-ui").join("main")
    );
}
