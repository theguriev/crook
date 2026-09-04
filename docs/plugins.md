# Plugins: a plan

*A design, and now partly an implementation. It is written so that the decisions it rests on
can be argued with; what has been built is listed under "Where this stands" and everything
else below is still a proposal.*

## Where this stands

Kept up to date as phases land, so that a plan nobody re-reads does not quietly become a
description of something that was never built.

- **Phase 0 — the kernel: done.** `crates/crook_plugin` (identities, manifest, slots with
  cardinality, named actions, `Registration` guards, the audit), `app/src/plugin.rs` (the
  contribution and handler types, `Host`, `Plugin`, `load`), and `app/src/plugins/` with two
  plugins in the box: `crook/header`, which owns the `header.right` slot, and `crook/usage`.
- **Phase 1 — in progress.**
  - `Plugin::build` takes the workspace's `ViewContext`, so a plugin can own a model or a
    view. `crook/usage` owns the `UsageChip`; `Workspace` no longer knows it exists.
  - Named actions are reachable: `crook/usage/refresh` is registered by a plugin, bindable
    from `keymap.json` by its name, dispatched as `WorkspaceAction::Run`, and listed on the
    settings page's Keys page with whatever chord reaches it.
  - `crook/window` registers every one of Crook's own commands under a name, and owns the
    `window.overlay` slot — where anything that floats over the whole window goes.
  - `crook/palette` is the first plugin that is not an extraction: a command palette, built
    entirely out of what the host already knew. Its rows come from `Host::commands`, its
    chord from `Host::suggest_binding` (a suggestion — a person's own file and every
    built-in chord win over it), and its Escape and arrows from `Host::claim_surface`, which
    names an action rather than doing anything, so they end at `WorkspaceAction::Run` like
    every other key.
  - The settings pages come from a slot. `crook/settings` owns the rail; every page belongs
    to the plugin whose feature it configures — `crook/appearance`, `crook/shell`,
    `crook/usage`, `crook/keys`, `crook/about` — so disabling a plugin takes its page off the
    rail with the rest of it. `--settings <name>` now matches a page's title, so a plugin's
    page is as reachable as one of Crook's.
  - There is a **Plugins page**, and the switches on it work: `Host::enable` is the other
    half of `unload`, `Plugin::ready` is a second pass so a page can offer a switch per
    plugin, and `disabled_plugins` in `settings.json` is where the answer is kept. A plugin
    switched off is carried and not built — its `build` never runs, so it registers nothing
    and makes nothing.
  - Still to move: the worktree menu, the Themes panel, the Omarchy palettes.

## 0. What was asked for, and what it means

Four requirements, in the owner's words: plugins that ship in the box; a store like an app
store; plugins able to change *practically everything*; and community submission by ordinary
pull request, the repository becoming a monorepo of plugins with a list of public,
not-preinstalled ones. Then, added: the DeepSeek Harness approach — **everything is a plugin**.

Those pull in two directions, and the plan has to be honest about it. "Change practically
everything" and "a stranger's PR runs in everyone's terminal" cannot both be true of one
mechanism. A plugin that can change everything is running Rust inside the process; a plugin
from a store has to be sandboxed, or the store is `curl | sh` with a nicer icon. Every system
that tried to have both (Obsidian: full JS access + PR-listed plugins) ended with an incident and
a change of model.

So: **two tiers, one discipline.** Everything Crook itself does becomes a *native* plugin —
that is where "everything is a plugin" is literally true, because the core's own features are
built on the same API a plugin would use, and the API is therefore proven to be enough. The
*store* ships *sandboxed* plugins that describe rather than paint, react rather than sit in
the frame loop, and hold only the capabilities they were granted. The sandbox host is itself a
native plugin. The line between the tiers is a type, not a language: **does it need
`&mut PaintContext`?** If yes, it is native.

## 1. What DeepSeek Harness actually is, and what of it transfers

DeepSeek Harness (`dsh`, August 2026, MIT, TypeScript) is built on Cordis, the kernel of the
Koishi chatbot framework. Its "no privileged core" is real in one sense — the agent loop, the
LLM adapter, the sandbox, the tools and the web UI are all rows in an 85-line YAML — and false
in another: the launcher, the kernel, the YAML stack and the UI's *slot declarations* are
privileged, and critics note the privileged core has moved into the YAML rather than gone.

What it is made of:

| Cordis idea | What it is | In Rust |
| --- | --- | --- |
| **Effects** — every registration returns a disposer; unloading a plugin unwinds all of them | RAII bolted onto JavaScript | `Drop`. A registration returns a guard; dropping it unregisters. Free. |
| **Seams** — a service *definition* (abstract class owning `ctx.key`), *providers*, *consumers* that `inject` it | Dependency injection by string key | A trait, its impls, and consumers over `dyn Trait`. Idiomatic. |
| **Events** — `emit` / `bail` / `serial` / `waterfall`, waterfalls being middleware chains where a listener that forgets `next()` silently short-circuits | Async promise chains per event | Typed, synchronous on the frame path; async delivery only across the sandbox boundary. |
| **Slots** — UI regions an owner *declares* with a cardinality (`single`, `list`, `keyed`, `chain`) and others *contribute to* with `id`/`order`; only the declaring package renders the slot | The UI contribution mechanism | The same, verbatim. This is the part to copy. |
| **Settings by schema** — a plugin registers a schema and the settings UI renders the form | Schemastery | `serde` + a small schema enum → the settings page's `Category`/`Entry` tables. |
| **Theme as tokens** — a theme plugin overrides tokens with a disposer; features use aliases only | CSS variables | Crook's 21 theme roles already are this. |
| **Bundles / profiles** — named plugin sets, layered YAML with replace-not-merge | Config composition | A `DefaultPlugins` group in code, and enable/disable of *registrations* in `settings.json`. |
| **Reactive fibers** — `PENDING → LOADING → ACTIVE → FAILED`; a plugin waits, silently, for a service to appear; dependents dispose when it goes | Runtime dependency resolution | **Do not port.** Only meaningful when providers change at runtime, which in a terminal they do not. The evidence: a community test found 107 of 923 plugins silently stuck in `PENDING`, and one plugin with a bad regex blocked the whole process from starting. Rust's crate graph resolves dependencies at compile time; a plugin that fails to build fails *by name, at startup*, and the window opens without it. |

The verdict from the field, after ~190k stars and 900+ plugins in two weeks: the discipline is
admired (Armin Ronacher: "inspired to revisit some of our choices"; reviewers: swapping the
sandboxed shell touches no tool, replacing the loop leaves 88 packages untouched), and the
runtime is what hurts — silent pending, waterfall footguns, a 47k-token prompt from composition
where a simpler harness spends 4.5k, no permission model, and compatibility breaks across
plugins within weeks of launch.

**Adopt dsh's discipline on Bevy's mechanism.** Bevy is the Rust precedent: `trait Plugin {
fn build(&self, app: &mut App) }`, `DefaultPlugins`, and the renderer, the window and the
input system are plugins on the same API — which is exactly what "everything is a plugin" has to
mean in a language with no runtime loading. Bevy also shows the limit honestly: plugins are not
removed after build and not hot-swapped. Enable/disable is of what a plugin *registered*, not
of its code.

## 2. Where Crook is today

The inventory (the full one is in the research this plan was written from) reduces to one
fact: **the models are open and the view is closed.**

Three models — `UsageModel`, `GitModel`, `TerminalModel` — are self-contained, run off-thread,
and reach the view only through `observe`/`subscribe`. They are the template for a plugin's
back half. The view is one 3,275-line `Workspace` struct, two views in the whole application
(`Workspace` and `UsageChip`), and every surface — header, strip, panel, body, settings page,
Themes panel, both menus, the hover card — is a free function over `&Workspace`. Its state is
that struct's fields; its vocabulary is one `WorkspaceAction` enum, `Copy`, compared by value in
a dozen places, with a sub-enum "one variant per control" for each surface.

What that costs a plugin today, concretely:

- **No slot exists anywhere.** The header says it in its own doc: "There is one right-hand
  item and its slot is hard-coded." `RowFacts` and `Chips` are structs with fixed fields, not
  lists. Adding a chip is a field, a `TabOptions` toggle, a `MenuState` handle and an arm in two
  renderers.
- **No event exists for most things.** `Workspace` has `type Event = ()`. Tab lifecycle emits
  nothing. Theme changes, settings changes, window resize: nothing. Command start/finish (OSC
  133) is a state machine read per frame — there is no `CommandFinished` anywhere. The one real
  pipeline is `TerminalUpdate` (title, cwd, bell, close, clipboard, completions), and its single
  subscriber is `Workspace` through a private handle.
- **No notification primitive, no command palette, no general tooltip.** The bell becomes an
  amber dot, and that is the whole attention model.
- **Keybindings name one of thirteen built-in actions.** The keymap file is "one file, one
  table, one rule"; a plugin action cannot be named in it.
- **`AgentStatus::Running` and `Failed` are never set at runtime.** There is no agent runtime
  behind the product's noun. This is the largest empty seam in the tree, and the first thing a
  real plugin ecosystem will want to fill.
- **The build rule, as enforced.** "No `build.rs` anywhere in the workspace" is our own rule;
  the dependency graph already contains `cc` (through `ring`, which rustls needs, and through
  the wayland/x11 crates) and `pkg-config` (x11/wayland). The invariant that is actually held is
  *no build script of our own and no system library required*. That decides which sandbox
  runtimes are admissible (§4).

And what is already plugin-shaped, ranked by how little core it touches: themes (a file in a
directory *is* the plugin — done); shell completion (a request file, a bound key, an answer
file, an OSC carrying a serial — a subprocess protocol that already works across three shells);
the shell-integration snippets; the usage chip (a model with its own poll chain, a view with its
own action, a header slot, a settings switch — the template for a feature plugin); the worktree
menu. Never plugins: the block machinery (one address space shared by marks, selection, copy and
paint), the composer and `input_keys::route` (its five rules are what makes `ctrl-c` safe), the
glyph atlas and the pipelines.

## 3. The kernel

A new crate, `crates/crook_plugin` (MIT), that the app depends on and that every plugin
depends on. It contains no behaviour — it is vocabulary and registries.

```rust
pub trait Plugin: 'static {
    /// The manifest, which is data the store and the settings page read without
    /// running anything.
    fn manifest(&self) -> &'static Manifest;
    /// Registers everything this plugin contributes. Every registration returns a
    /// guard; the plugin keeps them, and dropping the plugin unwinds all of them.
    fn build(&mut self, host: &mut Host) -> Result<(), BuildError>;
}
```

`Host` is what `App`/`Workspace` expose, and it is the whole of the API — nothing reaches a
plugin any other way:

**Registries (each `register` returns a guard):**

- `host.slots` — declare a slot (owner) or contribute to one (contributor). Slot ids are
  dotted strings owned by the declaring plugin: `header.right`, `tab.row.chips`,
  `tab.row.status`, `tab.menu.entries`, `block.footer`, `pane.content`, `settings.section`,
  `settings.page`, `palette.commands`, `overlay.layers`, `theme.tokens`. Cardinality is
  declared once, by the owner: `single` (highest priority renders), `list` (ordered by
  `order`, then registration), `keyed` (owner dispatches on a key), `chain` (each contributor
  supplies a pure `select`; first non-`None` wins). A contribution is an `Element` builder for
  native plugins and a declarative tree for sandboxed ones (§4).
- `host.actions` — register a named action (`owner/name`) with a handler. Named actions are
  what the keymap file, the palette and other plugins address; they replace "add a variant to
  `WorkspaceAction`". The thirteen built-in bindings become named actions of the core plugins.
- `host.settings` — register a schema under `plugins.<id>` in `settings.json`; the settings
  page renders it. The file already keeps unknown keys, so this is the one place a plugin can
  persist today; the schema is what makes it typed and visible.
- `host.keymap` — default chords for the plugin's actions; the user's file wins, as it does now.
- `host.themes` — a theme pack (files), or a token override with a disposer.
- `host.panes` — a new pane content type. `PaneContent` becomes `Agent | Settings | Plugin(id)`
  with a trait behind the third arm; the settings pane is the worked example of everything that
  arm costs (an `Option` on `session`/`status`, a branch in two row renderers, the singleton
  rule, exclusion from restore) — and is what the trait has to cover.

**Events (typed; synchronous for native, delivered for sandboxed):**

`tab.{opened,closed,focused,split}`, `pane.{opened,closed,focused,cwd,title,bell,exited}`,
`command.{started,finished}` (new: needs a `TerminalEvent` variant in `crook_terminal` and a
`TerminalUpdate` variant in the app, fed by the OSC 133 state machine that already exists),
`agent.status`, `usage.reading`, `git.facts`, `theme.changed`, `settings.changed`,
`window.resized`, `timer`. Three dispatch modes, not four: `emit` (broadcast), `bail` (first
`Some` wins — for `chain` slots and for "who handles this URL"), `serial` (ordered, each may
stop). No waterfall: a middleware chain where forgetting to call `next()` silently swallows the
event is the one Cordis feature the field agrees is a trap. Where a plugin genuinely needs to
*veto* — a keystroke, a paste, a command about to be sent — the event is a `bail` with an
explicit `Veto` return, and vetoes are monotonic: a listener may deny, never widen.

**Services (traits, provided by core plugins, consumed by anyone):**

`Shell` (send text, send keys, read a block, wait for output), `Tabs` (open in a directory —
`open_tab_in` exists — split, focus, close, select), `Settings`, `Theme`, `Git` (facts, worktrees),
`Process` (host-mediated spawn with timeout, through `process::command`), `Http` (the existing
ureq agent), `Notify` (new: a toast and an OS notification — the missing attention primitive),
`Clipboard`, `Log` (per-plugin file), `Storage` (per-plugin directory).

**Failure policy.** The house rule is written in `settings.rs`: nothing here can cost a person
their window. It extends to plugins verbatim. A plugin whose `build` fails is skipped by name
with one log line and the window opens. A contribution the host cannot build (a bad tree, a
panic in an element, an event handler that trapped) is dropped, logged, and counted; after N
failures the plugin **disables itself** and the settings page says why. Plugin code never runs
on the foreground executor except to build elements; anything else goes to the pool with a
deadline. `PARKED_WORKERS` becomes a budget a plugin declares (`parks: 1`) rather than a
constant a test guards.

## 4. The two tiers

### Tier 1 — native

A crate implementing `Plugin`, compiled into the binary, listed in `DefaultPlugins`. This is
where every feature Crook has today ends up, and where anything that touches `Scene`,
`Element`, the PTY read loop, the emulator, the renderer, windowing, focus routing or the entity
core lives.

**Native is not a store tier. Native is the product.** What is compiled in is Crook; there is
no "in the binary but disabled" — a release binary carries `DefaultPlugins` and nothing else,
because a disabled plugin's bytes in every user's binary is exactly the growth a plugin system
must not cause. A community contribution that genuinely needs native access is a pull request
to the core, reviewed as core, and it either becomes part of the product — enabled, like any
feature — or it does not ship. The rule "add a slot, never widen the vocabulary" (§7) exists so
that such requests are rare: nearly everything that looks native is a missing slot or a missing
host query.

Each native plugin sits behind a cargo feature, so somebody building from source can build a
Crook without the Themes panel or the usage chip. The shipped binary is the default set, whole.

This tier has no sandbox and needs none: it is the codebase, reviewed like the codebase.

### Tier 2 — sandboxed

A `.wasm` module, loaded at runtime by the wasm host (itself a Tier-1 plugin), talking to the
host through a Crook-owned ABI, holding the capabilities it declared and was granted.

**Runtime: `wasmi`.** Pure Rust, no `build.rs` beyond cfg emission, MIT, five dependencies,
core Wasm + WASI, an interpreter — which is enough: a plugin reacts to events and returns
trees; it is never in the frame loop. Zellij moved to it from wasmtime for exactly Crook's
reasons (binary size, portability, no compile cache, Windows). Typst runs its plugins on it.
The cost is honest: **no Component Model, no WIT.** The host↔plugin contract is bytes —
`postcard`-encoded `serde` types defined once in `crook_plugin_api`, versioned by an integer —
and its evolution is ours to manage. The alternative, `wasmtime` + WIT (Zed's route), buys typed
generated bindings at the price of a `cc`-compiled `helpers.c` and Cranelift's ISLE code
generation in the build, +6–10 MB of binary, minutes of compile, and a toolchain floor above the
current pin. `ring` already sets the precedent that a `cc`-compiled leaf is tolerable; the ABI
is designed so the same modules run on either engine, and the engine is swapped only if measured
performance demands it. **This is decision 1 (§7).**

**The plugin describes; the host paints.** A sandboxed plugin never sees an `Element`. It
returns a *tree* — a small, closed vocabulary: `column`, `row`, `text`, `icon` (from the
generated Lucide set), `chip`, `button`, `field`, `switch`, `divider`, `spacer`, with alignment,
padding and a colour *role* (never a colour) — and the host builds real elements from it into
the slot the plugin contributed to. This is Raycast's render tree and dsh's presentation
components, and it is the only sandboxed-by-construction UI model any surveyed system has. It
also settles what "change practically everything" means for a stranger's plugin: every slot in
§3, every event, every query, every command — but never the scene. The way to make more of
the application changeable is to add slots and queries, not to widen the vocabulary.

**Capabilities.** Declared in the manifest, granted per plugin-and-version at install in a
host-drawn dialog, stored in `settings.json`, re-prompted on escalation, enforced at the host
API rather than by trusting the sandbox alone. Zellij's fourteen are the starting list, adapted:
`read.output`, `read.selection`, `write.input` (types into a shell — the one that needs the
strongest warning), `run.commands` (with the exact argv shown at install), `net` (host
patterns), `fs` (paths), `clipboard`, `notify`, `tabs`, `settings.read`, `settings.write`,
`theme`, and two that no other terminal has had to name because Crook's unit is an agent:
`agent.read` (a block's text, an agent's status) and `agent.spend` (start work that costs the
session budget). A plugin with no capabilities can contribute UI and react to lifecycle events
and nothing else — which is most plugins.

**Isolation.** Fuel-metered execution (wasmi has it) with a per-call deadline; a trap or an
exhausted budget drops that contribution and counts toward self-disable; memory capped per
instance; one instance per plugin, on the pool, never on the foreground; results come home
through `ctx.spawn` like every other background answer.

### A third transport, later

herdr's substrate — a manifest, a socket speaking newline-delimited JSON, a CLI that doubles as
the SDK and as the agent-facing skill — is the right shape for *out-of-process* plugins: dev
tools, wrappers around agent CLIs, anything untrusted-but-yours, and above all **an agent in a
pane driving the terminal it runs in.** It is the same protocol as the wasm ABI over a different
transport, with no sandbox and therefore not the store's default. It comes after the store, as a
Tier-1 plugin called `host-process`, and it should copy herdr's details wholesale: the dot-named
event envelope, per-plugin config and state directories, `min_version`, the install preview of
every command a plugin will run, and per-command timeouts and budgets — the two things herdr
lacks and its users complain about.

## 5. The manifest

`plugin.toml`, in the plugin's directory, `deny_unknown_fields` (herdr silently ignores unknown
tables, and its community writes keybinding sections that do nothing):

```toml
schema = 1
id = "eugen/usage-chip"          # owner/name; owner is the GitHub account of the directory
name = "Usage chip"
version = "0.3.0"                # semver
api = ">=1.0, <2"                # the crook_plugin_api version range this was built against
license = "MIT"                  # SPDX, from a short accepted list; CI rejects anything else
repository = "https://github.com/…"
description = "How much of the session budget is spent, in the header."
tier = "wasm"                    # "native" | "wasm" | "process"
platforms = ["linux", "macos", "windows"]

[capabilities]
net = ["api.anthropic.com/*"]
settings = ["read", "write"]
parks = 1                        # background workers this plugin will hold on a timer

[contributions]                  # everything that is data and needs no code to be read
keymap = { "eugen/usage-chip/refresh" = "cmd-shift-u" }
settings = "settings.schema.json"
themes = ["themes/*.yaml"]
```

`id` is `owner/name` and the owner is the directory's CODEOWNER — Raycast's rule, enforceable
by GitHub. Built-ins carry `builtin = true` in the index, not in the manifest.

## 6. The store

**What the store distributes: `.wasm`, and only `.wasm`.** An installed plugin lives in
`<data>/crook/plugins/<id>/<version>/plugin.wasm`; disabling it stops loading it; uninstalling
deletes the directory. The binary never contains a store plugin, enabled or not, and does not
grow with the ecosystem. What it does carry, once: the `wasmi` runtime (about a megabyte, to be
measured in Phase 2; `wasmtime` would be six to ten) and the built-in plugins, embedded with
`include_bytes!` — a Rust plugin compiled to wasm with `opt-level = "z"`, `lto` and `wasm-opt`
is 50–400 KB, so the whole default set is a few megabytes on top of today's 19.4 MB release
binary. Disabling a built-in does not remove its bytes; they are the box. The alternative — not
embedding, and installing built-ins on first run the way Zed's `auto_install_extensions` does —
is rejected: a terminal that has none of its own features on a first run without a network is
not a terminal. How much of the box is "built-in plugin" rather than "core" is the one knob on
binary size, and it is a knob, not a doctrine.

**Sources.** `plugins/<id>/` directories in this repository — Raycast's monorepo, not Zed's
submodule list and not Obsidian's JSON list. **Not members of Crook's cargo workspace**: every
plugin directory is a workspace of its own, built only by the store's CI, so that cloning Crook
and running `cargo test --workspace` never compiles the community's plugins. The decisive property while the API is young is
that *one PR can migrate every plugin when the API changes* (Raycast keeps a `migrations/`
directory for exactly this); a submodule index turns each API bump into a chase across N
repositories, and a JSON list reviews only the first version of a plugin, which is what broke
Obsidian. Whether the directory is *this* repository or a sibling `crook-plugins` one that pins
an API version is **decision 2 (§7)**.

**Submission.** One plugin per PR; three open PRs per author; the author must approve changes
to their directory (CODEOWNERS); a three-week response window before a PR is closed. CI, made
mandatory by branch protection, checks the manifest, the licence, the ownership, the
capabilities against a policy (a `write.input` plugin gets a human look, every time), and that
the PR contains **no binaries** — every artifact is built by CI from source with the pinned
toolchain, so a user never runs a byte a reviewer did not see built.

**Artifacts and index.** CI on `main` builds each Tier-2 plugin to `wasm32-wasip1`, writes a
`plugins/index.json` (id, versions, api range, sha256 per artifact, capabilities, `builtin`,
`yanked`, a global blocklist) and publishes index and archives to a GitHub release or Pages.
**No server exists.** The app fetches only the index and the archive it was asked for, through
the existing `ureq`/rustls agent, `ETag`-cached on disk, carrying no identifier of any kind —
README says no telemetry and this is where that promise is easiest to break by accident. Offline
shows the cache. Obsidian proves GitHub-as-CDN carries 2,000 plugins.

**Built-ins.** The same mechanism, dogfooded: default Tier-2 plugins are embedded in the binary
with `include_bytes!` (no build script needed) and listed in the index with `builtin: true`, so
the store shows them as installed, undeletable, disableable. Default Tier-1 plugins are simply
`DefaultPlugins`. Both kinds appear on one page.

**In the app.** A `Plugins` page in the settings rail — searchable, since the rail searches —
listing installed, available and built-in plugins with state, granted capabilities, version,
last error, and per-plugin log. Install shows the capability dialog first. Updates are checked on
demand and applied on click; auto-update is off by default (the architecture doc lists
autoupdate as absent by design, and a terminal that changes under you is worse than a stale
one). A `yanked` bit in the index is honoured retroactively: the app refuses to load, disables,
and explains. A `--dev-plugin <path>` flag loads a plugin from a directory and reloads it when
its files change, because Raycast's and Obsidian's stores exist because the first plugin took
ten minutes.

## 7. The decisions the owner has to make

1. **Bytes or WIT.** `wasmi` with a Crook-owned versioned bytes ABI keeps the build rule as
   written and is the recommendation; `wasmtime` + WIT buys typed bindings at the price of an
   explicit exception to `docs/architecture.md`'s "no `build.rs` anywhere" — an exception
   `ring` already quietly is. This is the only decision that touches an invariant.
2. **Where plugin sources live.** In this repository (one clone, one CI, API migrations in one
   PR; every plugin PR lands in the terminal's history and every core change runs every plugin's
   tests) or a sibling monorepo pinning an API version (Raycast's shape; Raycast is 20k commits
   and 288 open PRs, which is what the in-repo option looks like at scale). Recommendation: in
   this repository until the API is 1.0, then reassess.
3. **The UI contract.** Which slots exist and what the declarative vocabulary is. This decides
   whether "change practically everything" is honest for a store plugin. The list in §3 is a
   proposal; the rule for growing it is "add a slot, never widen the vocabulary".
4. **What "everything" excludes.** The proposal: render *passes* are pluggable (decorations,
   cursor, overlays, background, image protocols — as `overlay.layers`), rasterisation is not;
   the block machinery, the composer's keyboard rules and the emulator are the core. These are
   plugins in name only in dsh too — its session log is a service every composition boots.

## 8. The plan, in phases

Each phase ships on its own and leaves the application working. Nothing here is a rewrite; it
is extraction, one seam at a time, each seam proven by moving a feature that exists onto it.

**Phase 0 — the kernel.** `crates/crook_plugin`: `Plugin`, `Manifest`, `Host`, the registries
with guards, slots with cardinality, the event types, the service traits. `App` grows a plugin
list; `Workspace` grows the slot renderers (a slot is rendered where its surface is rendered
today — the free functions stay, they just consult a registry) and the named-action table that
`WorkspaceAction` dispatches into. `command.{started,finished}` events plumbed from OSC 133.
`Notify` implemented (toast + OS notification). No user-visible change except the notification.

**Phase 1 — the first native plugins, by extraction.** In this order, each because it proves
a seam: the **usage chip** (a model with a poll chain, a header slot, a settings section, a
network capability — the template); the **worktree menu** (`tab.menu.entries`, the `Git` and
`Tabs` services, background work with deadlines); the **Themes panel** (`settings.page`, a new
pane content type or a panel slot — decision to be made when it is reached); the **settings
pages** themselves (`settings.section` fed by schemas — the search already works over
`Category`/`Entry`); the **Omarchy palettes** as a theme pack. And the first *new* plugin: a
**command palette**, because a system whose primary noun is "action" needs one on day one —
herdr shipped without and its community wrote three. When Phase 1 ends, `DefaultPlugins` is
real and `Workspace` is a host.

**Phase 2 — the sandbox.** `plugins/host-wasm` on `wasmi`; `crook_plugin_api` with the
`postcard` message types and the declarative tree; capabilities declared, granted, enforced;
fuel and deadlines; self-disable. One built-in re-implemented as a Tier-2 plugin and embedded,
to dogfood the ABI end to end before any outsider touches it — the git-status chip or the usage
chip itself.

**Phase 3 — the store.** `plugins/` layout, `plugin.toml`, CI (build, checks, index, publish),
the Plugins page, install/enable/disable/update/yank, the capability dialog, `--dev-plugin`, a
template repository, and the docs generated from `crook_plugin_api`.

**Phase 4 — the process transport.** `plugins/host-process`: the NDJSON socket, the CLI as
SDK, `--skill` output for an agent in a pane, supervised long-lived plugins with budgets.

**Phase 5 — the agent seam.** `AgentStatus::Running`/`Failed` finally written by something:
an `Agent` service that a plugin provides (Claude Code, Codex, a local model), with `agent.read`
and `agent.spend` as the capabilities that gate it. This is the seam the product's name
promises, and it is a plugin from the first day it exists.

## 9. What is deliberately not in the plan

- **Runtime loading of native code** (`dlopen`, `abi_stable`): no stable Rust ABI, and the core
  is `!Send` foreground contexts and generic typed `ViewContext<T>`s that cannot cross a
  dynamic boundary. Not viable, and not needed once native means "in the monorepo".
- **A scripting language** (Lua needs a C compiler; Rhai/Rune are pure Rust but unsandboxed):
  no isolation, and every unsandboxed in-process system surveyed ended with an incident or a
  maintainer saying it cannot be secured. If a user-scriptable configuration layer is ever
  wanted it is a separate feature, not the plugin runtime.
- **Extism**: wasmtime plus a `cbindgen` build script, for multi-language PDKs Crook does not
  need on day one.
- **Reactive dependency resolution, hot-swapping code, YAML composition**: dsh's runtime, and
  the source of its worst reports. Dependencies are the crate graph; failure is by name at
  startup; composition is code.
- **A server, telemetry, download counts, auto-update on by default**: each contradicts a
  promise the README makes.
- **Signing with hardware keys, provenance attestations, verified publishers**: the second
  threat (a compromised CI), not the first (a malicious PR); a CI-signed index with a sha256 per
  artifact covers the first.

## 10. Sources

herdr (installed here; `herdr plugin --help`, https://herdr.dev/docs/), DeepSeek Harness
(https://github.com/deepseek-ai/deepseek-harness and its `docs/`), Cordis, Bevy's `Plugin`,
Zed's extension system and `zed-industries/extensions`, Raycast's `raycast/extensions`,
Obsidian's community plugins and the 2026 PHANTOMPULSE incident, VS Code's built-in
`extensions/` and contribution points, Zellij's move to `wasmi` and its permission list, Typst's
plugin runtime, the `wasmi`/`wasmtime`/`extism`/`mlua`/`rhai` crate manifests. The research
reports behind this document are the workflow transcripts in the session that wrote it.
