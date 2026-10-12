//! `script/release`, which cuts a release only from a commit Crook CI passed.
//!
//! v0.1.20 and v0.1.21 were each tagged minutes after Crook CI failed, because
//! nothing between a merge and a tag asked it. The script now asks GitHub,
//! through `gh`, before it changes a file. A release is cut now and then on
//! one machine and `cargo test` runs on every change, so these run the script
//! here: in a scratch repository on `main` one commit past v0.1.0, under a
//! `PATH` that holds the few tools it uses and three stand-ins. `gh` answers
//! from files the test writes, one commit at a time, so a script that asked
//! about any commit but the one it releases would find no run at all. `npx`
//! prints a changelog section without fetching a generator. `cargo` ends the
//! run at the step after the version is set — the first file a release
//! changes — so a run that reaches it is one the check let through, and a run
//! the check stopped has to have left that file as it was.
//!
//! Unix only: the script is `sh`, and so is everything it runs here.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use crook::process::command;

/// The tools the script and `script/commit-lint` run, other than the
/// stand-ins. Each is reached through a wrapper in the scratch `bin`, so the
/// `PATH` the script sees holds nothing else — which is how a machine without
/// `gh` is made out of one that has it.
const TOOLS: [&str; 10] = [
    "cat", "dirname", "git", "grep", "head", "rm", "sed", "sort", "tail", "tr",
];

/// The release every case asks for, the one after the scratch v0.1.0.
const VERSION: &str = "0.1.1";

/// What the `cargo` stand-in exits with, so a run it ended is told apart from
/// one the script ended.
const CARGO_STAND_IN: i32 = 3;

/// A Crook CI run's page, which a refusal names so it can be opened.
const RUN_URL: &str = "https://github.com/theguriev/crook/actions/runs/38216481742";

/// The `gh` stand-in. It is logged in while `logged-in` exists beside it, and
/// to `run list` it prints `runs/<the --commit it was given>` when that file
/// exists: the lines the real `gh` prints for the script's `--jq`. Every call
/// is appended to `gh.log`.
const GH: &str = r#"#!/bin/sh
here=${0%/*}
echo "$*" >> "$here/gh.log"
case "$1 $2" in
  "auth status")
    [ -f "$here/logged-in" ]
    exit $?
    ;;
  "run list")
    commit=
    while [ $# -gt 0 ]; do
      [ "$1" = --commit ] && commit=$2
      shift
    done
    [ ! -f "$here/runs/$commit" ] || cat "$here/runs/$commit"
    exit 0
    ;;
esac
echo "gh stand-in: nothing to say to $*" >&2
exit 64
"#;

/// The `npx` stand-in: a dry run's changelog section, without changelogen.
const NPX: &str = "#!/bin/sh\necho 'npx stand-in: the changelog section'\n";

/// The `cargo` stand-in, which the script runs right after it sets the version.
const CARGO: &str = "#!/bin/sh\necho \"cargo stand-in: the release got to cargo $*\" >&2\nexit 3\n";

/// What one run of the script did.
struct Verdict {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl From<Output> for Verdict {
    fn from(output: Output) -> Self {
        Verdict {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// A scratch repository holding the release scripts, on `main` one commit past
/// a v0.1.0 tag, beside the `bin` directory that is the whole of its `PATH`.
/// Removed when it drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "crook-release-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = Self { root };
        let (repo, bin) = (scratch.repo(), scratch.bin());
        for dir in [repo.join("script"), bin.join("runs")] {
            std::fs::create_dir_all(dir).expect("the system temp directory is writable");
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the workspace root");
        // Copies, not links: the script finds the repository it releases from
        // where it sits, and commit-lint finds changelog.config.json the same way.
        for file in [
            "script/release",
            "script/commit-lint",
            "changelog.config.json",
        ] {
            std::fs::copy(workspace.join(file), repo.join(file))
                .expect("the release scripts can be copied");
        }
        write(
            &repo.join("Cargo.toml"),
            "[workspace.package]\nversion = \"0.1.0\"\n",
        );
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["add", "."]);
        git(
            &repo,
            &["commit", "--quiet", "-m", "chore(release): v0.1.0"],
        );
        git(&repo, &["tag", "v0.1.0"]);
        write(&repo.join("tabs.txt"), "one more tab\n");
        git(&repo, &["add", "tabs.txt"]);
        git(
            &repo,
            &["commit", "--quiet", "-m", "feat(tabs): one more tab"],
        );

        for tool in TOOLS {
            let wrapper = format!("#!/bin/sh\nexec '{}' \"$@\"\n", on_path(tool).display());
            executable(&bin.join(tool), &wrapper);
        }
        executable(&bin.join("gh"), GH);
        executable(&bin.join("npx"), NPX);
        executable(&bin.join("cargo"), CARGO);
        scratch
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// The full hash of a commit in the scratch repository.
    fn commit(&self, revision: &str) -> String {
        git(&self.repo(), &["rev-parse", revision])
    }

    /// Logs the `gh` stand-in in to GitHub.
    fn log_in(&self) {
        write(&self.bin().join("logged-in"), "");
    }

    /// Gives `commit` a newest Crook CI run that is `state`, written the way
    /// the script's `--jq` writes one: `<status>/<conclusion>`.
    fn ci(&self, commit: &str, state: &str) {
        write(
            &self.bin().join("runs").join(commit),
            &format!("{state} {RUN_URL}\n"),
        );
    }

    /// Every question the script put to the `gh` stand-in, one per line.
    fn asked(&self) -> String {
        std::fs::read_to_string(self.bin().join("gh.log")).unwrap_or_default()
    }

    /// Runs `script/release 0.1.1 <args>` in the scratch repository.
    fn release(&self, args: &[&str]) -> Verdict {
        let nowhere = self.root.join("no-such-gitconfig");
        command("/bin/sh")
            .arg(self.repo().join("script/release"))
            .arg(VERSION)
            .args(args)
            .current_dir(self.repo())
            .env("PATH", self.bin())
            .env("GIT_CONFIG_GLOBAL", &nowhere)
            .env("GIT_CONFIG_SYSTEM", &nowhere)
            .stdin(Stdio::null())
            .output()
            .expect("sh runs the script")
            .into()
    }

    /// Asserts that `verdict` is a refusal that changed nothing — no version
    /// set, no commit, no tag — and that its reason names each of `reasons`.
    fn assert_refused(&self, verdict: &Verdict, reasons: &[&str]) {
        assert_eq!(
            verdict.code,
            Some(1),
            "{}{}",
            verdict.stdout,
            verdict.stderr
        );
        for reason in reasons {
            assert!(
                verdict.stderr.contains(reason),
                "{reason:?} is not in:\n{}",
                verdict.stderr
            );
        }
        assert!(
            !verdict.stdout.contains("Setting Cargo.toml"),
            "{}",
            verdict.stdout
        );
        let manifest =
            std::fs::read_to_string(self.repo().join("Cargo.toml")).expect("Cargo.toml is there");
        assert!(manifest.contains("version = \"0.1.0\""), "{manifest}");
        assert_eq!(git(&self.repo(), &["status", "--porcelain"]), "");
        assert_eq!(git(&self.repo(), &["tag", "--list", "v0.1.1"]), "");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Asserts that `verdict`'s run got past every check to the first edit, where
/// the `cargo` stand-in ended it.
fn assert_released(verdict: &Verdict) {
    assert_eq!(
        verdict.code,
        Some(CARGO_STAND_IN),
        "{}{}",
        verdict.stdout,
        verdict.stderr
    );
    assert!(
        verdict.stdout.contains("Setting Cargo.toml to 0.1.1"),
        "{}",
        verdict.stdout
    );
}

/// Where `tool` is on this process's `PATH`.
fn on_path(tool: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(tool))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{tool} is not on PATH"))
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("the scratch directory is writable");
}

fn executable(path: &Path, contents: &str) {
    write(path, contents);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("a scratch file can be made runnable");
}

/// Runs git in `dir`, isolated from the machine's own configuration, and
/// returns its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let nowhere = dir.join("no-such-gitconfig");
    let output = command("git")
        .args(["-c", "init.defaultBranch=main"])
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &nowhere)
        .env("GIT_CONFIG_SYSTEM", &nowhere)
        .env("GIT_AUTHOR_NAME", "Crook Tests")
        .env("GIT_AUTHOR_EMAIL", "tests@crook.invalid")
        .env("GIT_COMMITTER_NAME", "Crook Tests")
        .env("GIT_COMMITTER_EMAIL", "tests@crook.invalid")
        .stdin(Stdio::null())
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn a_commit_crook_ci_passed_is_released() {
    let scratch = Scratch::new("passed");
    scratch.log_in();
    let head = scratch.commit("HEAD");
    scratch.ci(&head, "completed/success");

    let verdict = scratch.release(&[]);

    assert_released(&verdict);
    assert!(verdict.stdout.contains(RUN_URL), "{}", verdict.stdout);
    assert!(scratch.asked().contains(&head), "{}", scratch.asked());
}

#[test]
fn a_commit_whose_crook_ci_run_ended_any_other_way_is_refused_naming_the_run() {
    for conclusion in ["failure", "cancelled", "timed_out"] {
        let scratch = Scratch::new(conclusion);
        scratch.log_in();
        scratch.ci(&scratch.commit("HEAD"), &format!("completed/{conclusion}"));

        let verdict = scratch.release(&[]);

        scratch.assert_refused(&verdict, &[conclusion, RUN_URL]);
    }
}

#[test]
fn a_commit_crook_ci_is_still_running_on_is_refused_until_it_finishes() {
    // While a run is under way its conclusion is empty, and gh prints it so.
    for status in ["queued", "in_progress"] {
        let scratch = Scratch::new(status);
        scratch.log_in();
        scratch.ci(&scratch.commit("HEAD"), &format!("{status}/"));

        let verdict = scratch.release(&[]);

        scratch.assert_refused(&verdict, &[status, RUN_URL]);
    }
}

#[test]
fn a_commit_crook_ci_never_ran_on_is_refused_with_how_to_start_a_run() {
    // A green run on the commit before it vouches for none of what came after,
    // which is where main stands right after a merge.
    let scratch = Scratch::new("never-ran");
    scratch.log_in();
    scratch.ci(&scratch.commit("HEAD~1"), "completed/success");

    let verdict = scratch.release(&[]);

    scratch.assert_refused(&verdict, &["gh workflow run \"Crook CI\" --ref main"]);
}

#[test]
fn without_gh_or_a_login_to_github_the_release_is_refused() {
    let scratch = Scratch::new("logged-out");
    scratch.ci(&scratch.commit("HEAD"), "completed/success");

    let logged_out = scratch.release(&[]);
    scratch.assert_refused(&logged_out, &["gh auth login"]);

    std::fs::remove_file(scratch.bin().join("gh")).expect("the gh stand-in is there");
    let missing = scratch.release(&[]);
    scratch.assert_refused(&missing, &["https://cli.github.com"]);
}

#[test]
fn a_dry_run_says_what_crook_ci_said_and_goes_on() {
    let scratch = Scratch::new("dry-run");
    scratch.log_in();
    scratch.ci(&scratch.commit("HEAD"), "completed/failure");

    let verdict = scratch.release(&["--dry-run"]);

    assert_eq!(
        verdict.code,
        Some(0),
        "{}{}",
        verdict.stdout,
        verdict.stderr
    );
    for said in ["failure", RUN_URL, "npx stand-in: the changelog section"] {
        assert!(
            verdict.stdout.contains(said),
            "{said:?} is not in:\n{}",
            verdict.stdout
        );
    }
}

#[test]
fn no_ci_check_releases_without_asking_github() {
    // Logged out, with a failed run on the commit: the stand-in would refuse
    // on every count, so a release that gets past it was never put the question.
    let scratch = Scratch::new("no-ci-check");
    scratch.ci(&scratch.commit("HEAD"), "completed/failure");

    let verdict = scratch.release(&["--no-ci-check"]);

    assert_released(&verdict);
    assert_eq!(scratch.asked(), "");
}
