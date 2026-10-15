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
    let output = git_output(dir, args);

    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// [`git`], for a command that is expected to fail — a rebase that stops on
/// a conflict — handing back whatever it did.
fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    git_output_with(dir, args, &[])
}

/// [`git_output`], with `env` set on top.
fn git_output_with(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let nowhere = dir.join("no-such-gitconfig");
    command("git")
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
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .expect("git is installed; the caller checked")
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
fn a_rebase_in_progress_is_the_branch_it_is_rebasing() {
    // Both backends' files, as git writes them: `HEAD` a bare object id and
    // the branch's full ref in `head-name`.
    let scratch = ScratchDir::new("rebasing");
    for backend in ["rebase-merge", "rebase-apply"] {
        let repo = scratch.dir(backend);
        let git_dir = repo.join(".git");
        write(
            &git_dir.join("HEAD"),
            "1a22cb92d4e5f60718293a4b5c6d7e8f90123456\n",
        );
        write(
            &git_dir.join(backend).join("head-name"),
            "refs/heads/feat/pr-on-the-row\n",
        );

        assert_eq!(
            branch::rebasing(&git_dir).as_deref(),
            Some("feat/pr-on-the-row"),
            "{backend}"
        );
        assert_eq!(
            branch_at_work(&repo).map(|work| work.head),
            Some(Head::Branch("feat/pr-on-the-row".to_owned())),
            "{backend}"
        );
        // The row's own label still says where the rebase has got to.
        assert!(current_branch(&repo).is_some_and(|head| head.is_detached()));
    }
}

#[test]
fn a_detached_head_with_no_branch_being_rebased_is_still_detached() {
    let scratch = ScratchDir::new("rebasing-detached");
    let repo = scratch.dir("repo");
    let git_dir = repo.join(".git");
    write(
        &git_dir.join("HEAD"),
        "1a22cb92d4e5f60718293a4b5c6d7e8f90123456\n",
    );
    assert!(branch_at_work(&repo).is_some_and(|work| work.head.is_detached()));

    // What git writes when the thing being rebased was a detached head.
    write(
        &git_dir.join("rebase-merge").join("head-name"),
        "detached HEAD\n",
    );
    assert_eq!(branch::rebasing(&git_dir), None);
    assert!(branch_at_work(&repo).is_some_and(|work| work.head.is_detached()));

    // And a branch is itself, whatever a stale file says.
    write(&git_dir.join("HEAD"), "ref: refs/heads/main\n");
    write(
        &git_dir.join("rebase-merge").join("head-name"),
        "refs/heads/feat\n",
    );
    assert_eq!(
        branch_at_work(&repo).map(|work| work.head),
        Some(Head::Branch("main".to_owned()))
    );
}

#[test]
fn a_rebase_stopped_on_a_conflict_is_still_on_its_branch() {
    if without_git("a_rebase_stopped_on_a_conflict_is_still_on_its_branch") {
        return;
    }
    // The case a pull request has to live through: its branch brought up to
    // date with main, stopped on a conflict for as long as it takes to fix.
    let scratch = ScratchDir::new("rebase-conflict");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["checkout", "-b", "feat"]);
    write(&repo.join("tracked.txt"), "feat\ntwo\nthree\n");
    git(&repo, &["commit", "--no-verify", "-am", "feat"]);
    git(&repo, &["checkout", "main"]);
    write(&repo.join("tracked.txt"), "main\ntwo\nthree\n");
    git(&repo, &["commit", "--no-verify", "-am", "main"]);
    git(&repo, &["checkout", "feat"]);

    let rebase = git_output(&repo, &["rebase", "main"]);
    assert!(!rebase.status.success(), "the rebase was meant to stop");

    assert!(
        current_branch(&repo).is_some_and(|head| head.is_detached()),
        "git no longer detaches HEAD for a rebase, and this test proves nothing"
    );
    assert_eq!(
        branch_at_work(&repo),
        Some(BranchAtWork {
            repository: repo.join(".git"),
            head: Head::Branch("feat".to_owned()),
        })
    );

    git(&repo, &["rebase", "--abort"]);
    assert_eq!(
        branch_at_work(&repo).map(|work| work.head),
        Some(Head::Branch("feat".to_owned()))
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
fn a_lock_file_or_a_dot_file_under_refs_heads_is_not_a_branch() {
    // What a git killed mid-update and a Finder window leave behind. No git
    // needed: the walk reads the files directly.
    let scratch = ScratchDir::new("lock-files");
    let git_dir = scratch.path().join("git");
    let heads = git_dir.join("refs").join("heads");
    for name in [
        "main",
        "main.lock",
        ".DS_Store",
        "eugen/thing",
        "eugen/thing.lock",
    ] {
        write(&heads.join(name), "");
    }
    let layout = RepoLayout {
        work_tree: None,
        git_dir: git_dir.clone(),
        common_dir: git_dir,
    };

    assert_eq!(branches_in(&layout), ["eugen/thing", "main"]);
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

    // Another checkout of the same repository, whose branches are its
    // branches: what a pull request is a branch of is the one repository.
    let in_side = branch_at_work(&side).expect("a worktree is in a repository");
    let in_main = branch_at_work(&main).expect("so is the checkout it came from");
    assert_eq!(in_side.repository, main.join(".git"));
    assert!(in_side.is_same_repository(&in_main));
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
fn what_main_did_after_the_branch_left_it_is_not_counted_against_the_branch() {
    // The common case, and the one the test above cannot tell apart: there
    // `main` never moved, so its tip *is* where the branch left it, and a
    // diff against `main` and one against the fork print the same thing.
    // Once `main` has work of its own, only the fork keeps that work out of
    // the branch's count — a diff against `main`'s tip would read every line
    // `main` added as a line this branch removed.
    if without_git("what_main_did_after_the_branch_left_it_is_not_counted_against_the_branch") {
        return;
    }
    let scratch = ScratchDir::new("base-moved-on");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    commit(&repo, "notes.txt", "alpha\nbeta\n", "add notes");
    git(&repo, &["switch", "main"]);
    commit(&repo, "other.txt", "x\ny\nz\n", "main moves on");
    commit(&repo, "tracked.txt", "zero\none\ntwo\nthree\n", "on main");
    git(&repo, &["switch", "agent/task"]);
    write(&repo.join("tracked.txt"), "uno\ntwo\nthree\nfour\n");

    let facts = gather(&repo, &mut Bases::default());

    assert_eq!(
        facts.since_base,
        Some(SinceBase {
            base: "main".to_owned(),
            // `main`'s two new commits are not this branch's.
            commits: 2,
            // Exactly the branch's own lines, as when `main` stood still.
            diff: DiffStats {
                files_changed: 2,
                lines_added: 4,
                lines_removed: 1,
            },
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

// --- reading only what changed ------------------------------------------------
//
// A poll gathers every directory a pane is in, every cycle, and almost none of
// them have changed since the last. These count the gits a second gather
// starts, because a read skipped and a read that answered what it answered
// last time look the same from the facts alone.

/// Lets every stamp this thread takes stand at once, rather than refusing
/// files the test wrote a moment ago — see `stamp::settling`.
fn settle_at_once() {
    stamp::SETTLING_FOR.with(|settling| settling.set(std::time::Duration::ZERO));
}

/// The gits this thread has started since it last asked, by subcommand and
/// what tells the two diffs apart: `diff HEAD`, `diff --merge-base`,
/// `rev-list`.
fn reads_since() -> Vec<String> {
    run::started_here()
        .into_iter()
        .map(|read| {
            let words: Vec<&str> = read.split(' ').collect();
            match words.as_slice() {
                ["diff", rest @ ..] if rest.contains(&"--merge-base") => {
                    "diff --merge-base".to_owned()
                }
                ["diff", rest @ ..] if rest.contains(&"HEAD") => "diff HEAD".to_owned(),
                [command, ..] => (*command).to_owned(),
                [] => String::new(),
            }
        })
        .collect()
}

/// A repository on `agent/task`, a commit ahead of `main` and with an edit
/// nobody has added, gathered once already through `readings`.
fn gathered_once(scratch: &ScratchDir, readings: &mut Readings, bases: &mut Bases) -> PathBuf {
    let repo = repo_with_a_commit(scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "four");
    write(&repo.join("tracked.txt"), "one\ntwo\nthree\nfour\nfive\n");

    let facts = gather_remembering(&repo, bases, readings);
    assert_eq!(
        facts.since_base.as_ref().map(|since| since.commits),
        Some(1),
        "the first gather counted nothing: {facts:?}"
    );
    readings.forget_unvisited();
    reads_since();
    repo
}

#[test]
fn a_work_tree_nothing_has_written_to_is_not_read_again() {
    // A checkout an agent has finished with sits on screen for hours, and each
    // of the three reads goes through its whole tree — in a checkout that
    // `worktree add` has just made, hashing every file in it. A second gather
    // with nothing moved asks where the commits are and lists the tree, and
    // reads nothing.
    if without_git("a_work_tree_nothing_has_written_to_is_not_read_again") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("unchanged");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = gathered_once(&scratch, &mut readings, &mut bases);
    let first = gather(&repo, &mut Bases::default());
    reads_since();

    let second = gather_remembering(&repo, &mut bases, &mut readings);

    assert_eq!(second, first, "the kept counts are not the counts");
    assert_eq!(reads_since(), vec!["rev-parse", "ls-files"]);
}

#[test]
fn an_edit_nobody_has_added_is_read_again_and_the_commits_are_not_counted_again() {
    // The edit moves no commit and leaves the index as it was: what a gather
    // keeps cannot be keyed on those alone. Both diffs read the working tree
    // and are read again; the count of commits is about two commits that did
    // not move.
    if without_git("an_edit_nobody_has_added_is_read_again_and_the_commits_are_not_counted_again") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("edited");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = gathered_once(&scratch, &mut readings, &mut bases);

    write(
        &repo.join("tracked.txt"),
        "one\ntwo\nthree\nfour\nfive\nsix\n",
    );
    let second = gather_remembering(&repo, &mut bases, &mut readings);

    assert_eq!(
        reads_since(),
        vec!["rev-parse", "ls-files", "diff HEAD", "diff --merge-base"]
    );
    assert_eq!(second, gather(&repo, &mut Bases::default()));
    assert_eq!(second.diff.map(|diff| diff.lines_added), Some(2));
}

#[test]
fn a_commit_reads_everything_again() {
    if without_git("a_commit_reads_everything_again") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("committed");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = gathered_once(&scratch, &mut readings, &mut bases);

    git(&repo, &["commit", "--no-verify", "-am", "five"]);
    let second = gather_remembering(&repo, &mut bases, &mut readings);

    assert_eq!(
        reads_since(),
        vec![
            "rev-parse",
            "ls-files",
            "diff HEAD",
            "rev-list",
            "diff --merge-base"
        ]
    );
    assert_eq!(second, gather(&repo, &mut Bases::default()));
    assert_eq!(second.since_base.map(|since| since.commits), Some(2));
}

#[test]
fn a_head_that_moved_under_an_index_nobody_wrote_is_read_again() {
    // `reset --soft` moves `HEAD` and leaves the index and every file as they
    // were, so the stamp is the stamp it was — and the diff against `HEAD` is
    // not the diff it was.
    if without_git("a_head_that_moved_under_an_index_nobody_wrote_is_read_again") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("soft-reset");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = gathered_once(&scratch, &mut readings, &mut bases);

    git(&repo, &["reset", "--soft", "HEAD~1"]);
    let second = gather_remembering(&repo, &mut bases, &mut readings);

    assert_eq!(second, gather(&repo, &mut Bases::default()));
    assert_eq!(second.diff.map(|diff| diff.lines_added), Some(2));
    assert_eq!(second.since_base, None);
}

#[test]
fn a_base_that_moved_is_counted_from_again_and_the_tree_is_not_read_for_it() {
    // `main` moving on — a fetch, a merge somebody else made — changes what
    // the branch has done since it, and nothing about the working tree
    // against its own `HEAD`.
    if without_git("a_base_that_moved_is_counted_from_again_and_the_tree_is_not_read_for_it") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("base-moved");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = gathered_once(&scratch, &mut readings, &mut bases);

    let tree = git(&repo, &["rev-parse", "main^{tree}"]);
    let moved = git(
        &repo,
        &["commit-tree", &tree, "-p", "main", "-m", "main moves on"],
    );
    git(&repo, &["update-ref", "refs/heads/main", &moved]);
    let second = gather_remembering(&repo, &mut bases, &mut readings);

    assert_eq!(
        reads_since(),
        vec!["rev-parse", "ls-files", "rev-list", "diff --merge-base"]
    );
    assert_eq!(second, gather(&repo, &mut Bases::default()));
}

#[test]
fn a_work_tree_is_read_once_a_cycle_however_many_directories_are_in_it() {
    if without_git("a_work_tree_is_read_once_a_cycle_however_many_directories_are_in_it") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("one-tree");
    let (mut readings, mut bases) = (Readings::default(), Bases::default());
    let repo = repo_with_a_commit(&scratch, "repo");
    let deeper = repo.join("src");
    commit(&repo, "src/lib.rs", "fn main() {}\n", "src");
    write(&repo.join("tracked.txt"), "one\n");

    let top = gather_remembering(&repo, &mut bases, &mut readings);
    reads_since();
    let below = gather_remembering(&deeper, &mut bases, &mut readings);

    assert_eq!(below, top);
    assert_eq!(reads_since(), Vec::<String>::new());

    // And read again, once, the next cycle.
    readings.forget_unvisited();
    gather_remembering(&deeper, &mut bases, &mut readings);
    assert_eq!(reads_since(), vec!["rev-parse", "ls-files"]);
}

#[test]
fn a_stamp_taken_while_a_file_is_still_settling_vouches_for_nothing() {
    // A file written twice inside one tick of the file system's clock, the
    // same length both times, looks the same both times; a stamp taken in
    // that tick would keep the first count for the second file.
    if without_git("a_stamp_taken_while_a_file_is_still_settling_vouches_for_nothing") {
        return;
    }
    let scratch = ScratchDir::new("settling");
    let repo = repo_with_a_commit(&scratch, "repo");
    write(&repo.join("tracked.txt"), "uno\ndos\ntres\n");

    assert_eq!(stamp::stamp(&repo, &repo.join(".git")), None);

    settle_at_once();
    assert!(stamp::stamp(&repo, &repo.join(".git")).is_some());
}

#[test]
fn a_submodule_is_never_vouched_for() {
    // What git counts for one is its own `HEAD` and whether anything inside
    // it is dirty, and neither shows on the directory it is checked out in.
    if without_git("a_submodule_is_never_vouched_for") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("submodule-stamp");
    let repo = repo_with_a_commit(&scratch, "repo");
    let head = git(&repo, &["rev-parse", "HEAD"]);
    std::fs::create_dir_all(repo.join("vendored")).expect("the scratch directory is writable");
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},vendored"),
        ],
    );

    assert_eq!(stamp::stamp(&repo, &repo.join(".git")), None);
}

#[test]
fn a_sparse_checkout_is_stamped_by_the_files_it_has_checked_out() {
    // A sparse checkout is how a monorepo too big to check out is worked in,
    // and its index still lists every file the repository has. The diffs pass
    // over the ones left out; a stamp that went through them would cost the
    // whole tree every cycle, and listing a sparse index the plain way expands
    // it to every file first, which git itself calls slow.
    if without_git("a_sparse_checkout_is_stamped_by_the_files_it_has_checked_out") {
        return;
    }
    settle_at_once();
    let scratch = ScratchDir::new("sparse");
    let repo = repo_with_a_commit(&scratch, "repo");
    commit(&repo, "inside/kept.txt", "kept\n", "inside");
    commit(&repo, "outside/left.txt", "left out\n", "outside");
    git(
        &repo,
        &[
            "sparse-checkout",
            "set",
            "--cone",
            "--sparse-index",
            "inside",
        ],
    );
    assert!(
        !repo.join("outside").exists(),
        "the sparse checkout checked out what it was to leave out"
    );
    let (mut readings, mut bases) = (Readings::default(), Bases::default());

    gather_remembering(&repo, &mut bases, &mut readings);
    readings.forget_unvisited();
    let listings: Vec<String> = run::started_here()
        .into_iter()
        .filter(|read| read.starts_with("ls-files"))
        .collect();
    assert_eq!(listings, ["ls-files -z -t --sparse"]);

    gather_remembering(&repo, &mut bases, &mut readings);
    readings.forget_unvisited();
    assert_eq!(reads_since(), vec!["rev-parse", "ls-files"]);

    // What is checked out is still watched.
    write(&repo.join("inside/kept.txt"), "kept, and edited\n");
    let edited = gather_remembering(&repo, &mut bases, &mut readings);
    assert_eq!(reads_since(), vec!["rev-parse", "ls-files", "diff HEAD"]);
    assert_eq!(edited, gather(&repo, &mut Bases::default()));
    assert_eq!(edited.diff.map(|diff| diff.lines_added), Some(1));
}

#[test]
fn what_a_sparse_checkout_leaves_out_is_not_stamped() {
    // `S` is skip-worktree, which the diffs never look at — a file, or with
    // `--sparse` a whole directory a sparse index keeps as one entry. `H` is
    // an ordinary file and `M` one in the middle of a merge, both read.
    let listed = b"H kept.txt\0S left/out.txt\0S left/\0M both.txt\0";
    assert_eq!(
        stamp::checked_out(listed),
        Some(vec![&b"kept.txt"[..], &b"both.txt"[..]])
    );
    // Untagged is not the listing asked for, and vouches for nothing.
    assert_eq!(stamp::checked_out(b"kept.txt\0"), None);
}

// --- what a repository's configuration may run ---------------------------------
//
// A read is git run on nobody's say-so: the row's counts, every fifteen seconds
// in every directory a pane is in, and the menus as they open. A repository
// somebody unpacked brings its own `.git/config`, and some of its settings name
// a program for git to run. Each case first has plain git run that program,
// here and on this git, because a git that never asks would pass the rest.
//
// A partial clone's fetch is the exception, and the last three cases are its
// two sides: the row's counts go without, and the reads a person opens
// something for still fetch, because without it they would show nothing.

/// A program at `<scratch>/<name>` that leaves `<scratch>/<name>.ran` behind
/// when it runs and then fails, so that git carries on without it. The
/// program's path, for a setting to name, and the file's.
#[cfg(unix)]
fn tripwire(scratch: &ScratchDir, name: &str) -> (String, PathBuf) {
    marked(scratch, name, "exit 1")
}

/// A program like [`tripwire`]'s that then does the job: hands on to the
/// real `git upload-pack`, so that a fetch it is named for goes through.
#[cfg(unix)]
fn relay(scratch: &ScratchDir, name: &str) -> (String, PathBuf) {
    marked(scratch, name, "exec git upload-pack \"$@\"")
}

/// A shell program at `<scratch>/<name>` that leaves `<scratch>/<name>.ran`
/// behind and then runs `then`.
#[cfg(unix)]
fn marked(scratch: &ScratchDir, name: &str, then: &str) -> (String, PathBuf) {
    let ran = scratch.path().join(format!("{name}.ran"));
    let program = scratch.path().join(name);
    crate::process::write_program(
        &program,
        &format!("#!/bin/sh\ntouch '{}'\n{then}\n", ran.display()),
    );
    (program.display().to_string(), ran)
}

/// A clone of `server`'s `branch` at `<scratch>/clone` that has only the
/// files its checkout needed: `--filter=blob:none`, a blobless clone, which
/// fetches any other the moment a command asks for it.
#[cfg(unix)]
fn blobless_clone(scratch: &ScratchDir, server: &Path, branch: &str) -> PathBuf {
    git(server, &["config", "uploadpack.allowFilter", "true"]);
    let url = format!("file://{}", server.display());
    git(
        scratch.path(),
        &[
            "clone",
            "--quiet",
            "--filter=blob:none",
            "--branch",
            branch,
            &url,
            "clone",
        ],
    );
    scratch.path().join("clone")
}

#[cfg(unix)]
#[test]
fn no_read_runs_the_fsmonitor_program_a_repository_names() {
    // `core.fsmonitor` is the program git asks what changed in the working
    // tree, and every read that looks at the working tree asks it.
    if without_git("no_read_runs_the_fsmonitor_program_a_repository_names") {
        return;
    }
    let scratch = ScratchDir::new("fsmonitor");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "notes.txt", "alpha\nbeta\n", "add notes");
    write(&repo.join("tracked.txt"), "uno\ntwo\nthree\n");
    let (program, ran) = tripwire(&scratch, "fsmonitor");
    git(&repo, &["config", "core.fsmonitor", &program]);

    git(&repo, &["status", "--porcelain"]);
    assert!(ran.exists(), "git never ran the fsmonitor program here");
    std::fs::remove_file(&ran).expect("the scratch directory is writable");

    // The row's two counts, the worktree menu's listing and what removing a
    // checkout would lose, and the Changes column.
    let facts = gather(&repo, &mut Bases::default());
    worktree::list(&repo).expect("the worktrees are listed");
    worktree::local_work(&repo).expect("what is loose is read");
    worktree::loose_files(&repo).expect("the loose files are read");
    changes::overview(&repo).expect("the changes are read");

    assert!(
        facts.diff.is_some() && facts.since_base.is_some(),
        "a count was not read, so it may never have asked: {facts:?}"
    );
    assert!(
        !ran.exists(),
        "a read ran the program the repository names as its fsmonitor"
    );
}

#[cfg(unix)]
#[test]
fn no_read_runs_the_signature_program_a_repository_names() {
    // `log.showSignature` has `log` hand every signed commit it shows to the
    // program `gpg.program` names, and the worktree menu proves a branch has
    // landed by reading the base's history with `log` as it opens.
    if without_git("no_read_runs_the_signature_program_a_repository_names") {
        return;
    }
    let scratch = ScratchDir::new("signature");
    let repo = repo_with_a_commit(&scratch, "repo");
    git(&repo, &["switch", "-c", "agent/task"]);
    commit(&repo, "notes.txt", "alpha\nbeta\n", "add notes");
    git(&repo, &["switch", "main"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    // The base's newest commit again, signed. There is no key here to sign
    // with and no need of one: a signature is a header, and git hands any
    // commit that carries one to the program to check.
    let unsigned = git(&repo, &["cat-file", "commit", "HEAD"]);
    let (headers, message) = unsigned.split_once("\n\n").expect("a commit has a message");
    let signed = scratch.path().join("signed-commit");
    write(
        &signed,
        &format!(
            "{headers}\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n AAAA\n -----END PGP SIGNATURE-----\n\n{message}\n"
        ),
    );
    let signed = git(
        &repo,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            &signed.display().to_string(),
        ],
    );
    git(&repo, &["update-ref", "refs/heads/main", &signed]);
    let (program, ran) = tripwire(&scratch, "gpg");
    git(&repo, &["config", "log.showSignature", "true"]);
    git(&repo, &["config", "gpg.program", &program]);

    git(&repo, &["log", "-1", "--format=%H", "main"]);
    assert!(ran.exists(), "git never ran the signature program here");
    std::fs::remove_file(&ran).expect("the scratch directory is writable");

    let landed = merged::merged(&repo, "refs/heads/main", &["agent/task".to_owned()]);

    assert!(landed.is_empty(), "the branch has not landed: {landed:?}");
    assert!(
        !ran.exists(),
        "a read ran the program the repository names to check signatures"
    );
}

#[cfg(unix)]
#[test]
fn no_row_count_fetches_what_a_partial_clone_lacks() {
    // A partial clone fetches an object it was cloned without the moment a
    // command needs one, from the remote its configuration names and by the
    // means it names: `remote.<name>.uploadpack` here, `core.sshCommand` or a
    // credential helper as easily. The count since the base needs the files
    // as they were where the branch left it, which a clone of the branch
    // never had.
    if without_git("no_row_count_fetches_what_a_partial_clone_lacks") {
        return;
    }
    let scratch = ScratchDir::new("partial");
    let server = repo_with_a_commit(&scratch, "server");
    commit(
        &server,
        "tracked.txt",
        "one\ntwo\nthree\nfour\n",
        "add four",
    );
    git(&server, &["switch", "-c", "agent/task"]);
    commit(&server, "tracked.txt", "uno\ntwo\nthree\nfour\n", "uno");
    let clone = blobless_clone(&scratch, &server, "agent/task");
    let (program, ran) = tripwire(&scratch, "upload-pack");
    git(&clone, &["config", "remote.origin.uploadpack", &program]);
    let base = "refs/remotes/origin/main";
    let count = ["diff", "--shortstat", "--merge-base", base, "--"];

    // Fails, since the program it runs to fetch with does.
    git_output(&clone, &count);
    assert!(ran.exists(), "git never fetched here");
    std::fs::remove_file(&ran).expect("the scratch directory is writable");
    // A git from before the fix that taught it the variable — 2.44.0 and
    // 2.45.0 among them — fetches whatever it says, and so would the row.
    git_output_with(&clone, &count, &[("GIT_NO_LAZY_FETCH", "1")]);
    if ran.exists() {
        eprintln!(
            "skipping no_row_count_fetches_what_a_partial_clone_lacks: \
             this git fetches whatever GIT_NO_LAZY_FETCH says"
        );
        return;
    }

    let since = since_base_blocking(&clone, base);

    assert!(
        !ran.exists(),
        "the row's count fetched from the remote the repository names"
    );
    // What not fetching costs: no count, which a row already reads as a
    // number it does not have.
    assert_eq!(since, None);
}

#[cfg(unix)]
#[test]
fn a_landed_proof_still_fetches_what_a_blobless_clone_lacks() {
    // The menu proves a squash landed by reading the patch of every commit
    // the base gained since the branch left it, and a blobless clone has the
    // files of none of them: `fetch` brings their commits and no files. A
    // read that went without would prove nothing in such a clone for as long
    // as the base had moved on, which is always.
    if without_git("a_landed_proof_still_fetches_what_a_blobless_clone_lacks") {
        return;
    }
    let scratch = ScratchDir::new("partial-proof");
    let server = repo_with_a_commit(&scratch, "server");
    commit(&server, "other.txt", "x\n", "add other");
    let clone = blobless_clone(&scratch, &server, "main");
    git(&clone, &["switch", "-c", "agent/task"]);
    commit(&clone, "tracked.txt", "one\ntwo\nthree\nfour\n", "add four");
    // On the forge, somebody else's change, and then the task's, squashed.
    commit(&server, "other.txt", "y\n", "change other");
    commit(
        &server,
        "tracked.txt",
        "one\ntwo\nthree\nfour\n",
        "add four (#1)",
    );
    let squash = git(&server, &["rev-parse", "HEAD"]);
    git(&clone, &["fetch", "--quiet"]);
    let (program, ran) = relay(&scratch, "upload-pack");
    git(&clone, &["config", "remote.origin.uploadpack", &program]);

    let landed = merged::merged(
        &clone,
        "refs/remotes/origin/main",
        &["agent/task".to_owned()],
    );

    assert_eq!(
        landed.get("agent/task").map(merged::Landed::proof),
        Some(&merged::Proof::Patch { commit: squash }),
        "a squash on a base that moved on is not proved in a blobless clone"
    );
    assert!(
        ran.exists(),
        "the proof found every file it read already here, so it fetched nothing"
    );
}

#[cfg(unix)]
#[test]
fn the_changes_column_still_fetches_what_a_blobless_clone_lacks() {
    // The column finds a rename by comparing the file that went with the one
    // that came, as the fork had the first — which a clone of the branch
    // never checked out. A read that went without would list nothing at all.
    if without_git("the_changes_column_still_fetches_what_a_blobless_clone_lacks") {
        return;
    }
    let scratch = ScratchDir::new("partial-changes");
    let server = repo_with_a_commit(&scratch, "server");
    let lines = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\n";
    commit(&server, "old.txt", lines, "add old");
    git(&server, &["switch", "-c", "agent/task"]);
    git(&server, &["mv", "old.txt", "new.txt"]);
    commit(
        &server,
        "new.txt",
        &lines.replace("kappa", "lambda"),
        "rename old to new",
    );
    git(&server, &["switch", "main"]);
    let clone = blobless_clone(&scratch, &server, "agent/task");
    let (program, ran) = relay(&scratch, "upload-pack");
    git(&clone, &["config", "remote.origin.uploadpack", &program]);

    let overview = changes::overview(&clone);

    let overview = overview.expect("the Changes column lists nothing in a blobless clone");
    assert_eq!(
        overview.files,
        vec![changes::FileChange {
            status: changes::Status::Renamed,
            path: PathBuf::from("new.txt"),
            from: Some(PathBuf::from("old.txt")),
        }]
    );
    assert!(
        ran.exists(),
        "the column found every file it read already here, so it fetched nothing"
    );
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
