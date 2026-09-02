# Contributing

## Before you push

CI runs these four commands, in this order, on macOS, Linux and Windows. Run them locally and
you will not be surprised:

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

There is a fifth job, which compiles the workspace with the feature set a shipped build uses:

```sh
./script/bundle --check-only
```

Run it if you touch a `Cargo.toml`, a feature gate, or anything under `#[cfg(...)]`.

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
