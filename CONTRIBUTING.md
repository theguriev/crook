# Contributing

## Before you push

**These five commands are the gate.** Not "run them and CI will confirm it" — CI does not run
by itself. `.github/workflows/ci.yml` is `workflow_dispatch` only, so nothing starts when you
push or open a pull request, and a green branch is one somebody made green here. Run them, in
this order:

```sh
export PATH="$HOME/.cargo/bin:$PATH"

cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::redundant_explicit_links" \
  cargo doc --locked --no-deps --workspace --document-private-items
```

Clippy warnings are errors, and so is a comment whose [`link`] points at an item that has
been renamed or removed — the comments link to the code they explain, and a link to nothing
is a comment that has quietly started lying about where to look. So is a target spelled out in
full when the text beside it already resolves there — ``[`Foo`](Foo)`` — a second copy of the
link that drifts from the first the moment either one moves. `.clippy.toml` also bans `std::process::Command`, because on
Windows it flashes a console window unless the spawner sets `CREATE_NO_WINDOW` — invisible on
macOS and Linux, and a shipping-blocker for a terminal. Use `crook::process::Command`.

There is a sixth check, which compiles the workspace with the feature set a shipped build uses:

```sh
./script/bundle --check-only
```

Run it if you touch a `Cargo.toml`, a feature gate, or anything under `#[cfg(...)]`.

## The workflow, when you want it

The same five checks run on macOS, Linux and Windows as `Crook CI`, and they run when they are
asked to:

```sh
gh workflow run "Crook CI" --ref <branch>
```

or the Run workflow button on the Actions tab. It is not on a push or a pull request on
purpose: the account this repository is under has no minutes, so an automatic trigger put a
red cross on every branch — including the ones that were merged — and a check that always
fails is one nobody reads. The three platforms are what it is still worth asking for, since
`crook::process::Command`, the path handling and the release feature set are the things four
local commands on one machine cannot cover.

If `cargo metadata --locked` fails, `Cargo.lock` is out of date with a manifest. Run
`cargo check` and commit the updated lockfile.

## Commit titles are what the release notes say

Every commit title is a [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/):

```
type(scope): what changed
```

The scope is optional and names the part of Crook — `tabs`, `find`, `palette`, `plugins`,
`api` for the plugin API crate. A `!` before the colon marks a change that breaks something,
and the body says what. The types are the keys under `types` in `changelog.config.json`:

| type | what it lists as |
| --- | --- |
| `feat` | Features |
| `fix` | Fixes |
| `perf` | Performance |
| `refactor` | Refactors |
| `docs` | Documentation |
| `test` | Tests |
| `build` | Build |
| `ci` | CI |
| `chore` | Chores |

The reason is mechanical. `script/release` writes the section for a version into
`CHANGELOG.md` from the commits since the previous tag, with
[changelogen](https://github.com/unjs/changelogen), and `release.yml` lifts that section into
the release page. The generator reads exactly this shape and drops every other title without
a word, so a title outside it is a change the release will not mention. The check runs in
three places so that it is caught where the title is written rather than on release day:

```sh
git config core.hooksPath script/hooks    # once per clone: refuses the commit locally
./script/commit-lint v0.1.7..HEAD         # a range, by hand
```

and `.github/workflows/commits.yml` on every pull request, over its commits and its title —
the title because a squash merge makes it the commit `main` gets.

The description after the colon is the same sentence it always was: what changed, in the
imperative, specific enough to mean something on a release page on its own.
`fix(find): count the open block's matches too` says more than `fix: find bug`.

## Cutting a release

```sh
./script/release 0.1.8            # bump, changelog, commit, tag; nothing pushed
./script/release 0.1.8 --dry-run  # print the section it would write and stop
./script/release 0.1.8 --push     # and push main with the tag, which starts release.yml
```

From a clean `main`, with Node installed for the generator. The script sets the workspace
`version` in `Cargo.toml`, refreshes `Cargo.lock`, prepends `## vX.Y.Z` to `CHANGELOG.md`,
commits the three as `chore(release): vX.Y.Z` and tags it. It refuses a range holding
titles the generator would drop; `--allow-untyped` is the way past that, and is needed exactly
once, for the release that spans the change of convention.

`script/release-notes vX.Y.Z` prints what a tag's release page will say: the section out of
`CHANGELOG.md`, or — for a tag made by hand, which has none — the same generator run over the
same range, with a line admitting that is what happened. `release.yml` publishes what it
prints, and every plugin repository cuts its releases with the same two scripts.

## Comments say why, never what

A comment that restates the code beneath it is noise that has to be maintained. Delete it.
Write the comment the code cannot express: the constraint, the bug it avoids, the alternative
that was rejected.

```rust
// Bad — the code already said this.
// Increment the retry count.
retries += 1;

// Good — this is not derivable from the line below it.
// wgpu reports Outdated for a resize that raced the frame; reconfiguring and
// retrying once is cheaper than dropping the frame, and it never loops.
retries += 1;
```

The same rule applies upward. Every public item gets a doc comment. Every module gets a `//!`
header explaining its role in the whole, not a restatement of its name.

## `ctx` goes last

Context parameters are named `ctx`, never `cx` or `context`, and they are the last parameter —
unless the function takes a closure, in which case the closure is last and `ctx` is
second-to-last.

```rust
fn close(&mut self, id: TabId, ctx: &mut ModelContext<Self>);

fn update<R>(&self, ctx: &mut AppContext, f: impl FnOnce(&mut T, &mut ModelContext<T>) -> R) -> R;
```

## Two smaller conventions

Do not annotate types the compiler infers, especially closure parameters: write
`|tab| tab.title()`, not `|tab: &Tab| -> &str`.

Do not leave `_`-prefixed unused parameters. Delete the parameter and fix the call sites; an
unused argument that every caller still passes is a lie about the interface.

## Processes go through `crook::process::command`

Never `std::process::Command::new`. On Windows every process a GUI application starts flashes a
console window of its own unless it is created with `CREATE_NO_WINDOW`, and a terminal starts a
lot of processes. `crook::process::command` sets the flag; everything else is identical. The
workspace's `.clippy.toml` bans `Command::new` outright and names the replacement, so the gate
catches a slip rather than a Windows user finding it — which is why `app/src/process.rs` is the
one file allowed to call the banned method, and it says so where it does.

## The window keeps no idle timer

Nothing ticks while nothing is happening. A poll, a refresh, a menu's animation, a plugin's
surface — each hangs off an event or a wake, never a clock that runs when the window is at
rest, so an idle window does no work and wakes no CPU. When a feature seems to need "check
every so often", the question to answer first is what event it is really waiting for.

## A disabled control has no handler

A control that cannot do anything right now — a stepper at the end of its range, a Reset with
nothing to reset, a button for a state the row is not in — is drawn de-emphasised and given no
click handler at all. Not a live control whose handler returns early: the absence of the
handler is what says "there is nothing to do here", and it is what keeps a dead button from
lighting under the pointer as though a press would do something. The `Option` an action builder
returns is `None` in exactly these cases, and the button reads it.
