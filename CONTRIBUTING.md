# Contributing

## Before you push

**These four commands are the gate.** Not "run them and CI will confirm it" — CI does not run
by itself. `.github/workflows/ci.yml` is `workflow_dispatch` only, so nothing starts when you
push or open a pull request, and a green branch is one somebody made green here. Run them, in
this order:

```sh
export PATH="$HOME/.cargo/bin:$PATH"

cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
```

Clippy warnings are errors. `.clippy.toml` also bans `std::process::Command`, because on
Windows it flashes a console window unless the spawner sets `CREATE_NO_WINDOW` — invisible on
macOS and Linux, and a shipping-blocker for a terminal. Use `crook::process::Command`.

There is a fifth check, which compiles the workspace with the feature set a shipped build uses:

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
