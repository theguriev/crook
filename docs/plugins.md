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
  plugins in the box: `crook/header`, which owns the `header.right` slot, and `crook/usage`,
  which contributed the chip to it. That pair is where the split was first proved, and half of
  it is no longer in the binary at all — see Phase 2.
- **Phase 1 — in progress.**
  - `Plugin::build` takes the workspace's `ViewContext`, so a plugin can own a model or a
    view. `crook/usage` owned the `UsageModel` and the `UsageChip`, and `Workspace` stopped
    knowing that either existed. That seam was proved twice: once when the chip left
    `Workspace` for a plugin, and again when the plugin left the binary, which cost `Workspace`
    nothing because it had already forgotten the chip was there.
  - Named actions are reachable: a plugin registers one under its own name, `keybindings.json`
    binds it by that name, `WorkspaceAction::Run` dispatches it, and the settings page's
    Keyboard Shortcuts page lists it with whatever chord reaches it. The first two were
    `crook/usage/refresh` and `crook/usage/panel`, and `--usage-panel` reached a popover the
    workspace itself had no handle on — which is still the plainest demonstration of what a
    named action is for. Both left with the chip, and nothing in the core had to be put in
    their place: a sandboxed plugin registers its actions in the same table, prefixed with its
    own id, and a person binds one of them exactly as they bind `crook/window/close-window`.
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
    `crook/shortcuts`, `crook/about` — so disabling a plugin takes its page off the rail with
    the rest of it, and removing a plugin takes it off for good. The rail was five pages while
    `crook/usage` was in the box and is four without it, and not a line of `crook/settings`
    knew either number. `--settings <name>` matches a page's title, so a plugin's page is as
    reachable as one of Crook's.
  - The **Plugins page** is a list beside a card, which is VS Code's shape: a field and every
    plugin this Crook has — in the box or installed as a file — on the left, and on the right
    whatever the list has selected: what it is, where it came from, what it puts on screen,
    what it can be asked to do, what it has asked to be allowed, and the switch. It is a page
    that *draws itself* (`Host::add_settings_view`) rather than a column of settings rows,
    which is what a master–detail layout needs and what a `Vec<Category>` cannot describe.
  - The switches on it work: `Host::enable` is the other
    half of `unload`, `Plugin::ready` is a second pass so a page can offer a switch per
    plugin, and `disabled_plugins` in `settings.json` is where the answer is kept. A plugin
    switched off is carried and not built — its `build` never runs, so it registers nothing
    and makes nothing.
  - The sidebar's **sections** are a slot: `sidebar.section`, declared by `crook/window`,
    contributed to by `crook/settings` and `crook/plugins`. A section is a button at the foot
    of the panel plus what the window shows while it is chosen — the sidebar's body and the
    main area, built together because they are two views of one answer. The settings stopped
    being a pane and the plugins left the settings rail entirely.
  - The **tab row's leading mark is a slot**, and so is the badge on its corner:
    `crook/tabs` declares `tab.row.mark` and `tab.row.badge`, and the status disc the panel has
    always drawn is what the host puts there when nothing has taken them. These are the first
    slots that are drawn **more than once** — once per row — which is a different kind of slot
    and needed two things the others did not. A contribution is handed the *row*
    (`plugin::RowContribution`, a second registry beside the one `header.right` lives in,
    because a contribution that cannot be told which row it is on cannot answer differently for
    two of them). And it may **decline** a row by answering `None`, which is what lets a plugin
    mark the worktrees and leave everything else alone: declining means "as it was" rather than
    "empty", which is also why the disc is the host's answer to an empty slot rather than a
    contribution of its own competing at some order.

    **What a slot per row costs**, because it is worth writing down before somebody declares one
    per block: a sandboxed contribution to it is one guest call per row per frame, each with its
    own fuel budget. Seven tabs is seven calls where the header's slot is one, and the budget is
    per call rather than shared — so a plugin cannot be starved by the panel being long, and a
    panel cannot be slowed to a crawl by a plugin that spends its whole budget, because spending
    it is a trap and three traps in a row stop it being asked at all. The two plugins written
    against it answer in a table lookup and a boolean; a slot per *block* would be the same
    arithmetic against a list that is thousands long, and would need something this does not
    have.
  - Still to move: the worktree menu, the Themes panel, the Omarchy palettes.
  - `crook/tabs` owns `tab.menu.entries`, and a tab's secondary press opens a *place*
    rather than a feature. The menu it opens knows no entry by name: four of them are
    `crook/tabs`'s own — new group with tab, the two copies, close tab — and the fifth is
    `crook/worktrees`, which is the first half of moving that menu. The claim moved and the
    code did not: the worktree popup is still `workspace::tab_menu`, because that column
    holds a text field, a background git read and a two-step confirmation. What did move is
    what matters for the API — the menu is a list a stranger's plugin can put a row in, and
    switching `crook/worktrees` off leaves a tab's menu four entries long with nothing
    anywhere saying a fifth is missing. Entries are grouped by dividing `order` by a hundred,
    so a slot that hands its renderer a flat list still draws its bands, and no
    plugin can draw a seam across somebody else's group.
  - **A plugin can own a text field.** `host.claim_field` is the other half of
    `claim_surface`, and it closes the last gap between a plugin and a built-in: a plugin could
    already put a surface on screen and claim a keystroke, and still had nowhere for a *letter*
    to land, because which field is listening is a fact the element tree cannot work out and
    `Workspace::sync_input_keys` answered by naming, in source, every field in the window.
    There were two and neither was a plugin's. The host hands the field back — a plugin builds
    before the workspace exists and has nothing to take one from — asks each claim in
    registration order, and the first to want the keyboard gets it. The worktree menu's branch
    field has moved onto it, so it now goes out with `crook/worktrees` when that is switched
    off, even though the popup that draws it has not moved yet. `crook/tabs` renames a tab and
    a pane through one of its own, and nothing about renaming is in the menu's shell — it
    draws the field, and that is the whole of its involvement.
  - The menu is nine entries from two plugins across seven bands, and one of them is not a line
    of text: the colour swatches are a row of controls the shell offers and the plugin fills,
    because which six colours a tab may be is the tab model's business and how a menu row
    looks is the menu's.
  - Still to move: the worktree menu's own popup, the Themes panel, the Omarchy palettes.
- **Phase 2 — the sandbox: done, and dogfooded.** `crook_plugin_api` is the wire — a
  manifest, capabilities that each say what they are in a sentence, and a `Node` vocabulary
  that *describes* rather than paints: no colours, no pixels, tones and sizes the host
  resolves against the theme in force. `crook_wasm` is the sandbox: a `wasmi` module with no
  imports but six, per-call fuel budgets, a memory ceiling, and every offset a guest hands
  back checked against its own memory before it is read. Its tests are real WebAssembly,
  assembled from text at test time, so a wasm toolchain is not needed to run `cargo test`.

  Two things were **measured rather than assumed**, and both changed a decision:

  - `wasmi` pulls in fourteen crates and not one of them needs a C compiler. Decision 1 in §7
    is settled: `wasmi` and a bytes ABI, and `docs/architecture.md` keeps its rule.
  - `wasmi`'s default dispatch backend uses tail calls and relies on the optimiser to turn
    the recursion into a jump. Unoptimised, it grows the *host* stack per instruction and a
    guest loop of about ten thousand iterations aborts the process — which no amount of fuel
    can catch, because an abort is not a trap. The workspace turns on `portable-dispatch`,
    which dispatches in a loop and cannot grow the stack whatever it is compiled at. The
    cost is some interpreter speed, for a plugin whose whole job is to format a percentage.

  The application half is in: `plugins::wasm` reads `plugin.wasm` out of each directory under
  the platform's data directory, turns what a guest registered into real registrations —
  resolving every string it was handed, refusing a slot nothing declares and prefixing every
  action with the plugin's own id — and draws what it describes through one `Node` → element
  translator. A sandboxed plugin is on the Plugins page, in the command palette and in the
  header beside the ones in the box, and it can *replace* one of them: `header.right` is a
  `Single` slot, and a store plugin asking for a lower order wins it.

  A render that fails draws nothing and says so once; a plugin that fails three times in a
  row stops being asked, because a plugin that traps on frame one will trap on frame two.

  **What it costs in the binary**, measured on this machine: 20,602,664 bytes before and
  23,486,232 after — 2.8MB, or 2.1MB stripped. That is the *host*, and it is the whole of the
  increase: an uninstalled plugin is still not in the binary, and a disabled one is still not
  built. The store's plugins are files in a directory, which is what the tier was for.

  **ABI 2 is the version a plugin can *do* something in.** Version 1 could describe a badge
  and register an action, which is a plugin that can say what it already knew. Two adds:

  - A **request, a ticket and an answer**. The guest calls one import and gets an integer
    back; no socket is opened and no file is touched inside the call, because a guest call
    runs on the thread that draws. The host takes what was asked for afterwards, decides
    whether it is inside what a person granted, does the work on the background pool, and
    brings the answer to `crook_deliver` carrying the same ticket. `crook_wasm` cannot tell an
    allowed request from a refused one and does not try: the grant is checked in the
    application, by `app/src/plugins/wasm/runtime.rs`, which is a model per installed plugin.
  - A **clock**, and a timer. `now` answers with the wall time and asks nobody, because a
    plugin that cannot tell the time cannot say "resets in forty minutes", and knowing what
    time it is reveals nothing that is anybody's to protect. `crook_tick` is the guest export
    the host comes back to after the interval the guest asked for — one worker parked between
    ticks, which is why `PARKED_WORKERS` counts a plugin chain. Fuel remains what *bounds* a
    plugin; the clock is for what it says, never for how long it may run.
  - **Capabilities granted and enforced.** `Capability::ReadFiles` joins the list, and
    `Capability::keys` writes a grant down as text — one key per host and per path — which is
    what makes "re-prompted on escalation" a comparison rather than a judgement. A person
    answers on the Plugins page, the answer is kept in `settings.json` under `plugin_grants`,
    and answering rebuilds that plugin there and then rather than at the next launch.
  - **Seven more `Node` variants** — `Meter`, `Rule`, `Fill`, `Note`, `Pressable`, `Anchored`
    and `Bars` — which is what a panel needs and nothing beyond it. A `Bars` is a row of
    columns each as tall as its share of the tallest, because nobody reads the height of a
    chart of days: they read which day was the busy one.
  - **The pirate, by icon name.** `pirate`, `pirate-open` and `pirate-wide` resolve to Crook's
    own two-layer artwork in `crookui_core::icons::art`. The plugin ships no picture and the
    host runs no timer for it: a plugin that wants a chomping pirate holds its own clock and
    names a different frame, which is animation done entirely on the plugin's side of a
    boundary that carries no pixels.
  **ABI 3 is the version a plugin can *count* in**, and it exists because of one measurement.
  The usage chip's week — a column per day, a breakdown per model, the busiest projects — is
  read out of the transcripts Claude Code writes: three hundred megabytes across five hundred
  files, of which a hundred are lines carrying a `usage` object. The obvious shape was to hand
  the guest those lines a page at a time and let it add them up. Measured, that costs **ninety
  thousand instructions a line**, which for a week is forty seconds of interpreter on the
  thread that draws, and no page size fixes a cost that is per line.

  So `Request::Tally` has the host count instead. The plugin says which files, which lines are
  worth parsing, which of them are the same event written twice, what to group by and what to
  add up — every one of those a *field name* it supplies, none of them anything the host
  understands. What crosses is a couple of hundred rows of totals rather than forty thousand
  lines: about a second on the pool, thirteen milliseconds inside the guest. A `Key` may take
  a prefix of a field, which is how "by the hour" is expressible without the host knowing what
  a date is — and which hour belongs to which day is a question about time zones that stays
  with the plugin, answered by a `timezone` import beside the clock.

  The rule it is an instance of: **the host does what a host is for, and the plugin keeps what
  it means.** Reading a directory and touching a hundred megabytes is the first; deciding that
  two lines are one turn is the second. A capability that had said "read the transcripts"
  would have put the second one here.

  - **`crook --install-plugin <path>`**, which opens a module, checks its ABI, decodes its
    manifest and only then copies it into the plugins directory. There is no server and no
    index; installing is a file copy, and the flag exists because "copy this into a directory
    whose path is different on three platforms" is a sentence a README should not have to say
    twice.

  **ABI 4 is the version a plugin can be asked about something.** Every version up to it could
  be asked what goes in a slot; that is a question with one answer, and a slot drawn once per
  row of a list needs the answer to be different seven times. So a render carries a `Render` — the slot, and the
  `Subject` it is about when the slot has one — and the first subject is a tab row.

  What a subject carries is **redacted against the grant**, in `plugins::wasm`, one field at a
  time: the title and the agent's status need `ReadTabs`, the directory and its branch and
  whether it is a worktree need `ReadWorkingDirectory`, and a plugin that was granted neither
  finds `None` where each would have been rather than a refusal it has to handle. What is left
  for everybody is one number per row — a hash of where the tab is working, salted with the
  asking plugin's own id, so a plugin can tell two rows apart and keep telling them apart
  tomorrow, and two plugins cannot work out that two of their rows are one row. It is not
  offered as a secret: a hash can be checked against a guess, which is exactly why it is *all*
  that is ungated.

  That is what makes a plugin that draws a picture on every tab a plugin nobody has to allow
  anything, and the pair of them is the second dogfood:
  [crook-emoji](https://github.com/theguriev/crook-emoji) takes `tab.row.mark` and asks for no
  capability at all, and [crook-worktree](https://github.com/theguriev/crook-worktree) takes
  `tab.row.badge`, asks for `ReadWorkingDirectory`, and draws nothing until somebody says yes.

  **And the usage chip is not in the binary any more.** It is a sandboxed plugin —
  `theguriev/pirate`, in a public repository of its own at github.com/theguriev/crook-pirate,
  released as one 122KB `plugin.wasm`, asking to read `~/.claude/.credentials.json` and to
  reach `api.anthropic.com`, and able to reach nothing else. `crates/crook_usage`,
  `app/src/usage_model.rs`, `app/src/plugins/usage/`, the `show_usage_chip` setting and the
  `--usage` and `--usage-panel` flags are gone with it. This is the dogfooding the plan asked
  for, and it was done from further away than the plan proposed: not a built-in re-implemented
  and embedded, but a plugin in a repository of its own with nothing to reach the host by
  except the wire. A built-in shares a repository and a CI run with the host and can lean on
  one by accident; this one had nothing to lean on, which is the only way to find out whether
  the ABI is enough.

  **What that cost.** One thing did not survive the move. A second thing was said not to have
  survived, and that turned out to be a wrong answer worth keeping the record of:

  - **The week-history panel did come across, on the second attempt.** It reads the Claude
    Code transcripts under `~/.claude/projects/` — three hundred megabytes in a busy week —
    and the first conclusion here was that a sandbox cannot do that, because one request
    answers with at most a megabyte and a guest handed three hundred of those can exhaust the
    machine by asking. Every fact in that sentence is true and the conclusion did not follow.
    The megabytes are not what the panel draws: a week is twenty-two thousand turns and the
    panel is a hundred and sixty-nine rows of totals. What was needed was not a bigger pipe
    but the counting happening where the reading happens, which is what `Request::Tally` is.

    The measurement that settled it is worth writing down, because the argument had been going
    on estimates: handing the guest the *lines* costs ninety thousand instructions each — for
    a week, forty seconds of interpreter on the thread that draws. Counting them in the host
    costs about a second on the pool, and the answer lands inside the guest in thirteen
    milliseconds.

    What it does cost is the sentence a person has to agree to, and that objection was the
    real one: the grant reads "Read everything under `~/.claude/projects`", which is every
    transcript of everything they have ever asked an agent. There is no narrower way to ask —
    the files are named after sessions nobody knows in advance — so the honest thing is to
    show the sentence and let them refuse it. What the plugin receives is totals; what it is
    *allowed* is the directory, and those are not the same size.

  - **The macOS Keychain path does not come across.** Claude Code refreshes the Keychain copy
    of its credentials and lets `~/.claude/.credentials.json` lag, sometimes by days, and the
    only way to read the Keychain is to shell out to `security`. A sandboxed plugin cannot
    spawn a process — `run.commands` is in §4's list and is not implemented, because a plugin
    that may run one command is a plugin that may run any command — so on macOS the plugin
    reads the file alone and can be behind. That is a real regression on one platform, and it
    is the price of the chip being something a stranger could have written.

- **ABI 6 — a plugin that can *do* something: done.** (A version, not a phase: the phases here
  are the plan's, and the store is still the one numbered three.) Every version up to five let
  a plugin describe what it already knew and ask for what it could be told — a chip that
  reports. Everything below is what it took for one to **act**, and every line of it was
  demanded by a real plugin — the chips at
  [github.com/theguriev/crook-chips](https://github.com/theguriev/crook-chips), which draws the
  row under the line you are typing and is the second thing in this document that lives outside
  the binary.

  - **A place that is not the header.** `pane.chips` is a `List` slot declared by `crook/pane`
    and drawn by `workspace::body` in two places: under the line being composed, and — when a
    program has taken the screen and there is no line — floating over the pane's bottom corner.
    Which of the two a contribution lands in is not something it is told. It is drawn for the
    **focused** pane only, which is the same pane `ReadWorkingDirectory` is about.
  - **A contribution knows which of its own it is.** `crook_render` takes the entry as well as
    the slot, because a plugin may put four things in one list slot and a render told only the
    slot would have to draw all four in each of them.
  - **An action may be told what it is about.** `crook_run` takes an argument, and `Host::voice`
    is the same thing for a native one: an action is a name with no parameters, so what a
    *thing* — this row, that command — is said into a place the handler takes it from. One
    press, one thing said. The Keyboard Shortcuts page's Change buttons use it too.
  - **Three more nodes**, and one of them changes what this tier is. `Chip` is the quiet pill a
    fact sits in, as against `Badge`, which is a loud one for a reading. `Menu` is a secondary
    click, drawn by the host. `Picker` is a field over a filtered list — and **the host drives
    it**: the plugin supplies rows and is told which one was chosen, while the field, the
    filtering, the arrows, Enter and Escape stay on Crook's side. That is not a convenience. It
    is what lets a sandboxed plugin have a search box *without ever being handed a keystroke*,
    and what keeps the per-keystroke work off a call into a guest on the thread that draws.
  - **Six more requests**: `Where` (the focused pane's directory, branch and line counts),
    `List` (the names in one directory, never a tree and never a byte of content),
    `Repository` (a head and its branches, read by Crook's own git so a plugin need not be
    handed a repository), `Commands` (what Crook can be asked to do and the chords that reach
    it), `Type` (a line into the shell) and `Run` (one of Crook's own commands).
  - **Four more capabilities**, each a list rather than a flag for the reason the network is:
    `ListDirectories` names roots and grants *names* rather than contents, which is a weaker
    thing to ask for than `ReadFiles` and is what a directory picker actually needs;
    `TypeCommands` names exact templates with one `{}` in each — the shape of the command is
    the person's and only the hole is the plugin's, and the host fills and **quotes** it, so a
    branch called `; rm -rf ~` stays a branch name; `RunCommands` names exact commands; and
    `ReadCommands` is the command list and its chords.
  - **A request that changes something happens only because somebody pressed something.** The
    four calls that can raise one are a build, a tick, a delivery and an action, and `Type` and
    `Run` are taken from the last of those alone. It is not a refusal — nothing about the grant
    failed — so it comes back as `Answer::Failed` saying exactly that.
  - **Panels come down when the attention moves.** `Host::claim_panel` is `claim_surface` plus
    that rule: a palette floats over the whole window and may go on owning its keys after a tab
    switch, and a panel hung under a chip in a pane is drawn by that pane and may not.
  - **Something only the host can do, done by the host.** "Change keybinding" is a menu entry
    in a plugin and a *command in `crook/shortcuts`*: recording a chord means taking the whole
    keyboard and writing somebody's file, and neither is a thing this tier will ever be given.
    So the plugin asks for `crook/shortcuts/rebind` by name — one line in its manifest — and
    Crook puts up the recorder. The page that used to say "read-only, unlike VSCode's" now has
    a Change button on every row, which is the same flow with a different caller.
  - **Two flags on the headless snapshot**, because a tier whose worked examples can only be
    seen by launching a window is a tier nobody can screenshot: `--with-plugins` loads the
    machine's installed plugins and their grants, and `--action <name [argument]>` runs a named
    action before the picture is taken, which is how a plugin's own panel gets photographed.

  **What it cost.** Modules built against version five are refused by number, which is the
  mechanism working rather than failing: `crook_run` grew an argument and `Render` grew the
  entry, and a host that guessed which shape a module meant would be a host decoding one that
  means something else now. Every plugin in the store is rebuilt against the new vocabulary —
  the API crate is vendored into each of them, so "rebuilt" is a copy of one directory and a
  `cargo build`.

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
back half, and the first of them turned out to be the template twice: `UsageModel`'s poll
chain is now the plugin's own, inside the sandbox, and the thing that drives it from this side
of the boundary — `plugins::wasm::runtime` — is the same shape again with the plugin moved
across. The view is one 3,275-line `Workspace` struct, two views in the whole application
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
- **Keybindings cannot be edited from the interface.** The file is VSCode's — rules, `when`
  clauses, chord sequences, removal by name — and the Keyboard Shortcuts page prints what is
  in force without being able to record a chord into it. That needs a control the settings
  page does not have.
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
own action, a header slot, a settings switch — the template for a feature plugin, and it turned
out to be the template for a *sandboxed* one: this sentence was written while it was a struct in
this repository and it is now a file somebody downloads); the worktree menu. Never plugins: the
block machinery (one address space shared by marks, selection, copy and
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
  dotted strings owned by the declaring plugin: `header.right`, `tab.row.mark`,
  `tab.row.badge` (both built), `tab.row.chips`, `tab.menu.entries`, `block.footer`,
  `pane.content`, `settings.section`,
  `settings.page`, `palette.commands`, `overlay.layers`, `theme.tokens`. Cardinality is
  declared once, by the owner: `single` (highest priority renders), `list` (ordered by
  `order`, then registration), `keyed` (owner dispatches on a key), `chain` (each contributor
  supplies a pure `select`; first non-`None` wins). A contribution is an `Element` builder for
  native plugins and a declarative tree for sandboxed ones (§4).
- `host.actions` — register a named action (`owner/name`) with a handler. Named actions are
  what the keybindings file, the palette and other plugins address; they replace "add a
  variant to `WorkspaceAction`". The thirteen built-in bindings are named actions of
  `crook/window`, bound by the shipped keybindings like anything else.
- `host.settings` — register a schema under `plugins.<id>` in `settings.json`; the settings
  page renders it. The file already keeps unknown keys, so this is the one place a plugin can
  persist today; the schema is what makes it typed and visible.
- `host.fields` — a text field the plugin owns, with the question that decides when the
  keyboard is in it. Registered rather than handed over: a plugin builds before the workspace
  and cannot be given one. This is what a rename, a search box or any other typing a plugin
  offers is built out of, and without it a plugin's surface is one a person can look at and
  not type into.
- `host.keybindings` — default chords for the plugin's actions, as the weakest layer of the
  keybindings: the user's file wins, and so does every shipped binding. `Host::suggest_binding`
  is this, for a native plugin.
- `host.themes` — a theme pack (files), or a token override with a disposer.
- `host.panes` — a new pane content type. `PaneContent` becomes `Agent | Settings | Plugin(id)`
  with a trait behind the third arm; the settings pane is the worked example of everything that
  arm costs (an `Option` on `session`/`status`, a branch in two row renderers, the singleton
  rule, exclusion from restore) — and is what the trait has to cover.

**Events (typed; synchronous for native, delivered for sandboxed):**

`tab.{opened,closed,focused,split}`, `pane.{opened,closed,focused,cwd,title,bell,exited}`,
`command.{started,finished}` (new: needs a `TerminalEvent` variant in `crook_terminal` and a
`TerminalUpdate` variant in the app, fed by the OSC 133 state machine that already exists),
`agent.status`, `git.facts`, `theme.changed`, `settings.changed`, `window.resized`, `timer`.
`usage.reading` was on this list and is off it: the reading belongs to a plugin now and no part
of the core can emit it, which is the ordinary fate of an event named after a feature rather
than after a thing that happens. Three dispatch modes, not four: `emit` (broadcast), `bail` (first
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
Crook without the Themes panel or the command palette. The shipped binary is the default set,
whole.

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

**As built, that tree is fifteen `Node` variants**, and the list above is close but is not what
landed: `Empty`, `Text`, `Badge`, `Icon`, `Row`, `Column`, `Gap`, `Button`, `Meter`, `Rule`,
`Fill`, `Note`, `Pressable`, `Anchored`, `Explained`. There is no field and no switch, because
nothing has needed one yet and a variant nobody uses is a variant that has to keep working for
ever. The last two are the two ways something can be hung off a contribution, and they differ
by who owns the fact that it is up: an `Anchored` panel is the plugin's state and needs an
action to learn it was dismissed, while an `Explained` note is up exactly while the pointer is
on the thing — so the host shows it without asking, and a plugin that has stopped answering
cannot leave one on screen. Two of them are a *share of an axis* rather than a size — `Fill`,
which takes whatever is left of a row, and `Meter`, which is a fraction of a bar the host
decides the length of — and an axis nobody bounded cannot be shared. A contribution starts
unbounded, because a slot offers no width: `header.right` hands its entry an infinite main
axis, the row it sits in having already given its surplus away. The one thing the host bounds
is a panel, which it made a fixed width itself. So **a `Fill` or a `Meter` outside a panel
draws nothing**, and says so once in the log for whoever wrote the plugin. The alternative is
what `Flex` does when it is asked to divide infinity, which is to assert in a debug build and
lay out something degenerate in a release one, and a plugin from a store does not get to do
either to somebody's window.

**As built, that tree is nineteen `Node` variants**, and the list above is close but is not
what landed: `Empty`, `Text`, `Badge`, `Chip`, `Icon`, `Row`, `Column`, `Gap`, `Button`,
`Meter`, `Bars`, `Rule`, `Fill`, `Note`, `Pressable`, `Anchored`, `Explained`, `Picker` and
`Menu`. There is still no switch, and there is no *field* either: `Picker` carries one, and it
is the host's rather than the plugin's, so what is typed into it never crosses the wire. A
variant nobody uses is a variant that has to keep working for ever, which is why each of them
arrived with a plugin that needed it. Two are a *share of an axis* rather than a size — `Fill`, which takes whatever is
left of a row, and `Meter`, which is a fraction of a bar the host decides the length of — and
an axis nobody bounded cannot be shared. A contribution starts unbounded, because a slot offers
no width: `header.right` hands its entry an infinite main axis, the row it sits in having
already given its surplus away. The one thing the host bounds is a panel, which it made a fixed
width itself. So **a `Fill` or a `Meter` outside a panel draws nothing**, and says so once in
the log for whoever wrote the plugin. The alternative is what `Flex` does when it is asked to
divide infinity, which is to assert in a debug build and lay out something degenerate in a
release one, and a plugin from a store does not get to do either to somebody's window.

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

Thirteen are built, and they are the ones the plugins that exist needed or could be given
honestly: `ReadSettings`, `ReadTabs`, `ReadWorkingDirectory`, `Clipboard`, `Storage`,
`Network` as a list of hosts, `ReadFiles` as a list of exact paths, `PlaySound`,
`WatchCommands`, and the four ABI 6 added — `ListDirectories` (roots, and names rather than
contents), `TypeCommands` (exact command templates, with the host filling and quoting the
hole), `RunCommands` (exact command names) and `ReadCommands`. The last two are lists
rather than flags for the same reason: "this plugin talks to the internet" and "this plugin
reads your files" are not things anybody can meaningfully agree to, and "api.anthropic.com" and
"~/.claude/.credentials.json" are. A leading `~` is the person's home directory and is the only
thing expanded; a path holding `..` is refused rather than resolved, so a granted path cannot
be walked out of. The grant is answered on the Plugins page rather than in an install dialog —
there being no installer to put one in — and is kept as *text* under `plugin_grants`, one key
per host and per path. That is the whole mechanism behind "re-prompted on escalation": a
plugin that adds a host in its next version asks for a key nobody allowed, so it is not
granted, and the page can say which line is the new one. Comparing the capability values
instead would make any change to a list a change to one value, and the only honest thing to do
then would be to ask about all of it again.

**Asking, and being answered.** A capability is not a door a plugin walks through; it is what
makes an *ask* answerable. Everything past drawing is a `Request`: the guest *asks*, and the
host *decides*. Four calls can raise one, and they are the four that run somewhere work can be
started from — `build`, an action a person ran, a tick the plugin's own timer brought round,
and the delivery of an earlier answer. The last of those is what makes this a loop rather than
a single shot: a plugin that reads a file and then fetches what the file authorised it to fetch
needs no special case.

**A request raised while describing waits for the next of those.** The guest may call the
import from anywhere, but a render runs on the frame path, where nothing may be started, so
what a render asked for is taken on the next call that can take it. A plugin that wants to poll
asks from its tick, which is what a tick is for.

**A refusal is not a failure.** A request outside the grant does not trap the plugin, does not
count against the budget that disables one, and does not come back as an error. It comes back
as `Answer::Refused` carrying the permission sentence **verbatim** — the same string
`Capability::sentence` hands the dialog — so a plugin can say "allow me to reach
api.anthropic.com" rather than "something went wrong". That is the reason the sentence belongs
to the capability rather than to the dialog: the plugin and the dialog have to say the same
thing, and a plugin is the last thing that should be trusted to know what the dialog said.

**Refusals are bounded all the same.** A plugin that answers every refusal by asking again is a
loop, and a loop that spends a background task per turn is a machine with a fan on. Sixteen in
a row and it stops being asked at all until Crook is restarted — far more than a plugin has
reason to ask before somebody allows it, and few enough that the loop stops being free. One
allowed request resets the count, because a plugin nobody has answered for yet is in the
ordinary state and not in a bad one.

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

The plugin below is not an invention. Its id, its repository and the two capabilities it asks
for are the real ones — this is the chip that used to be a feature in the README, and the thing
every claim in §4 was tested against.

```toml
schema = 1
id = "theguriev/pirate"          # owner/name; owner is the GitHub account of the directory
name = "Claude Code usage"
version = "0.1.0"                # semver
api = ">=2, <3"                  # the crook_plugin_api version range this was built against
license = "MIT"                  # SPDX, from a short accepted list; CI rejects anything else
repository = "https://github.com/theguriev/crook-pirate"
description = "How much of the session budget is spent, in the header."
tier = "wasm"                    # "native" | "wasm" | "process"
platforms = ["linux", "macos", "windows"]

[capabilities]
net = ["api.anthropic.com"]      # one host, not a pattern: see §4
files = ["~/.claude/.credentials.json"]
parks = 1                        # background workers this plugin will hold on a timer

[contributions]                  # everything that is data and needs no code to be read
keybindings = [{ key = "shift+cmd+u", command = "theguriev/pirate/refresh" }]
settings = "settings.schema.json"
themes = ["themes/*.yaml"]
```

`id` is `owner/name` and the owner is the directory's CODEOWNER — Raycast's rule, enforceable
by GitHub. Built-ins carry `builtin = true` in the index, not in the manifest.

**None of this file exists yet**, and the distinction matters more now that there is a real
plugin to be honest about. What a `.wasm` carries today is `crook_plugin_api`'s `Manifest`,
encoded into the module and read by the host *before* any of the plugin runs — the api version,
the id, the name, the description, the plugin's own version, and the capabilities it asks for.
That is enough to list a plugin, to say what it wants, and to refuse one built against a
vocabulary this binary does not speak, none of which may require running it. The rest of the
table above — the licence, the repository, the platforms, the default keybindings, the settings
schema — is the *store's*, and arrives with the store in Phase 3. A plugin installed by hand
today has no store and needs none.

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

There are none yet, and the first Tier-2 plugin was deliberately not made one. The usage chip
could have been embedded and would have proved less: an embedded plugin is a plugin whose
author can reach across the boundary by accident, because both halves are in one repository and
one CI run. Sending it away — its own repository, its own tests, its own release — is what made
the ABI answer for itself. The rule above still stands for a *feature a terminal is not a
terminal without*, and the usage chip turned out not to be one of those. Which built-ins there
should be is a question for whenever something is.

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
5. **How a plugin's surface is photographed.** This one is a regression, and it is the only
   thing the move to the second tier made worse. Most of the flags in `--help` are there so a
   surface can be drawn into a PNG deterministically, and `--usage <PERCENT>` was the one for
   the header's right-hand side: it put a known reading in the chip, so the picture was the
   same on every machine and in every run — no network, no credentials, no clock. There is no
   equivalent now. A plugin's surface is whatever the plugin says it is, and nothing in the
   binary can be told what a plugin ought to be saying. The candidates are a flag that stands
   a named plugin's contribution in for a fixed `Node` tree, a plugin that draws a fixed one
   and is installed by the snapshot script, and doing nothing on the grounds that a store
   plugin's pixels are not Crook's to guarantee. Until one of them is chosen, the right-hand
   end of the header is the one surface with no picture of it under test.

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
network capability — the template, and the one that did not stay: Phase 2 rebuilt it outside
the binary and `DefaultPlugins` is one shorter for it); the **worktree menu**
(`tab.menu.entries`, the `Git` and `Tabs` services, background work with deadlines); the
**Themes panel** (`settings.page`, a new pane content type or a panel slot — decision to be
made when it is reached); the **settings pages** themselves (`settings.section` fed by schemas
— the search already works over `Category`/`Entry`); the **Omarchy palettes** as a theme pack.
And the first *new* plugin: a **command palette**, because a system whose primary noun is
"action" needs one on day one — herdr shipped without and its community wrote three. When
Phase 1 ends, `DefaultPlugins` is real and `Workspace` is a host.

**Phase 2 — the sandbox.** `plugins/host-wasm` on `wasmi`; `crook_plugin_api` with the
`postcard` message types and the declarative tree; capabilities declared, granted, enforced;
fuel and deadlines; self-disable. One built-in re-implemented as a Tier-2 plugin, to dogfood
the ABI end to end before any outsider touches it. Done, and it went further than this
sentence proposed: the plugin was not embedded but *removed* — the usage chip is a file in
another repository, installed with `--install-plugin`, and the binary no longer contains it in
any form. What that proved, and what it cost, is in "Where this stands".

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
