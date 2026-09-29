//! Executable specification for the git reads.
//!
//! The parsing cases are pure and run anywhere. The rest build real
//! repositories in the system temp directory and ask git what it thinks —
//! because every bug this module can have is a bug about a layout git
//! produces and a hand-written fixture would get subtly wrong. A machine with
//! no git skips those with a message rather than failing: the branch half of
//! the module works without git, and its tests should too.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::process::command;

// --- scratch directories -----------------------------------------------------

/// A directory under the system temp directory, removed when it drops.
///
/// Hand-rolled rather than `tempfile`: it is twenty lines, and the workspace's
/// rule is that a dependency has to earn its place.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);

        let path =
            std::env::temp_dir().join(format!("crook-git-{label}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");

        // Spelled the way git will print it, once, now: git writes that form
        // into a linked worktree's `.git` file, and the tests here read it
        // back. See `git::as_git_prints_it`.
        let path = crate::git::as_git_prints_it(path);
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
/// `core.hooksPath` or `core.autocrlf` set would otherwise see these tests
/// fail for reasons that have nothing to do with the code.
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

/// A repository at `<scratch>/<name>` with one commit of a three-line file.
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

// --- reading HEAD --------------------------------------------------------------

#[test]
fn a_branch_name_keeps_every_slash_in_it() {
    let scratch = ScratchDir::new("head-slashes");
    let git_dir = scratch.dir("git-dir");
    write(
        &git_dir.join("HEAD"),
        "ref: refs/heads/eugen/claude-code-usage-indicator\n",
    );

    assert_eq!(
        read_head(&git_dir),
        Some(Head::Branch("eugen/claude-code-usage-indicator".to_owned()))
    );
}

#[test]
fn a_bare_object_id_in_head_is_a_detached_head_shown_as_a_short_sha() {
    let scratch = ScratchDir::new("head-detached");
    let git_dir = scratch.dir("git-dir");
    write(
        &git_dir.join("HEAD"),
        "1a22cb92d4e5f60718293a4b5c6d7e8f90123456\n",
    );

    let head = read_head(&git_dir).expect("a bare object id is a legal HEAD");

    assert!(head.is_detached());
    assert_eq!(head.label(), "1a22cb9");
    assert_eq!(
        head,
        Head::Detached {
            short: "1a22cb9".to_owned(),
            full: "1a22cb92d4e5f60718293a4b5c6d7e8f90123456".to_owned(),
        }
    );
}

#[test]
fn a_sha256_object_id_is_a_detached_head_too() {
    let scratch = ScratchDir::new("head-sha256");
    let git_dir = scratch.dir("git-dir");
    let full = "0".repeat(63) + "f";
    write(&git_dir.join("HEAD"), &format!("{full}\n"));

    assert_eq!(
        read_head(&git_dir).as_ref().map(Head::label),
        Some("0000000")
    );
}

#[test]
fn a_symbolic_head_outside_refs_heads_shows_its_last_segment() {
    let scratch = ScratchDir::new("head-tag");
    let git_dir = scratch.dir("git-dir");
    write(&git_dir.join("HEAD"), "ref: refs/tags/v1.2.0\n");

    assert_eq!(read_head(&git_dir), Some(Head::Branch("v1.2.0".to_owned())));
}

#[test]
fn a_head_that_is_neither_form_is_no_branch_rather_than_a_branch_called_that() {
    let scratch = ScratchDir::new("head-garbage");
    let git_dir = scratch.dir("git-dir");
    write(&git_dir.join("HEAD"), "not a head at all\n");

    assert_eq!(read_head(&git_dir), None);
    assert_eq!(read_head(&scratch.path().join("nothing-here")), None);
}

#[test]
fn a_reftable_repositorys_placeholder_head_is_no_branch_rather_than_a_branch_called_that() {
    // `git init --ref-format=reftable` writes exactly this: the branch lives
    // in the binary tables, and the file holds a ref that cannot exist.
    let scratch = ScratchDir::new("head-reftable");
    let git_dir = scratch.dir("git-dir");
    write(&git_dir.join("HEAD"), "ref: refs/heads/.invalid\n");

    assert_eq!(read_head(&git_dir), None);
}

// --- finding the repository ----------------------------------------------------

#[test]
fn a_directory_outside_a_repository_has_no_facts() {
    let scratch = ScratchDir::new("no-repo");
    let inside = scratch.dir("deep/deeper");

    if let Some(repo) = repository_above(scratch.path()) {
        eprintln!(
            "skipping a_directory_outside_a_repository_has_no_facts: the system temp directory \
             is inside the repository at {}",
            repo.display()
        );
        return;
    }

    assert_eq!(discover(&inside), None);
    assert_eq!(current_branch(&inside), None);
    assert_eq!(gather(&inside, &mut Bases::default()), GitFacts::default());
}

#[test]
fn a_relative_gitdir_pointer_resolves_against_the_file_that_holds_it() {
    // The shape a submodule has: `.git` is a file, and the path in it is
    // relative to the directory the file sits in, not to the process cwd.
    let scratch = ScratchDir::new("relative-pointer");
    let store = scratch.dir("store");
    write(&store.join("HEAD"), "ref: refs/heads/vendor/pinned\n");
    let checkout = scratch.dir("checkout");
    write(&checkout.join(".git"), "gitdir: ../store\n");

    let layout = discover(&checkout).expect("the pointer names a git directory");

    assert_eq!(layout.work_tree.as_deref(), Some(checkout.as_path()));
    assert_eq!(layout.git_dir, store);
    assert_eq!(layout.common_dir, store);
    assert_eq!(
        current_branch(&checkout),
        Some(Head::Branch("vendor/pinned".to_owned()))
    );
}

#[test]
fn a_gitdir_pointer_at_something_that_is_not_a_repository_is_ignored() {
    let scratch = ScratchDir::new("dangling-pointer");
    let checkout = scratch.dir("checkout");
    scratch.dir("store");
    write(&checkout.join(".git"), "gitdir: ../store\n");

    assert_eq!(discover(&checkout), None);
}

#[test]
fn the_walk_stops_before_the_home_directory() {
    let Some(home) = std::env::home_dir() else {
        eprintln!("skipping the_walk_stops_before_the_home_directory: no home directory");
        return;
    };

    // Whether or not `~` is itself a checkout, discovery must refuse to answer
    // for it — that is what stops a dotfiles repository from claiming every
    // directory beneath it.
    assert_eq!(discover(&home), None);
}

// --- what git actually produces ------------------------------------------------

#[test]
fn a_repository_reports_the_branch_it_is_on_from_any_depth() {
    if without_git("a_repository_reports_the_branch_it_is_on_from_any_depth") {
        return;
    }
    let scratch = ScratchDir::new("branch");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["checkout", "-b", "eugen/nested/feature"]);

    let deep = repo.join("app/src/git");
    std::fs::create_dir_all(&deep).expect("the scratch directory is writable");

    let expected = Some(Head::Branch("eugen/nested/feature".to_owned()));
    assert_eq!(current_branch(&repo), expected);
    assert_eq!(current_branch(&deep), expected);

    let layout = discover(&deep).expect("the repository is above it");
    assert_eq!(layout.work_tree.as_deref(), Some(repo.as_path()));
    assert_eq!(layout.git_dir, repo.join(".git"));
    assert_eq!(layout.common_dir, repo.join(".git"));
}

#[test]
fn a_detached_head_reads_as_the_short_sha_git_itself_would_print() {
    if without_git("a_detached_head_reads_as_the_short_sha_git_itself_would_print") {
        return;
    }
    let scratch = ScratchDir::new("detached");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["checkout", "--detach"]);

    let head = current_branch(&repo).expect("a detached head is still a head");

    assert!(head.is_detached());
    assert_eq!(
        head.label(),
        git(&repo, &["rev-parse", "--short=7", "HEAD"])
    );
}

#[test]
fn a_packed_repository_reads_the_same_branch_as_a_loose_one() {
    if without_git("a_packed_repository_reads_the_same_branch_as_a_loose_one") {
        return;
    }
    let scratch = ScratchDir::new("packed");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["checkout", "-b", "release/2.0"]);
    git(&repo, &["pack-refs", "--all"]);

    // HEAD is never packed, which is the whole reason this module needs no
    // packed-refs code path. Prove the refs really were packed first.
    assert!(!repo.join(".git/refs/heads/release/2.0").exists());
    assert!(repo.join(".git/packed-refs").is_file());

    assert_eq!(
        current_branch(&repo),
        Some(Head::Branch("release/2.0".to_owned()))
    );
}

#[test]
fn a_flat_refs_heads_past_the_limit_is_capped_like_a_packed_one() {
    // The cap exists so a repository with tens of thousands of refs costs a
    // long list rather than a plugin's whole memory, and a flat `refs/heads`
    // — every branch a plain name, no `/` — is the ordinary layout. The loose
    // walk has to honour the cap there too, not only when it recurses into a
    // subdirectory. No git needed: the walk reads the files directly.
    let scratch = ScratchDir::new("many-loose-branches");
    let git_dir = scratch.path().join("git");
    let heads = git_dir.join("refs").join("heads");
    std::fs::create_dir_all(&heads).expect("the scratch directory is writable");
    for index in 0..branch::REF_LIMIT + 10 {
        std::fs::write(heads.join(format!("b{index:05}")), "")
            .expect("the scratch directory is writable");
    }

    let layout = RepoLayout {
        work_tree: None,
        git_dir: git_dir.clone(),
        common_dir: git_dir,
    };

    assert_eq!(branches_in(&layout).len(), branch::REF_LIMIT);
}

#[test]
fn a_linked_worktree_finds_the_repository_it_was_added_from() {
    if without_git("a_linked_worktree_finds_the_repository_it_was_added_from") {
        return;
    }
    let scratch = ScratchDir::new("worktree");
    let main = repo_with_a_commit(&scratch, "main");
    git(&main, &["worktree", "add", "-b", "side", "../side"]);
    let side = scratch.path().join("side");

    let layout = discover(&side).expect("a worktree is in a repository");

    assert_eq!(layout.work_tree.as_deref(), Some(side.as_path()));
    assert_eq!(
        layout.git_dir,
        main.join(".git").join("worktrees").join("side")
    );
    assert_eq!(layout.common_dir, main.join(".git"));
    assert_eq!(current_branch(&side), Some(Head::Branch("side".to_owned())));
}

#[test]
fn a_worktree_that_lost_its_commondir_is_still_found_by_where_it_lives() {
    // The fallback for an admin directory with no `commondir` in it: the
    // `<main>/.git/worktrees/<name>` shape still gives it away.
    if without_git("a_worktree_that_lost_its_commondir_is_still_found_by_where_it_lives") {
        return;
    }
    let scratch = ScratchDir::new("worktree-no-commondir");
    let main = repo_with_a_commit(&scratch, "main");
    git(&main, &["worktree", "add", "-b", "side", "../side"]);
    let admin = main.join(".git").join("worktrees").join("side");
    std::fs::remove_file(admin.join("commondir")).expect("git writes one");

    let layout = discover(&scratch.path().join("side")).expect("still a repository");
    assert_eq!(layout.common_dir, main.join(".git"));
}

#[test]
fn a_submodule_under_a_directory_named_worktrees_is_not_a_linked_worktree() {
    // Its repository is `.git/modules/worktrees/lib`, which has the shape of
    // a linked worktree's admin directory without being one.
    if without_git("a_submodule_under_a_directory_named_worktrees_is_not_a_linked_worktree") {
        return;
    }
    let scratch = ScratchDir::new("submodule-worktrees");
    let upstream = repo_with_a_commit(&scratch, "upstream");
    let outer = repo_with_a_commit(&scratch, "outer");
    let source = upstream.to_str().expect("a UTF-8 scratch path");
    git(
        &outer,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source,
            "worktrees/lib",
        ],
    );
    let lib = outer.join("worktrees").join("lib");

    let layout = discover(&lib).expect("a submodule is a repository");
    assert_eq!(layout.git_dir, outer.join(".git/modules/worktrees/lib"));
    assert_eq!(
        layout.common_dir, layout.git_dir,
        "a submodule owns its refs"
    );
    assert!(
        !facts_without_diff(&lib).worktree,
        "a submodule is not a worktree"
    );
    assert_eq!(branches(&lib), vec![String::from("main")]);
}

#[cfg(unix)]
#[test]
fn a_directory_symlinked_into_a_repository_has_its_branch() {
    // `~/notes -> ~/Work/repo/docs`: the shell's `$PWD` is the link, whose
    // parents are not the repository's, and git still names the branch.
    if without_git("a_directory_symlinked_into_a_repository_has_its_branch") {
        return;
    }
    let scratch = ScratchDir::new("dir-symlink");
    let repo = repo_with_a_commit(&scratch, "repo");
    let docs = repo.join("docs");
    std::fs::create_dir_all(&docs).expect("a directory in the repository");
    let link = scratch.dir("elsewhere").join("notes");
    std::os::unix::fs::symlink(&docs, &link).expect("a link");

    assert_eq!(git(&link, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(current_branch(&link), Some(Head::Branch("main".to_owned())));
}

#[cfg(unix)]
#[test]
fn a_submodule_opened_through_a_symlink_keeps_its_branch() {
    // Its `.git` file points at `../.git/modules/lib`, which is beside the
    // submodule on disk and not beside the link.
    if without_git("a_submodule_opened_through_a_symlink_keeps_its_branch") {
        return;
    }
    let scratch = ScratchDir::new("submodule-symlink");
    let upstream = repo_with_a_commit(&scratch, "upstream");
    let outer = repo_with_a_commit(&scratch, "outer");
    let source = upstream.to_str().expect("a UTF-8 scratch path");
    git(
        &outer,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source,
            "lib",
        ],
    );
    let link = scratch.path().join("lib-link");
    std::os::unix::fs::symlink(outer.join("lib"), &link).expect("a link");

    assert_eq!(git(&link, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(current_branch(&link), Some(Head::Branch("main".to_owned())));
}

#[test]
fn a_bare_repository_is_recognised_by_its_own_name() {
    if without_git("a_bare_repository_is_recognised_by_its_own_name") {
        return;
    }
    let scratch = ScratchDir::new("bare");
    let mirror = scratch.dir("mirror.git");
    git(&mirror, &["init", "--bare"]);

    let layout = discover(&mirror).expect("a bare repository is a repository");

    assert_eq!(layout.work_tree, None);
    assert_eq!(layout.git_dir, mirror);
    // Nothing to diff without a working tree, and nothing spawned to find out.
    assert_eq!(gather(&mirror, &mut Bases::default()).diff, None);
    assert!(gather(&mirror, &mut Bases::default()).branch.is_some());
}

// --- diff stats ----------------------------------------------------------------

#[test]
fn a_clean_tree_reads_as_zero_changes_rather_than_as_no_reading() {
    if without_git("a_clean_tree_reads_as_zero_changes_rather_than_as_no_reading") {
        return;
    }
    let scratch = ScratchDir::new("clean");
    let repo = repo_with_a_commit(&scratch, "repo");

    let stats = diff_stats_blocking(&repo).expect("git answered");

    assert!(stats.is_empty());
    assert_eq!(stats, DiffStats::default());
    assert_eq!(
        gather(&repo, &mut Bases::default()).diff,
        Some(DiffStats::default())
    );
}

#[test]
fn staged_and_unstaged_changes_are_both_counted_and_untracked_ones_are_not() {
    if without_git("staged_and_unstaged_changes_are_both_counted_and_untracked_ones_are_not") {
        return;
    }
    let scratch = ScratchDir::new("dirty");
    let repo = repo_with_a_commit(&scratch, "repo");

    write(&repo.join("tracked.txt"), "one\ntwo\nthree\nfour\n");
    write(&repo.join("added.txt"), "alpha\nbeta\n");
    git(&repo, &["add", "tracked.txt", "added.txt"]);
    // Unstaged, on top of the staged change to the same file.
    write(&repo.join("tracked.txt"), "one\ntwo\nfour\nfive\nsix\n");
    // Untracked, and deliberately not counted: `diff --shortstat HEAD` only
    // sees tracked content. Warp's watcher-backed path does count these, so
    // the two legitimately disagree here.
    write(&repo.join("untracked.txt"), "gamma\ndelta\n");

    let stats = diff_stats_blocking(&repo).expect("git answered");

    assert_eq!(
        stats,
        DiffStats {
            files_changed: 2,
            lines_added: 5,
            lines_removed: 1,
        }
    );
    assert!(!stats.is_empty());
    assert_eq!(stats.tokens(), ["+5", "-1"]);
}

#[test]
fn a_repository_with_no_commits_yet_has_no_stats_to_read() {
    if without_git("a_repository_with_no_commits_yet_has_no_stats_to_read") {
        return;
    }
    let scratch = ScratchDir::new("no-commits");
    let repo = scratch.dir("repo");
    git(&repo, &["init"]);
    write(&repo.join("staged.txt"), "one\n");
    git(&repo, &["add", "."]);

    // There is no HEAD to diff against, which is a missing number and not an
    // error the row has to render. The branch still reads, from the file.
    assert_eq!(diff_stats_blocking(&repo), None);
    assert_eq!(gather(&repo, &mut Bases::default()).diff, None);
    assert!(gather(&repo, &mut Bases::default()).branch.is_some());
}

#[test]
fn a_directory_that_is_not_a_repository_has_no_stats_and_no_error() {
    if without_git("a_directory_that_is_not_a_repository_has_no_stats_and_no_error") {
        return;
    }
    let scratch = ScratchDir::new("stats-no-repo");

    assert_eq!(diff_stats_blocking(scratch.path()), None);
}

// --- work since the base ----------------------------------------------------------

/// Writes `file` in `repo` and commits it on whatever is checked out.
fn commit(repo: &Path, file: &str, contents: &str, message: &str) {
    write(&repo.join(file), contents);
    git(repo, &["add", file]);
    git(repo, &["commit", "--no-verify", "-m", message]);
}

#[test]
fn a_branch_two_commits_ahead_with_an_edit_counts_both_commits_and_every_line_since_main() {
    if without_git(
        "a_branch_two_commits_ahead_with_an_edit_counts_both_commits_and_every_line_since_main",
    ) {
        return;
    }
    let scratch = ScratchDir::new("ahead");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    commit(&repo, "notes.txt", "alpha\nbeta\n", "add notes");
    // Not committed, on top of both: the count since the base takes it in.
    write(&repo.join("tracked.txt"), "uno\ntwo\nthree\nfour\n");

    let facts = gather(&repo, &mut Bases::default());

    assert_eq!(
        facts.since_base,
        Some(SinceBase {
            base: "main".to_owned(),
            commits: 2,
            // `one` became `uno` and `four` arrived in tracked.txt, and
            // notes.txt is new: committed and uncommitted, as one number.
            diff: DiffStats {
                files_changed: 2,
                lines_added: 4,
                lines_removed: 1,
            },
        })
    );
    // The count against `HEAD` is what it always was: the edit alone.
    assert_eq!(
        facts.diff,
        Some(DiffStats {
            files_changed: 1,
            lines_added: 1,
            lines_removed: 1,
        })
    );
}

#[test]
fn on_main_itself_the_count_is_the_plain_one_even_ahead_of_origin() {
    // The sharp case of "the base itself": `base_of` prefers the
    // remote-tracking branch, and a local `main` with a commit it has not
    // pushed *is* one ahead of `origin/main` — but it is the base, not a
    // branch that left it, and its row says what it always said.
    if without_git("on_main_itself_the_count_is_the_plain_one_even_ahead_of_origin") {
        return;
    }
    let scratch = ScratchDir::new("on-main");
    let upstream = repo_with_a_commit(&scratch, "upstream");
    let clone = scratch.path().join("clone");
    git(
        scratch.path(),
        &["clone", "--quiet", &upstream.to_string_lossy(), "clone"],
    );
    commit(&clone, "tracked.txt", "one\ntwo\nthree\nfour\n", "unpushed");
    write(&clone.join("tracked.txt"), "one\ntwo\nthree\nfour\nfive\n");
    assert_eq!(
        merged::base_of(&clone).as_deref(),
        Some("refs/remotes/origin/main"),
        "the clone's base is the remote-tracking branch, or this proves nothing"
    );

    let facts = gather(&clone, &mut Bases::default());

    assert_eq!(facts.since_base, None);
    assert_eq!(
        facts.diff,
        Some(DiffStats {
            files_changed: 1,
            lines_added: 1,
            lines_removed: 0,
        })
    );
}

#[test]
fn a_detached_head_keeps_the_plain_count() {
    if without_git("a_detached_head_keeps_the_plain_count") {
        return;
    }
    let scratch = ScratchDir::new("detached-ahead");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    // A commit ahead of `main`, checked out directly: on no branch, so there
    // is no branch to have left anything.
    git(&repo, &["switch", "--detach"]);
    write(&repo.join("tracked.txt"), "one\ntwo\nthree\n");

    let facts = gather(&repo, &mut Bases::default());

    assert!(facts.branch.as_ref().is_some_and(Head::is_detached));
    assert_eq!(facts.since_base, None);
    assert_eq!(
        facts.diff,
        Some(DiffStats {
            files_changed: 1,
            lines_added: 0,
            lines_removed: 1,
        })
    );
}

#[test]
fn a_repository_with_no_base_keeps_the_plain_count() {
    if without_git("a_repository_with_no_base_keeps_the_plain_count") {
        return;
    }
    let scratch = ScratchDir::new("no-base");
    let repo = repo_with_a_commit(&scratch, "repo");
    // No remote, no `main`, no `master`, and a default branch set here — over
    // whatever the machine's own configuration says — that does not exist.
    git(&repo, &["branch", "-m", "main", "trunk"]);
    git(&repo, &["config", "init.defaultBranch", "nowhere"]);
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");

    let facts = gather(&repo, &mut Bases::default());

    assert_eq!(facts.since_base, None);
    assert_eq!(facts.diff, Some(DiffStats::default()));
}

#[test]
fn a_branch_with_no_commit_yet_has_nothing_since_its_base() {
    if without_git("a_branch_with_no_commit_yet_has_nothing_since_its_base") {
        return;
    }
    let scratch = ScratchDir::new("unborn-branch");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "--orphan", "fresh"]);

    let facts = gather(&repo, &mut Bases::default());

    assert_eq!(facts.branch, Some(Head::Branch("fresh".to_owned())));
    assert_eq!(facts.since_base, None);
}

#[test]
fn a_repositorys_base_is_looked_up_once_across_refreshes_and_by_all_its_worktrees() {
    // A base is up to three subprocesses and almost never changes. Asked
    // every fifteen seconds for every row it would more than double what a
    // refresh costs; asked once per repository, each directory a refresh
    // gathers adds at most the two that count.
    static LOOKUPS: AtomicU32 = AtomicU32::new(0);
    fn counted(repository: &Path) -> Option<String> {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        merged::base_of(repository)
    }
    if without_git("a_repositorys_base_is_looked_up_once_across_refreshes_and_by_all_its_worktrees")
    {
        return;
    }
    let scratch = ScratchDir::new("base-once");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "one"]);
    commit(&repo, "one.txt", "one\n", "one");
    let two = scratch.path().join("two");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "two",
            &two.to_string_lossy(),
            "main",
        ],
    );
    commit(&two, "two.txt", "two\n", "two");

    let mut bases = Bases::looking_up_with(counted);
    for refresh in 0..3 {
        for directory in [&repo, &two] {
            let facts = gather(directory, &mut bases);
            assert_eq!(
                facts.since_base.map(|since| (since.base, since.commits)),
                Some(("main".to_owned(), 1)),
                "refresh {refresh} of {} lost its count",
                directory.display()
            );
        }
        bases.forget_unasked();
    }

    assert_eq!(
        LOOKUPS.load(Ordering::Relaxed),
        1,
        "two checkouts of one repository, three refreshes: one lookup"
    );
}

#[test]
fn a_repository_that_had_no_base_asks_again_from_a_branch_it_has_not_asked_from() {
    // A repository made a minute ago has no `main` until its first commit.
    // Keeping that first "none" for good would count nothing on any branch of
    // it for as long as it stayed on screen.
    static LOOKUPS: AtomicU32 = AtomicU32::new(0);
    fn counted(repository: &Path) -> Option<String> {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        merged::base_of(repository)
    }
    if without_git("a_repository_that_had_no_base_asks_again_from_a_branch_it_has_not_asked_from") {
        return;
    }
    let scratch = ScratchDir::new("base-later");
    let repo = scratch.dir("repo");
    git(&repo, &["init"]);
    let mut bases = Bases::looking_up_with(counted);

    assert_eq!(gather(&repo, &mut bases).since_base, None);
    assert_eq!(gather(&repo, &mut bases).since_base, None);
    assert_eq!(
        LOOKUPS.load(Ordering::Relaxed),
        1,
        "the same branch asked twice for a base it was told is not there"
    );

    commit(&repo, "tracked.txt", "one\n", "initial");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\n", "two");

    let facts = gather(&repo, &mut bases);
    assert_eq!(facts.since_base.map(|since| since.commits), Some(1));
    gather(&repo, &mut bases);
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 2);
}

#[test]
fn a_repository_no_gather_asked_about_for_a_whole_cycle_is_looked_up_afresh() {
    static LOOKUPS: AtomicU32 = AtomicU32::new(0);
    fn counted(repository: &Path) -> Option<String> {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        merged::base_of(repository)
    }
    if without_git("a_repository_no_gather_asked_about_for_a_whole_cycle_is_looked_up_afresh") {
        return;
    }
    let scratch = ScratchDir::new("base-forgotten");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    let mut bases = Bases::looking_up_with(counted);

    // Two cycles that asked: kept through both.
    gather(&repo, &mut bases);
    bases.forget_unasked();
    gather(&repo, &mut bases);
    bases.forget_unasked();
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 1);

    // A cycle in which no row was in this repository, and then one that is.
    bases.forget_unasked();
    gather(&repo, &mut bases);
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 2);
}

// --- parsing and display -------------------------------------------------------

#[test]
fn shortstat_parses_singular_plural_and_missing_clauses() {
    assert_eq!(
        DiffStats::parse_shortstat(" 1 file changed, 2 insertions(+), 17 deletions(-)\n"),
        DiffStats {
            files_changed: 1,
            lines_added: 2,
            lines_removed: 17,
        }
    );
    assert_eq!(
        DiffStats::parse_shortstat(" 3 files changed, 45 insertions(+)\n"),
        DiffStats {
            files_changed: 3,
            lines_added: 45,
            lines_removed: 0,
        }
    );
    assert_eq!(
        DiffStats::parse_shortstat(" 2 files changed, 8 deletions(-)\n"),
        DiffStats {
            files_changed: 2,
            lines_added: 0,
            lines_removed: 8,
        }
    );
    assert_eq!(
        DiffStats::parse_shortstat(" 1 file changed, 1 insertion(+), 1 deletion(-)\n"),
        DiffStats {
            files_changed: 1,
            lines_added: 1,
            lines_removed: 1,
        }
    );
}

#[test]
fn empty_shortstat_output_is_a_clean_tree() {
    assert!(DiffStats::parse_shortstat("").is_empty());
    assert!(DiffStats::parse_shortstat("\n").is_empty());
}

#[test]
fn the_badge_omits_a_side_that_is_zero_and_says_zero_when_both_are() {
    let added = DiffStats {
        files_changed: 1,
        lines_added: 12,
        lines_removed: 0,
    };
    let removed = DiffStats {
        files_changed: 1,
        lines_added: 0,
        lines_removed: 3,
    };

    assert_eq!(added.tokens(), ["+12"]);
    assert_eq!(removed.tokens(), ["-3"]);
    assert_eq!(DiffStats::default().tokens(), ["0"]);
}

#[test]
fn a_base_is_called_by_the_name_a_person_calls_it() {
    assert_eq!(diff::base_name("refs/heads/main"), "main");
    assert_eq!(diff::base_name("refs/remotes/origin/main"), "main");
    assert_eq!(diff::base_name("refs/heads/release/2.0"), "release/2.0");
    assert_eq!(
        diff::base_name("refs/remotes/origin/release/2.0"),
        "release/2.0"
    );
    assert_eq!(diff::base_name("refs/tags/v1"), "refs/tags/v1");
}

#[test]
fn a_working_directory_under_home_is_printed_with_a_tilde() {
    let home = Path::new("/Users/eugen");

    assert_eq!(
        user_friendly_path(Path::new("/Users/eugen/work/crook"), Some(home)),
        "~/work/crook"
    );
    assert_eq!(user_friendly_path(home, Some(home)), "~");
    assert_eq!(
        user_friendly_path(Path::new("/opt/crook"), Some(home)),
        "/opt/crook"
    );
    // A sibling whose name merely starts with the home directory's is not
    // under it, and abbreviating it would name a directory that is not there.
    assert_eq!(
        user_friendly_path(Path::new("/Users/eugene/work"), Some(home)),
        "/Users/eugene/work"
    );
    assert_eq!(
        user_friendly_path(Path::new("/Users/eugen/work"), None),
        "/Users/eugen/work"
    );
}

#[test]
fn a_directory_is_called_by_its_last_component() {
    let home = Path::new("/Users/eugen");
    assert_eq!(
        directory_label(Path::new("/Users/eugen/work/crook"), Some(home)),
        Some("crook".to_owned())
    );
    // The home directory is `~`, not the account name. A macOS app opened from
    // the Dock starts there, so this is the first thing a new tab is called.
    assert_eq!(directory_label(home, Some(home)), Some("~".to_owned()));
    // Nowhere to be. The row falls back past this to the placeholder.
    assert_eq!(directory_label(Path::new("/"), Some(home)), None);
    // No home to compare against is not a reason to have no name.
    assert_eq!(
        directory_label(Path::new("/opt/crook"), None),
        Some("crook".to_owned())
    );
}
