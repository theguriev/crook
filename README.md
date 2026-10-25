# Crook

A terminal whose unit of work is an agent, not a tab.

Every terminal ever written treats a shell session as the thing you open, arrange and close.
Crook treats an *agent* as that thing. A tab is one agent's workspace: its transcript, its
working directory, its state. The tab strip is therefore a list of what is currently being
worked on, and the header carries whatever says what that work is costing.

Crook copies the architecture of [Warp](https://www.warp.dev) — an Entity/Handle application
core, immutable `View::render`, constraint-based layout, a `Scene` display list handed to a
GPU renderer — while deliberately taking one backend instead of two, and no build scripts at
all. See [`docs/architecture.md`](docs/architecture.md) for what was inherited, what was
dropped, and why.

## What it looks like

<img src="docs/images/tabs.png" alt="The tab panel: four agent sessions, each with a coloured status dot and the branch it is working on, one of them a group of two tabs" width="330" align="right">

The panel down the left edge is the claim above, drawn: one row per agent, a dot saying whether
it is running, waiting on an answer, finished or failed, and under the title the branch that
work is happening on. Two rows that belong together fold into a group with a count — a tab and
the worktree opened from it. The strip is the list of what is being worked on, so reading it is
reading the state of the machine.

<br clear="right">

![Three finished commands, each in its own block with a rule between them, the last one showing the copy and menu controls that appear under the pointer](docs/images/blocks.png)

The output is a transcript of blocks rather than one scrolling screen: a command, everything it
printed, and a boundary. The pointer over a block brings up the two controls in its corner —
copy the whole thing, or open its menu — and the block under the prompt is the one still open.

![The find bar over a pane, reading "tab", counting three matches and highlighting them in the output where they are](docs/images/find.png)

Find searches the blocks, not the screenful: every finished command and the open one at once,
counted in the bar, the current match in the accent and the rest in amber. It is a search and
not a filter, so the lines between the hits stay where they are. A pane with no blocks to
search — an agent, an `ssh` session — is searched through its whole scrollback instead.

![The command palette over the window, reading "rec": two commands under a Commands heading, a tab under Tabs with the directory it is working in, and five settings rows under Settings, each naming the page and category it lives on](docs/images/palette.png)

The palette is one box for everything that has a name: the commands, the tabs that are open
and the rows of the settings, each kind under a heading of its own when there is more than one
kind of answer. A command row prints its name and, when one reaches it, the chord; a tab row
says where it is working and Enter goes there; a settings row says which page and category it
lives on and Enter opens that page with the row on screen. A sigil narrows it to one list —
`>` commands, `@` tabs, `#` settings, `?` what is bound to what — and `tab` walks between them.

![The Plugins page: every plugin with a face beside its name, and the Pirate's card — what it asks to be allowed to do, with Allow beside it, and the pictures it carries under "What it looks like"](docs/images/plugins.png)

Every plugin has a card, and the card is where it is answered. The ones from the box wear one
of Crook's own marks; the ones from a file bring their own face and their own pictures, inside
the module, so the card can show what a plugin looks like before it has been allowed anything —
and what it asks for is a list of sentences with one button beside it, because installing is
not allowing.

These are rendered by Crook itself rather than captured from a window — `--snapshot` draws one
frame of the real view tree to a PNG, which is how a picture of the application is the same on
every machine. The two that print a git log print it up to the `v0.1.0` tag, so that they say
the same thing whenever they are rendered again and the find still counts three:

```sh
./script/run --snapshot docs/images/tabs.png            # then cropped to the panel
./script/run --run 'git status --short --branch' \
             --run 'git log --oneline -8 v0.1.0' \
             --run 'cargo test -p crook_plugin_api --quiet' \
             --hover-block 2 --snapshot docs/images/blocks.png
./script/run --run 'git log --oneline -12 v0.1.0' \
             --find-output tab --snapshot docs/images/find.png
./script/run --action 'crook/palette/open rec' --snapshot docs/images/palette.png
./script/run --dev-plugin ../crook-pirate/target/wasm32-unknown-unknown/release/pirate.wasm \
             --section Plugins --action 'crook/plugins/show theguriev/pirate' \
             --action 'crook/plugins/pictures theguriev/pirate' --snapshot docs/images/plugins.png
```

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/theguriev/crook/main/script/install | sh
```

macOS on both architectures, and Linux on x86_64 with glibc 2.31 or newer: Debian 11, Ubuntu
20.04, RHEL 9 and anything since. It reads the latest release, checks the archive against the
`SHA256SUMS` published beside it, and puts the binary in `~/.local/bin`, or somewhere else with
`--to`. Windows is a `.zip` on the [releases page](https://github.com/theguriev/crook/releases).

The Linux binary is built in a Debian 11 container so that it starts on all of those, and
`script/glibc-floor` stops a release whose binary would need a newer glibc before it is
published. v0.1.13 was built on Ubuntu 24.04 instead: it needs glibc 2.39, and on anything older
it does not start, with ``version `GLIBC_2.39' not found``.

### Linux, in the launcher

```sh
curl -fsSL https://raw.githubusercontent.com/theguriev/crook/main/script/install | sh -s -- --desktop
```

`--desktop` also installs Crook's desktop entry, its icons and its AppStream record under
`~/.local/share`, which is what puts it in the application menu and lets `xdg-terminal-exec` —
what a launcher's *Open in terminal* and a `Terminal=true` entry ask for — start it. The Linux
archive carries all three under `share/`, laid out the way a package installs them. Nothing that
is not Crook's is written: which terminal your desktop opens is yours to say, so
`xdg-terminals.list` and `mimeapps.list` are left alone and the installer prints the one command
that puts Crook first — or the **Default terminal** button on the settings' Appearance page does it for you.

A launcher starts Crook the way it starts any terminal, with the flags the entry advertises:

```sh
crook -e htop -d 10                     # a window whose one pane runs htop, and closes with it
crook --working-directory ~/src/crook   # a window whose shells start there; --cwd is the same
crook --title notes --app-id notes      # its name, and the app_id / WM_CLASS a window rule matches
```

Everything after `-e` is the command, as in xterm, and `crook -- htop -d 10` says the same. It is
typed into the pane's shell as an `exec`, so it runs on the `PATH` your profile built and, when it
exits, the pane closes the way it does when a shell exits. `-e` is not available on Windows, whose
shells have no `exec`. The line it types has to fit what a terminal keeps of a line typed before its
shell is reading, which is 4095 bytes on Linux and about a thousand on macOS; a longer command is
refused rather than cut short.

**A window opened with `-e` or `--working-directory` is a launcher's.** It is a quick terminal
beside your work rather than the work: it opens fresh instead of as the last window, and nothing it
does is written to the session file, so closing it leaves the agent tabs your next ordinary launch
comes back to exactly as they were. `--title` and `--app-id` alone open an ordinary window.
The entry also offers **New Window** to launchers that list actions. It runs a plain `crook`,
so while Crook is open it gives you a second, fresh window that leaves your tabs to the first
(see Tabs below).

That covers every window a launcher opens with a directory, including one that always names a
directory. Omarchy's terminal key is one: it runs `xdg-terminal-exec --dir=…` with the focused
terminal's directory, or your home when there is none. So once Crook is xdg-terminal-exec's first
choice, the windows that key opens start fresh and never save the session. To get your tabs back,
open Crook from the application menu or run a plain `crook`.

### macOS, from the browser

`crook-v<version>-macos.dmg` on the [releases page](https://github.com/theguriev/crook/releases)
holds `Crook.app` — one universal build for Apple Silicon and Intel both. It is signed with a
Developer ID, notarized, and the ticket is stapled to the app before the image is built around
it; the image is signed, notarized and stapled the same way. So the image opens with no warning
and the check needs no network. Drag `Crook.app` to Applications. Its first launch asks once to
confirm an app downloaded from the Internet, and **Open** starts it.

A question rather than a refusal is the difference notarization makes, and it is worth knowing
what it fixes. A Mac quarantines anything a *browser* downloads, and on an unnotarized build the
first launch is refused outright — *"Apple could not verify 'crook' is free of malware"*,
offering only **Move to Trash** and **Done**. Releases before v0.1.1 were unnotarized and do
that. If you are looking at that dialog now, press **Done** rather than *Move to Trash*, and
clear the attribute yourself:

```sh
xattr -dr com.apple.quarantine crook && ./crook
```

On macOS 15 and later that command, or **System Settings → Privacy & Security → Open Anyway**, is
the whole of the way past it: the old right-click-and-Open shortcut was removed.

The `.tar.gz` archives hold the bare binary and are what `script/install` fetches. Nothing `curl`
downloads is quarantined, which is why that path never needed the dialog in the first place.

**`crook` on the command line.** Nothing in an app bundle is on `PATH`, so in its own panes Crook
puts itself there: when no `crook` answers in a pane, its `PATH` ends with
`Crook.app/Contents/MacOS`, so the `crook …` lines below work in a pane as written. It goes last,
so it never stands in front of a `crook` of your own. A startup file that sets `PATH` outright and
then starts another shell or tmux in its own place can still drop it, and `"$CROOK_BIN"` is the
same command there. For other terminals, link it once into a directory on your `PATH`:
`ln -s /Applications/Crook.app/Contents/MacOS/crook /usr/local/bin/crook`, with `sudo` if that
directory is not yours.

To build it yourself instead, see [Prerequisites](#prerequisites) and [Build and run](#build-and-run).

### Updating

```sh
crook --check-update      # is there a newer release? one request, and only when you ask
crook --update            # fetch it, check it against SHA256SUMS, and replace this binary
crook --update-plugins    # the same for every installed plugin the registry is ahead of
crook --update-plugin theguriev/chips
```

`--update` installs the archive for this platform the way `script/install` does — verified
against the published sums, unpacked, and renamed into place within the binary's own directory —
so a Crook that is running keeps the file it started from until it is restarted. A `Crook.app`
is replaced whole instead: the release's disk image is checked against the same sums, mounted
read-only out of sight, and the app in it is copied beside this one, held to the same signing
team and identifier, accepted by Gatekeeper, and only then renamed into place. Where Crook is
not its to replace it says so and stops: an app run from the disk image or from a folder this
user cannot write is updated from a new disk image, a binary a package manager installed is that
package manager's, and a build in `target/` is another build.

Both also live in the window. **Settings → About** has an **Updates** row: *Check for updates*
asks, the answer is the line under it, and *Update* appears beside it when this copy is one Crook
may replace — with the reason in its place when it is not. While a check has found a newer
release the **Settings** button in the sidebar carries a dot, which is the whole of the notice.
A plugin is the same story on its own card: the Plugins page says when the registry is ahead of
it and offers **Update**, and the list has **Update all**.

**Nothing here is asked of the network until you ask for it.** There is no check at launch, no
timer and no background poll, which is the same rule the store is written under: the request
goes out when you run one of these, or press one of those buttons, and carries no version, no
machine id and nothing else about this machine.

## When something goes wrong

Every window Crook opens writes a log file, and a panic writes a crash report, into one folder
on this machine:

| platform | folder |
| --- | --- |
| Linux | `~/.local/state/crook` — `$XDG_STATE_HOME/crook` when that is set |
| macOS | `~/Library/Logs/Crook` |
| Windows | `%LOCALAPPDATA%\crook` |

`logs/` holds `crook-<channel>-<when>.log`, `<when>` being the launch in UTC: the lines the
terminal gets, filtered the same way (`RUST_LOG` still decides), written whether or not a
terminal is attached. `crashes/` holds `crook-<channel>-<when>.txt`, `<when>` being the panic:
the version and platform, where it panicked and with what message, a backtrace, the GPU it was
drawing with, and the last 200 lines of the log. A shipped build's backtrace names functions but
not files and lines on Linux and macOS, and names nothing on Windows, whose names are in a
`crook.pdb` the download does not carry — there the report's `Where` line is the place to start.
The newest five of each are kept, and a log another Crook window is still writing is never
removed. **Settings → About → Logs and crash reports** names the folder and opens it, and after
a crash the next window says so in one line under the header: *Show* opens the folder,
*Dismiss* puts the line away, and neither brings back a report that line was about.

**Nothing in that folder leaves this machine.** Crook never sends, uploads or reads any of it
back to anyone. To report a problem, [open an issue](https://github.com/theguriev/crook/issues)
and attach the crash report, or paste the log lines from around the time it went wrong — having
read them first, since a log can name folders and files of yours.

What it cannot catch is a native crash: a segfault in a GPU driver, an access violation, an
abort inside a system library ends the process without running any of Crook's code, so it
leaves no report. The log file is still there up to its last line, and the system's own crash
reporter — `coredumpctl`, Console.app, the Event Viewer — has the rest.

## v1 scope

Twenty-two features, and the page that configures them:

- **Tabs.** Open, close, switch, reorder. One agent session per tab, and the tab is named
  the way a person would answer "what is that one?": what you called it, else what the agent
  in it calls its work, else what it is running right now — `cargo test` while the tests run —
  else the directory it sits in. A number is what is left when a pane has no name, no program
  and nowhere to be, which is nowhere anybody works.
  A tab closed by mistake comes back with `cmd-shift-t` (`ctrl-shift-alt-t` off macOS) — where
  it was and in the group it was in, with its name, colour and split, each shell in the
  directory it was in, and each agent that was running offered its resume line, unsent, the
  way a restart offers it. A group closed from its heading comes back whole in one press,
  under the same heading, and a pane closed out of a split goes back into its tab, where it
  was — the corner of a 2×2 back in its corner, with the shares it had. The last
  ten closes are kept, for as long as the window is open.
  They live in a panel down the left edge, and a **View options** menu — the panel's own
  secondary click, on the empty space the list leaves — says what a row of them shows: which
  fact it leads with, what its second line says, which chips it carries, whether hovering it
  opens a card, and the tab's number, which is the one `cmd-4` means once the tabs are named.
  The diff chip counts what is not committed yet — `+12 -3` — until the branch has commits of
  its own, and from then on counts from where the branch left its base: `2 commits, +40 -3`,
  committed and uncommitted lines together, with `since main` added on the card, which has the
  room to say what they are counted from. So a tab whose agent has just committed everything
  still says what it did, rather than going blank at the moment there is most to look at. The
  base itself and a detached checkout keep the plain count.
  One more row is the Appearance page's alone: **Status marks**, which turns the dot into a
  glyph per state in the same colour — a play mark, a bell, a crossed ring, a hollow one — for
  an eye the colour says nothing to, or a screenshot in greyscale. Either way a running agent's
  mark breathes, fading and coming back every two seconds, so a working row reads as working
  without a pointer over it, and nothing moves once nothing runs. Nor is a breath drawn for a
  row scrolled out of the panel, nor while the window is minimised or, on macOS and on X11
  without a compositor, entirely covered — Windows and a compositor do not say when a window
  is behind others. **Motion**, on the same page, keeps the marks and the caret still:
  *System*, the default, does as the desktop's own reduce motion switch says (macOS, Windows,
  and GNOME's animations), and *Reduced* and *Full* say it for Crook alone. A caret left alone
  for ten seconds stops blinking either way. A row whose mark a plugin has taken keeps its
  status on the mark's corner instead: the PowerBook Duo pirate grinning while the agent
  works, a dot in the status colour while it waits or after it failed.
  Under that space is a `+`, which is where Warp's browser puts the one that opens another tab.
  A new tab starts where the focused pane is working, not where Crook itself was started — from
  the Dock, that would be your home directory every time.
  Above the list is a **search box**, Telegram's way round: it
  filters the rows by what a tab is running, where it is working, the branch it is on and its
  status word — `waiting` is the amber rows, and `running`, `failed` and `idle` are the dot's —
  and filters nothing else — the active tab stays active and a filtered-out tab goes on
  printing.
  `cmd-k` (`ctrl-shift-k` off macOS) puts the keyboard in it from anywhere in the window,
  Enter opens the top match, and Escape gives the keyboard back to the shell.
  `cmd-b` (`ctrl-shift-b`) hides the whole panel for wide output or a small screen and brings
  it back on the next press — the search chord brings it back too, since a box nobody can see
  is no use — and a hidden panel stays hidden across launches. Its right edge is a handle:
  dragged, it makes the panel wider or narrower, and a double click puts the default width
  back. The width is remembered across launches too.

  Tabs that belong together are folded into a **group**: a heading with a chevron, a count and
  a close button, and its tabs indented under it. A worktree opened from a tab makes one out of
  the two of them, which is what says the two checkouts are one piece of work. Drag a row into a
  group to add it, past the group's edge to take it out, and a heading to move the whole block.
  The row leaves the list and follows the pointer, the list reorders itself under it as it goes
  and shows the slot it will drop into, and a group whose last tab leaves it goes away by
  itself. A group is not a split screen — the tabs in it are still one at a time, and splitting
  a tab is still `cmd-d` (`ctrl-shift-d` off macOS), asked for on purpose.

  **The window comes back the way it was closed**: the tabs with their names, pins, colours
  and groups and the order you last used them in, the splits, the directory each shell was
  in — and, for a pane that was running a coding agent, that agent's name. Every Crook update
  is a restart and a restart ends every process in the window, so a pane that had Claude Code
  in it comes back as a shell in the same
  directory with `claude --continue` waiting in its command line, **unsent**: nothing runs
  until you press Enter. Codex is offered `codex resume --last` and Gemini CLI `gemini --resume
  latest`, each the most recent conversation in that directory, and Copilot its `copilot
  --resume` picker, since its most recent is the repository's rather than the worktree's. Two
  panes of one agent in one directory are offered its picker instead (`claude --resume`,
  `codex resume`), because the directory names one conversation and they had two; Gemini CLI
  has no picker on its command line, so two of its panes in one directory are offered nothing.
  Any command that runs in a pane first, even a `cd`, ends the offer there; clearing the line,
  with ctrl-c or otherwise, does not, and the next restart offers it again. **Resume
  every agent** in the palette (`resume-agents`) sends every line a restore typed that you
  have not touched, in one press. What is remembered is the program's name and nothing after
  it, so a prompt typed on the command line never reaches the file, and it needs the shell
  integration, which is what says what a pane is running. An agent started through a package
  runner or a version manager is known too — `npx @google/gemini-cli`, the way Gemini CLI's
  quick start has it, or `bunx`, `pnpm dlx`, `yarn dlx`, `npm exec`, `mise x … --` and
  `asdf exec` — and its line is offered through the same one, `npx @google/gemini-cli --resume
  latest`, since there may be no `gemini` on your `PATH`; the runner and the package are kept,
  and its options are not. So is one behind `env`, a `VAR=value`, `command`, `exec`, `time`,
  `nohup` or `nice`, none of which is kept, so an API key set on the line stays off the disk;
  nor is a `PATH=`, so an agent only that `PATH` finds is offered its bare line. One run in
  another directory (`env -C`, `mise x -C`, `npx --workspace`), in a sandbox (`mise x
  --deny-all`), or with an option of `env`'s, the runner's or `mise x`'s that changes how it
  runs (`-E`) or that Crook does not know is offered nothing, since a line without those would
  resume another directory's conversation or run it some other way; its status is still read
  from its title while it runs. An alias is not seen through: the shell reports the line the
  way you typed it. A line of your own goes in `settings.json` under `resume_lines`, keyed by program — `{"claude":
  "claude --continue --model opus"}` — and an empty one offers nothing; one that starts with
  the agent's name is run through the same runner. OpenCode and aider have no line until you
  give them one: OpenCode's `--continue` reaches across a repository's worktrees, and aider's
  `--restore-chat-history` is left for you to choose. No output comes back, and no process
  does. The window itself comes back the size it was and where it was, maximised or
  fullscreen if it was, on the monitor it was on — or on one still plugged in, if that one is
  gone. Wayland does not tell an application where its window is, so there the compositor
  places it as it always does. On X11 a window closed fullscreen comes back the size and in
  the place it had before, but not fullscreen.
  **The first window you open keeps them.** Open Crook again while it is running, from the
  application menu or with a plain `crook`, and the second window opens fresh, with a line
  under its header that says another window keeps your tabs. It does not come back with a copy
  of the first window's tabs, which would offer to resume agents still running there, and
  nothing it does is written over them, so its own tabs are not there at the next launch. A
  window does not see settings changed in another one, so turning **Bring the tabs back** on in
  the second window does not turn it on in the first: turn it on there. If the first window has
  closed by then, the second keeps the tabs in its place. Once the first window closes, the next
  one you open keeps the tabs. A lock on `session.lock` beside the session file says which
  window that is, and the system lets go of it when that window's process ends, crash included.
  **New window**, `cmd-n` (`ctrl-shift-n` off macOS), opens another window, fresh, in the
  focused pane's directory, as `--working-directory` would.
  **Closing asks first when it would end something still working.** The window's close — the
  desktop's close button, `alt-f4`, the palette's Close the window — a row's ×, a group's and
  `cmd-w` (`ctrl-shift-w`) on a pane all end whatever runs in the panes they take, so while an
  agent in one of them is running or waiting on you, or its shell is running a command, a card
  says how many, names up to three, and offers **End them and quit** (or **End them and close**)
  beside **Cancel**. Cancel is the default: Enter and Escape both press it, and so does a click
  anywhere off the card. Tab or the arrow keys move to the other button, and Enter or Space then
  presses whichever is filled — once the keyboard has been still for a second, so a Tab and a
  Space typed on after a stray close cannot end anything. While the card is up the pane under it
  hears no keys at all, so a `ctrl-c` meant for the card cannot interrupt the agent it is asking
  about. A window close that asks restores a minimised window and, if the window is not the one
  being typed in, asks the desktop for attention rather than taking the keyboard: a flashing
  taskbar button, a bouncing dock icon, an urgent window, or on Wayland an activation request
  the compositor may answer by focusing it. A close with nothing working in it goes at once, as
  it always did.
  **Ask before ending working agents**, on the Appearance page, turns the question off, and
  **End all agents and quit** in the palette quits without it once. Nothing can ask when the
  system ends Crook itself, as a `SIGTERM` or a forced logout does. On macOS `cmd-q`, the Quit
  menu item, the Dock's Quit and a logout or restart ask too: with something working, the quit
  is cancelled — a logout with it, as Terminal and iTerm2 cancel one — and the same card comes
  up as for the window's close. Nobody has watched that happen on a Mac yet.
- **The window's own title bar.** There is no strip of system chrome above Crook. The window is
  opened with the application's frame, so the header *is* the title bar: dragging its empty
  space moves the window and a double click maximises it. On macOS that surface carries
  AppKit's traffic lights — the bar is made transparent and the content view reaches the top of
  the window, so the buttons stay exactly where every Mac application has them, and the header
  reserves the 70px they occupy and takes it back in fullscreen, where macOS moves them away.
  Crook draws no caption buttons of its own anywhere: on Windows and Linux the window has no
  frame at all, the header runs to both corners, and minimising, maximising and closing are the
  desktop's own shortcuts and the window plugin's commands. Crook still finds its own resize
  edges there. **Linux has been run and Windows has not**: the frameless window comes up under
  Hyprland with a shell drawing in it, and what that first run found was a window with no
  `app_id` — nothing a window rule could match — which is fixed. See
  [`docs/architecture.md`](docs/architecture.md) §3 for what is still unrun and what was checked
  instead of running it.
- **Plugins, in two tiers, and the second one is not in the binary.** Everything Crook itself
  does is a plugin on the registries a stranger's plugin uses — the header, the window's own
  commands, the command palette, every settings page — which is the only way to know
  the API is enough. Beside that native tier is a **sandboxed** one: a `.wasm` module in the
  plugins directory, run in an interpreter with no imports but a handful, with no filesystem,
  no network and no clock of its own. It *describes* what it wants drawn — text, a badge, a
  meter, a hairline, something pressable, a panel hung under it, one of Crook's own icons by
  name — and the host paints it, so a plugin names no colour and no pixel and comes out right
  in a theme written years after it. Anything it wants from the machine it has to **ask** for,
  and every request is checked against what a person granted on the Plugins page and kept in
  `settings.json`. A request outside that grant is refused rather than failed, and the refusal
  carries the sentence the permission dialog says, so a plugin can tell you what to allow
  instead of that something went wrong.

  **The Claude Code usage chip is the worked example**, and it is the proof, because it used to
  be one of the features on this list and is now a file. It lives at
  [github.com/theguriev/crook-pirate](https://github.com/theguriev/crook-pirate) and ships as
  one 204KB `plugin.wasm`. `crook --install-plugin <path>` checks the module, reads its
  manifest and puts it where Crook looks —
  `<data>/crook/plugins/theguriev.pirate/0.3.0/plugin.wasm`, the version in the path so that an
  upgrade writes somewhere new rather than over the bytes a running interpreter is reading.
  `crook --plugins` says what is installed and which file each one runs from — as a JSON array
  with `--json`, for a script or an agent — and
  `crook --uninstall-plugin theguriev/pirate` takes it and its permissions back off. It draws
  Crook's own pirate — the artwork is the host's, asked for by icon name, and the plugin
  animates the bite itself by naming a different frame — and it says how much of the session's
  token budget is spent and when it resets. To do that it asks to read one file,
  `~/.claude/.credentials.json`, and to reach one host, `api.anthropic.com`. It can reach
  nothing else, and until somebody says yes it reaches neither.

  **A plugin can also be asked the same question once per row.** The mark at the head of every
  tab in the panel is a slot, and the small badge on its corner is another one, so a plugin
  can replace what a tab is drawn as or add one more thing to it. A contribution to those is
  handed the row it is being drawn on, with everything a person did not allow it to see left
  out — and a plugin allowed *nothing* still gets one number per row, the same number for one
  tab every day and a different one for the tab beside it. That is enough to give every tab a
  picture of its own, and it is what
  [github.com/theguriev/crook-emoji](https://github.com/theguriev/crook-emoji) does: an emoji
  where the status dot was, asking for no permission at all. Beside it,
  [github.com/theguriev/crook-worktree](https://github.com/theguriev/crook-worktree) puts a
  branch mark on the corner of every tab whose directory is a git worktree, which it can only
  do because somebody allowed it to see which project each tab is in.

  **And there is a place to get one from.** The **Store** at the foot of the sidebar is the
  registry's list — [github.com/theguriev/crook-plugins](https://github.com/theguriev/crook-plugins),
  which builds every plugin from source and publishes one static `index.json`. Nothing is
  fetched until you press *Look for plugins*: no account, no machine id, no list of what you
  have — the request says `crook` and not even which version — and a machine that is offline
  shows the last list it read with its age under it.
  Pressing **Install** downloads one file, checks it hashes to what the list said, checks the
  module's own manifest against what the list promised — an index is a mirror and never the
  authority — and then *runs it*, in the window that is already open. Every press is one
  request: **Update**, **Update all** (one module at a time), and **Fetch pictures** — a
  plugin's screenshots are inside its module, so seeing them before installing is fetching
  the same file Install would, which the card says beside the button, and Install afterwards
  needs no second download. A version the registry
  **withdraws** stops running at the next launch, with the sentence whoever withdrew it wrote on
  its card; that is read off the list already on this machine, so it holds offline, and a list
  that cannot be read withdraws nothing. Installing is still not
  allowing: a plugin that has just arrived may do nothing at all until its card in **Plugins**
  is answered, and **Remove** takes it and its permissions back off.

  **Writing one is a flag and a template.** `crook --dev-plugin <path>` runs the module you are
  working on straight out of `target/`, and runs it again every time you build it: no install,
  no copy, and nothing left on the machine when the window closes. A build that does not compile
  is a line in the log and the version before it still running. What to point it at on the first
  afternoon is
  [github.com/theguriev/crook-plugin-template](https://github.com/theguriev/crook-plugin-template)
  — five exports, a manifest that asks for nothing, and one shape to change.

  **The second worked example is the chips**, and it is the one that proves a plugin may
  *act*. It lives at [github.com/theguriev/crook-chips](https://github.com/theguriev/crook-chips)
  and draws the row under the line you are typing: where the pane is, which branch it is on,
  how much has changed, and what one chord would do. Pressing the first opens a directory
  picker; pressing the second opens a branch picker; choosing a row in either **types the
  command into your shell** — `cd …` or `git switch …`, quoted by the host, and only ever
  those two, because those two strings are what it asked to be allowed. The third chip runs
  the command it names, and its secondary click offers to change the chord — which opens
  Crook's own Keyboard Shortcuts page with that row recording, not a recorder of the plugin's. The field, the filtering, the arrow keys, Enter and
  Escape all belong to Crook: the plugin says what can be chosen and is told which row a
  person chose, so a chip with a search box in it is never handed a keystroke.
- **An agent that says what it is doing.** The dot on a tab's row is written by the program
  in the pane, over the one channel it already has: `crook --agent running`, `needs-input`,
  `failed` or `idle`, with `--title` for what it calls its work, writes one escape sequence to
  its own terminal and exits. `needs-input --message "run rm -rf build?"` says what it is
  waiting for, `failed --message` what went wrong, and the row prints that under the title
  until the agent gets on with it — so you know whether to come now or later without
  switching to the tab; `--message -` reads it from stdin, a hook's JSON `message` or the
  whole line. No socket and no pane id — the terminal it has *is* the pane — so the command
  works from a hook, over `ssh` and inside a container, and every other terminal drops the
  sequence unread. A hook with no terminal of its own —
  every hook Claude Code runs — writes to the terminal of the program that ran it: the same
  pane. Inside tmux in a pane, which keeps a sequence it does not know to itself, it goes round
  tmux to the terminal of each tmux client attached to the agent's session — the pane among
  them, and any other terminal drops it unread — as `tmux` names them, and a client whose
  terminal is not being read is given a fifth of a second. Written beside tmux's own output, a
  report can land in a sequence tmux was cut off in the middle of writing to a terminal that
  reads slowly, and leave a few stray characters until tmux redraws them. tmux's own
  passthrough, a sequence wrapped for tmux to pass on in step with what it draws, is no cure
  for that, whatever `allow-passthrough` says: tmux throws a passthrough away with everything
  else it has yet to write to a client that has fallen that far behind, and never sends it
  again, so on that terminal the report would be lost rather than followed by a few stray
  characters. A report is wrapped that way only where `tmux` cannot be asked or no client takes
  it, and then only if `TERM_PROGRAM` says tmux, as it does from 3.2 on; a tmux older than 3.3
  passes the wrapper on regardless, and 3.3 and later only where `allow-passthrough` is `on`,
  from a pane on screen, or — from 3.4 — `all`. Claude Code's own notifications (OSC 9, below)
  are its own to send, and inside tmux they reach the row only as far as tmux passes them on,
  which takes the same option. Claude Code says all of it by itself — running when a prompt
  is sent and around every tool, needing input whenever it stops to ask, with the notification's
  own text as the message, idle when it is done, failed with the error when an API error such
  as a usage limit ends its turn — once Crook's plugin is installed:
  `claude plugin marketplace add theguriev/crook`, then `claude plugin install crook@crook`,
  with a Crook newer than 0.1.13. Claude Code keeps the plugin as it installed it until it is
  told otherwise — auto-update is off for a marketplace like Crook's — so after updating Crook
  run `claude plugin update crook@crook`, or turn on auto-update for the crook marketplace in
  Claude Code's `/plugin` › Marketplaces; a plugin from before 0.1.29 has no permission hook,
  and nothing it asks reaches Crook. Codex CLI and GitHub Copilot CLI install the same plugin from
  the same marketplace — `codex plugin marketplace add theguriev/crook`, then
  `codex plugin add crook@crook`; `copilot plugin marketplace add theguriev/crook`, then
  `copilot plugin install crook@crook` — and each reads a manifest of its own there, which names
  hooks under that agent's own event names: Codex's say needs-input, with the tool, when it
  stops for an approval, and Copilot's name their session, below, which a Crook older than
  0.1.27 does not read and reports nothing for. That is each agent writing its own settings,
  not anyone merging JSON; the plugin's hooks call `$CROOK_BIN`, do nothing in any other
  terminal unless it runs tmux, and exit 0 even when a report fails, so a Crook that cannot be
  reached is a line in the agent's log rather than a notice on every prompt; it carries the
  skill below too. Codex runs the plugin's hooks, as it runs any hooks, only once you trust
  them: it asks on its next start, or in its `/hooks`, and asks again whenever a hook's command
  changes.
  `crook --agent-hooks claude`, `codex` or `copilot` prints those two commands, then the same
  hooks for a person who would rather set them up by hand: JSON to merge into
  `~/.claude/settings.json` or `~/.codex/hooks.json`, or a file to save as
  `~/.copilot/hooks/crook.json`.
  With no hooks at all Claude Code still says half of it, in the glyph it puts in front of its
  own terminal title — `✳` at its prompt, a turning half circle while it works — which the row
  reads as idle and running, wearing the title without the glyph; a hook's report outranks it,
  but for the `✳` an Esc leaves, which no hook reports and which takes a hook's running back.
  `--agent-hooks gemini` prints the same guarded hooks for Gemini CLI, which has no plugin, to
  merge into its settings, `opencode` prints the plugin OpenCode loads instead, and `aider`,
  which has no hooks, gets a sentence saying what to do instead.
  A subagent works in a session of its own, and Copilot CLI runs its hooks for one as for the
  session you are talking to, so its hooks pass `--session -`: the pane takes a title or a stop
  only from the session whose turn it is, until that one stops, another takes the pane or its
  command ends. Its `sessionStart` hook, which no subagent runs, passes `--session-start -`. It
  runs once a session in the life of the process, so the pane remembers every session that
  started until Copilot exits. After a turn interrupted with Esc, the next turn takes the pane
  whether it is in a session `/new` began, one `/resume` brought back from disk, or one this
  Copilot has already run. Every session that never started is a subagent's, including a
  background one still working after the agent stopped. A subagent's prompt no longer renames the
  tab, and its end no longer wakes a `crook pane wait --until idle` partway through the turn; its
  questions still light the row. Nor does the `<system_notification>` prompt Copilot writes when
  a background subagent finishes rename the tab. The OpenCode plugin
  leaves its subagents out itself, but for their questions — asking OpenCode about a session it
  did not see made, such as a subagent's that `task` resumes by its id — takes no title from
  text OpenCode wrote itself, and sends its reports one after another, each only when it says
  something new. Copilot CLI refuses the tool call a hook failed over, so no hook ever fails:
  outside Crook, with no terminal to report to, or with the binary gone, each one exits 0.
  Outside Crook means where
  neither `TERM_PROGRAM=Crook` nor `CROOK_PANE_ID` reaches and nothing runs inside tmux. Inside
  any tmux the hooks run, since only tmux can name the terminals its clients are on:
  `crook --agent` asks it for them and writes the report to each, and one that is not Crook
  drops it unread. `ssh` and `docker exec` carry neither variable, so an agent on the far side
  of either reports nothing through these command hooks, the plugin's included,
  until two things hold there.
  `TERM_PROGRAM` is passed on: `SendEnv TERM_PROGRAM` in the ssh client's configuration with
  `AcceptEnv TERM_PROGRAM` in the server's, or `docker exec -e TERM_PROGRAM`. And the hooks find
  a `crook` on that machine, since they carry no path of their own: one on its `PATH`, or
  `CROOK_BIN` set there to its path, as in
  `docker exec -e TERM_PROGRAM -e CROOK_BIN=/opt/crook/crook` for a binary mounted off `PATH`.
  The OpenCode plugin and aider's command have no guard: they run the binary by the path they
  were printed with wherever they are loaded, so they report over `ssh` and from a container
  with nothing passed on.
  `--pull-request <url>` beside any status says which pull request the work is, and the row
  carries it as a chip that opens it — https only, capped, on a sequence of its own,
  `OSC 6342` — until the pane moves to another branch (a rebase is not a move); the Claude
  Code and Codex hooks send the address a `gh pr create` printed, with no second hook and no
  `jq`. Crook asks no forge to find it. The chip reads `PR #123` for github.com and names the
  host anywhere else, and the hover card prints the whole address, since any program in the
  pane can write the sequence.
  **Check pull request** on the row's menu is the one place Crook asks the network about your
  work: it runs your own `gh pr view <url> --json state,isDraft,statusCheckRollup` once, on that
  press, under a deadline — no token of Crook's, no timer, no poll — and the hover card says
  open, draft, merged or closed and how the checks stand until you press again, or that `gh` could
  not be found, is not signed in, or could not reach GitHub. `gh` is looked for on Crook's
  `PATH` and then where Homebrew, MacPorts, gh's own package and `~/.local/bin` put it, so a
  Crook opened from the Dock finds it too.
  `crook --skill` prints the skill file that teaches an agent the rest — how to tell it is in
  a pane, what the four words do, where the worktrees and the plugins are — to save as
  `~/.claude/skills/crook/SKILL.md`.
  **And the window answers.** `crook pane list` asks the window a pane is in what it has open
  and prints a row per pane: its number (the one in `CROOK_PANE_ID`, the focused one marked
  `*`), what its agent said, its title, group, branch and directory, and under it what a
  waiting agent is waiting for; `--json` prints the window's own array, for a script or
  another agent. It goes over a Unix socket in a directory only you can enter, named in every
  pane's `CROOK_SOCKET` — never a network port — and it only reads. Outside a pane it asks
  the one Crook that is running and refuses to guess among several. Not on Windows yet.
  **And an agent can fan its work out where you can see it.** From inside a pane,
  `crook tab new --worktree fix-x --in-my-group -- claude "fix the flaky test"` opens a tab
  beside it — in a new worktree on `fix-x` from the pane's `HEAD`, locked and given what
  `.worktreeinclude` names as one made from the menu is, folded into the pane's group —
  without taking your keyboard, and runs the command at the new shell's first prompt, every word
  as itself; it prints the new pane's number for `crook pane list` to watch, and on standard
  error what the worktree was not given, if anything. A pane is known by
  the secret in its own `CROOK_TOKEN`, so a script that is not in a pane can list but cannot
  open anything; eight tabs may be open on behalf of one pane you opened, a worker's own workers
  counted in, and the new row's card says which pane opened it. `sh`, `bash`, `zsh` and `fish`
  only.
  **And wait for it, and read what it did.** `crook pane wait 7 --until needs-input --timeout
  110` blocks until pane 7's agent stops for a person — or is `idle`, or its command has
  `finished`, or the pane has `exited`, even before you asked — and prints where it got to,
  failing when the time runs out first, so an agent whose tool cuts a command off after two
  minutes waits again rather than lose the wait; `crook pane blocks 7 --last 1` prints its newest
  finished command with what it printed, its exit status, how long it took and where, the end of
  a long output kept and marked cut; `crook events --follow` is a line of JSON for every status,
  every command finished (named, with its exit status) or seen running, and every tab opened and
  closed, and a reader that falls behind is told how many it missed rather than held for. A
  pane may watch itself and the tabs it opened, and nothing a person opened: that needs a grant
  Crook cannot ask for yet. A command still running has not finished, and an interactive agent
  is one live screen with no finished command to read: `pane blocks` says so rather than
  printing nothing, and a worker whose answer is meant to be read runs headless, `claude -p`.
  Crook also reads the notifications other terminals show — OSC 9, 777 and 99 — as the pane
  asking for a look, with the notification's text on the row and the status left as it was, so
  an agent on a remote box with its notification channel set to `iterm2`, `ghostty` or `kitty`
  lights its row with no Crook binary there. Claude Code's channel starts as `auto`, which picks
  one of those by the terminal's name and sends nothing in Crook: without the plugin, set
  `"preferredNotifChannel"` in `~/.claude/settings.json`, or in its `/config`, to `ghostty`,
  `kitty` or `iterm2` — whichever your other terminal is, since the setting holds in every
  terminal — and a stop to ask lights the row. `crook --agent-hooks claude` says so too.
  A notification over a failure the agent reported is dropped, so the row keeps what went
  wrong: with the plugin installed too, it is Claude Code's idle reminder, sent both ways.
  A status the agent never took back goes when the shell's own marks say the command ended,
  and a failure stays on the row until the next command starts. Looking at a tab clears the
  *attention* it asked for and nothing else: an agent waiting for an approval is still waiting
  after you glance at it. Looking means the pane has the keyboard *and* the window is the one
  in front: in a window behind another application even the pane with the keyboard is nobody's,
  so the agent in the only pane you have is waiting like any other when it stops to ask, and
  coming back to the window is the glance. A row that is waiting for you is washed amber, the
  header counts them in a chip that goes to the next one when pressed, and `cmd-j`
  (`ctrl-shift-j` off macOS) does the same from the keyboard, round the list in the panel's
  order. The window is named after the tab it shows, with that count in front —
  `(1 waiting) bisect the flaky test — Crook` — so a switcher or a taskbar tells three Crooks
  apart and says which one stopped for you. When one more starts waiting while the window is
  behind something else, Crook asks the desktop to point at it: the dock icon bounces once on
  macOS, the urgency hint goes up on X11, an activation request goes to a Wayland compositor
  that takes them and the taskbar button flashes on Windows — a request for a look, not a
  notification, and it is taken back when the window comes to the front. On Linux, and on macOS
  from Crook.app, a pane whose row turns amber while the window is behind something else — its
  agent stops to ask or says it is done, or a program rings the bell — also posts a desktop
  notification, titled `Crook — <tab>` with the agent's question under it, at most once every
  thirty seconds for a pane nobody has come back to, through `notify-send` on the session bus or
  a Mac's Notification Center (which asks you once, the first time a pane waits for you or
  Crook has something to post) and nowhere else; the **Notifications** settings page turns it
  off, the question included, and turns on the same for an agent that failed or a command that
  ran ten seconds or more. A click on one does not bring the pane forward. Windows posts none
  yet, and nor does a Mac binary outside Crook.app, since macOS delivers only to an app.
  Crook.app also puts the waiting count on its dock icon as a badge, under the same permission
  as the notifications and its Badges switch, so with notifications off there may be none; a Mac
  binary outside Crook.app is given the count too, and whether the dock shows it is unverified.
  A program that turns on focus reporting (`?1004`) is told `CSI I` and `CSI O` as the keyboard
  reaches its pane and leaves it, the window's own focus included, so an agent can tell whether
  anybody is watching.
- **A shell in every pane.** A real pseudo-terminal and a real xterm-compatible emulator:
  colour, bold and italic faces, underline and strikeout, the alternate screen, ten thousand
  lines of scrollback, `SIGWINCH` on resize, and titles and working directories the shell
  reports with OSC 0, 2 and 7 — which is what makes a tab rename itself and its git chips
  follow a `cd`. A tab splits into panes with `cmd-d` and `cmd-shift-d` (`ctrl-shift-d` and
  `ctrl-shift-e` off macOS), to the right and downwards, and a split always divides the
  *focused* pane the way it was asked: split right and then down, and the right half is two
  panes stacked beside a left one the full height of the tab — an agent with two watchers —
  and the same on the left half makes a 2×2. Splitting again the way a pane's own split
  already goes adds a pane to that row or column rather than halving the pane, tmux's rule,
  so three splits right are four even columns. Every divider drags, at every depth, moving
  only the two panes or columns either side of it, and a double click evens the whole tab
  out; closing a pane gives its room to the pane or column beside it. `cmd-shift-m`
  (`ctrl-shift-m`) gives the focused pane the whole tab and the split back on the next press,
  tmux's zoom, and a shell that exits closes its pane, its tab, and with the last tab the
  window. With shell integration the scrollback
  moves into the blocks: each finished command owns its rows, so ten thousand *commands*
  survive a `clear`, a resize, and the emulator's own history evicting anything. What empties
  a pane is **Clear blocks** — `ctrl-l` at the prompt, or `shift-cmd-k` (`ctrl-shift-l` off
  macOS) from anywhere — which drops the finished commands and the scrollback and keeps the
  prompt, or the command still running, along with the line being typed. Its bookmarks, its
  selection and its find bar go with them. Nothing asks first and nothing brings them back;
  `cmd-k` stays the tab search.
- **Output as a list of commands.** A pane is not a grid with decorations drawn over it: each
  command is a block holding its prompt, the line that was run and everything it printed, with
  a hairline between one and the next, a red wash on one that failed and an accent stripe on
  one that is still running. Hovering a block reveals a control that copies exactly that
  command and its output — no neighbour's text, no trailing blank rows, no over-selection —
  which is the thing scrollback cannot do. The list virtualises: a frame costs a screenful,
  not a session. **This needs shell integration**, which Crook installs into zsh, bash and
  fish by itself; see below for what a shell without it looks like.
  [`docs/blocks.md`](docs/blocks.md) is the map of the whole surface.
- **Output you can select and copy, across every command in it.** Drag across a pane's output
  to select it, double click for a word, triple click for a line, alt-drag for a column; the
  drag keeps going when the pointer leaves the pane, a drag past the top or bottom edge scrolls
  the list under the pointer, and the selection stays on its own text while the shell prints
  more underneath and while the list scrolls over it. **A selection is a block, a row of that
  block and a column** rather than a cell of the grid, so it spans the commands that have
  finished as well as the one still running: a drag from one block into the next copies the
  ends partially and the blocks between them whole, joined with one newline and none of the
  padding between them. A line the terminal folded comes back as the one line it is, trailing
  blanks stay behind, and a double-width character copies as one character.
  `cmd-c` — `ctrl-c` or `ctrl-shift-c` off macOS — copies it and lets it go, so the next
  `ctrl-c` interrupts the shell the way it always has. The half-written command line in the
  field below is left exactly where it was: a copy is not an interrupt. Copying a *whole*
  finished block still needs no selection at all: that is what its hover control is for, and it
  takes exactly what dragging across that whole block takes.
  A pane that is drawing one grid rather than a list — the alternate screen, or a command that
  has printed past the top of the viewport — is selectable in the same way, over its scrollback
  as well as its screen. What no selection survives is the picture under it changing: resizing
  the pane re-wraps the rows, and crossing between the list and the grid renumbers them, so the
  selection is let go of rather than re-read against text nobody selected.
- **The URLs and paths a program prints are links.** Hold `cmd` (`ctrl` off macOS) over one and
  it underlines; click with it held and it opens. A URL goes to your browser. A path — what
  compilers and agents print most: `--> src/main.rs:42:5`, `Update(app/src/view.rs)`,
  `src/app.ts(42,5)`, `./README.md`, `/etc/hosts`, `~/notes.md` — is read against the directory
  its command ran in (a finished block keeps its own; a running one is where the shell last
  said it was), and opens in your `$VISUAL` or `$EDITOR` at the line it names, the same editors
  the Changes column's **Open** knows; with no editor that opens a window named, the system's
  own handler opens the file, at the top. A folder opens in the file manager. A path is a link
  only once its file has been found — looked for off the thread that draws, a beat after the
  key goes down — so `and/or` in a sentence never underlines. A program (a file marked
  executable, an `.exe`, an `.app`, a `.jar`, a script Windows runs by its name such as `.sh` or
  `.py`, or a link to any of them) is opened only in the editor, never by the system's handler,
  which would run it. A path printed on the far side of an `ssh` is no link at all: it names a
  file on that machine, and this one's at the same path is not it.
- **Find across every command in the output.** `cmd-f` (`ctrl-shift-f` off macOS) opens a bar
  over the top-right of a pane and searches the blocks, not the screenful — every finished
  command and the open one at once. Each match is highlighted where it is, the current one in
  the accent and the rest in amber; the bar counts them, Enter and Shift-Enter step between
  them and scroll the current one into view, and Escape hands the keyboard back to the shell
  with the query kept for next time. Two switches on the bar change how it matches: `Aa` tells
  capitals apart, and `.*` reads the query as a regular expression — one that does not compile
  says so on the bar instead of counting no results. `cmd-alt-c` and `cmd-alt-r` (`alt-c` and
  `alt-r` off macOS) flip them without leaving the query, and each switch names its chord under
  the pointer. It is a search, not a filter: the lines between the hits stay where they are,
  because the output is a transcript. The filter is a block's own: **Filter output** on its
  menu, or `filter-block` on the selected one, opens the same bar filtering that block, which
  then shows its command and only the lines of its output that match — the bar counts them,
  `2/4 lines`, with the same two switches — until the bar closes or its filter switch goes
  back to finding. A copy of a filtered block takes the lines it shows: a drag over it, its
  copy square, and its menu's **Copy shown lines** and **Copy shown output**. A pane drawing
  one grid rather than a list — an agent session past its first screenful, an `ssh` session, a
  shell without the integration — gets the same bar over its whole scrollback as well as its
  screen, matched by the same rules and counted the same way, and a step scrolls the grid to
  the match. It is searched off the thread that draws, so on ten thousand lines of scrollback
  the count can come a moment after the keystroke. A full-screen program on the alternate
  screen keeps no scrollback for the terminal to search — Claude Code's fullscreen renderer
  keeps its history itself — so there the bar reads **Find on screen** and searches what the
  program has drawn; it closes when the program takes or gives back the screen, and the
  keyboard goes back to the prompt. While the bar has the keyboard, a word an input method
  composes goes into the bar, and a key the bar has no use for does not reach the program under
  it — except `ctrl-c`, `ctrl-d` and `ctrl-z` — while the copy chord over a selection in the
  output, `ctrl-c` included, still copies it.
- **Step through the commands with the keyboard.** `cmd-alt-up` and `cmd-alt-down`
  (`ctrl-alt-up` / `ctrl-alt-down` off macOS) move a selection through the finished blocks:
  up from the prompt lands on the last command, up walks to the oldest, down walks back and off
  the last block returns to the prompt. The selected block wears an accent wash and a stripe,
  and steps to an off-screen one scroll it into view. `cmd-c` (`ctrl-shift-c` off macOS) copies
  the whole selected block, and Escape lets the selection go. It is one selection per pane, and
  it answers the same question a drag does, so the two never both hold: stepping to a block
  drops any text the pointer had selected.
  A block scrolled into its middle — its top gone above the pane, its command with it — has
  that command **pinned over the top of the list** until its top comes back, the open block
  included, so a build or an agent printing screens of output still says what it is; a press
  on the pin scrolls to the block's first line.
  A block worth coming back to can be **bookmarked** — from its menu, or `bookmark-block` on
  the selected one — and wears a short accent flag down its right edge. `previous-bookmark`
  and `next-bookmark` select the nearest bookmarked block above or below and scroll it into
  view; from the prompt, `previous-bookmark` is the newest one. Bookmarks last as long as the
  block is in the pane's history.
- **A command line that behaves like a text field.** Under each pane's output is the line
  being composed — not a box and not a raw terminal line: no border, no fill, no focus ring,
  on the pane's own ground, in the terminal's own font and colours, at the same column zero as
  the output above it. It is an editor: a caret you can click, selection by drag, double and
  triple click, word and line movement, undo, the system clipboard, a per-pane history on the
  up and down arrows, and multi-line commands with shift-enter. Enter sends the line to the
  shell, which echoes and runs it exactly as before. A full-screen program — vim, `top`,
  `less` — takes the whole keyboard back, and so does any command that has been running for
  longer than a blink: the field goes away and its space goes to the block. A word an input
  method is composing for that program — Japanese, Chinese, Korean, a dead key's accent — is
  then drawn underlined at the program's own cursor, even one it keeps hidden as Claude Code
  does, with the candidate list beside it, and reaches the program only once it is committed.
  `ctrl-c`, `ctrl-z` and an end-of-input `ctrl-d` always reach the shell. So do the keys your
  shell's own line editor answers and the composer does not, pressed at an empty composer:
  `ctrl-r`'s history search, fzf's `ctrl-t` and `alt-c`, `alt-b`, `alt-f`, `alt-d` and `alt-.`
  off macOS, and Escape for vi mode. The composer steps aside for the search or the picker,
  and every key goes to the shell until it has finished a line and is back at its prompt
  with nothing typed: the line ran, or in bash and zsh an Enter or `ctrl-c` ended it. What
  you type while that line runs goes to the shell as well, which puts it on its next prompt,
  and the composer waits for that line too; typing into vim, a REPL or Claude Code run from
  there does not count. A search you give up leaves the shell's own empty line in charge
  until then, and in fish, which reports nothing for an empty Enter, until the next command
  runs. That line is drawn
  in one function, `app/src/input_keys.rs`, and the architecture doc's §7 says why it is
  drawn there.
- **Option types on a non-US Mac.** In vim, Claude Code or any program holding the keyboard,
  Option is Meta (Esc in front of the key) except where your layout puts an ASCII symbol on it:
  German Option-L types `@` and Option-8 types `{`, British Option-3 types `#`, and US Option-F
  is still Meta-F. Settings → Shell → *Option as Meta* (`option_as_meta` in the settings file)
  makes the left, the right or both Option keys Meta, or neither. An Option combination your
  layout makes a dead key (US Option-E or Option-U, German Option-N for `~`) starts an accent
  under every choice, because macOS takes it before Crook sees the key.
- **The shell your other terminal gives you.** A pane starts a *login* shell, which is what
  Terminal.app, iTerm2 and WezTerm start and what `login(1)` itself starts. That is the
  difference between the environment you configured and a subset of it: only a login shell
  reads `/etc/zprofile`, `~/.zprofile` and `~/.zlogin` on zsh, `/etc/profile` and
  `~/.bash_profile` on bash, and only a login shell makes macOS run `path_helper` — the thing
  that builds `PATH` out of `/etc/paths` and `/etc/paths.d` at all. So the ASCII art in your
  `~/.zprofile` appears, `which` answers the way it does next door, and you get the same
  version of every tool. See "Which files your shell reads" below for the details, including
  the switch that turns it off.
- **Shell integration, installed by itself.** A pane running zsh, bash or fish emits the four
  OSC 133 marks that say where a prompt starts, where a command starts and how it ended. There
  is nothing to install and nothing to configure: Crook writes a scratch `ZDOTDIR`, `--rcfile`
  or `vendor_conf.d` stub, chains onto whatever hooks are already there, never touches
  `~/.zshrc`, and removes the stub when the pane closes. The stubs sit in a directory only you
  can read — `$XDG_RUNTIME_DIR/crook`, or `crook-<uid>` in the temporary directory, and on
  Windows `crook-shell-integration` in your own `%TEMP%` — and a pane whose directory is
  someone else's runs without the marks rather than use it. Set `CROOK_NO_SHELL_INTEGRATION` to
  anything but `0` to turn it off. It reaches only shells Crook itself starts — not the far
  side of an `ssh`, not a container, and not a shell it has no snippet for (`pwsh`, `nu`,
  `ksh`, `tcsh`) — and on those machines `crook --shell-integration zsh` prints the same text
  to paste at the end of the rc file by hand. Pasted there, it reports the directory with the
  name of the machine it is on, as fish 4 does by itself, and **a pane whose shell is on another
  machine reads nothing of this one's**: the path it reports names a directory over there, and
  the checkout this machine may have at the same path is not the one the shell is in. So its row
  leads with that machine's name, muted, and shows no branch and no diff counts; the Changes
  column says where the pane is instead of reading; the worktree menu is not offered on it; a
  plugin asking where the pane is is told nowhere; a tab or a split opened from it starts where
  a new tab starts; and a command run there offers no branch in its block's menu, and no path
  it printed is a link — relative, absolute or under `~`, each names a file over there — though
  its URLs still are. The session file remembers the machine, so the pane comes back as a new
  shell here rather than one started in this machine's copy of the far side's directory. The
  name is the one the system gives, read once as the shell starts, not `$HOST` or `$HOSTNAME`,
  which a startup file can set to anything. The shell is on this machine when the name is this
  machine's own, `localhost`, a loopback address or none at all, which is what every shell
  Crook starts sends. Settings → Shell says which shell a pane starts
  and whether it gets the marks, which is where to look when the output is one long block;
  `crook --shell <path>` starts another shell in every pane, for trying one out. Marks or no
  marks, every shell Crook starts has `TERM_PROGRAM=Crook` and `CROOK_PANE_ID` set to the
  pane's number — what WezTerm's `WEZTERM_PANE` is — so a script or an agent can tell it is
  inside Crook and which pane, and name a log file after it — and `CROOK_SOCKET`, where the
  window answers `crook pane list`, with `CROOK_TOKEN`, the pane's own secret that
  `crook tab new`, `crook pane wait`, `crook pane blocks` and `crook events` send to say which
  pane is asking. `CROOK_BIN` is beside them: the absolute path of the `crook` binary the pane
  belongs to, so a hook can call it from a `PATH` it is not on — a Crook.app nobody linked, a
  build under `target/`. And `crook` itself is on the pane's `PATH`: when no `crook` answers
  there, that binary's directory goes at the end — never the front, so it shadows nothing. Every
  shell is handed it in the environment it starts with, which a startup file that execs fish or
  starts tmux passes on, unless the directory is a `bin` in your home: your own files place that
  one, and one that adds `~/.local/bin` only when it is missing has to find it missing to put it
  first. The zsh, bash and fish integration adds it again after your startup files have run, so
  one that sets `PATH` outright cannot drop it, and neither can an older copy of the snippet
  pasted into them. Windows panes keep the `PATH` the registry gives them. Every pane also has
  `CLAUDE_CODE_FORCE_STRIKETHROUGH=1`: Claude Code strikes `~~deleted~~` text through only in
  terminals it knows by name and prints the tildes everywhere else, and Crook draws it.

  It also **answers**, which is what makes Tab work. Command marks are an announcement and
  completion is a question, so there is a second channel beside them: Crook writes the line
  into the pane's own scratch, sends a key the snippet bound, and the snippet writes the
  answer back and says so with an escape sequence carrying only the request's number. fish
  answers with `complete -C`, which is its real completion; bash with `compgen`; zsh with its
  own hashes and globs. One candidate is typed whole and several type as much as they agree
  on, which is what every shell's Tab does — and what is still ambiguous after that is
  **offered rather than listed**: the first candidate stands after the caret in dim ink, Tab
  steps to the next and Shift-Tab back, the right arrow takes it, and typing on rules out the
  candidates that no longer match without asking the shell anything. There is no menu and no
  panel. A list under the composer is a surface that appears and disappears under whatever a
  person is reading, and it takes the output with it every time.
  **Without it Crook is a plain terminal**: one continuous stream of output drawn as a grid,
  scrolled through the emulator's own scrollback, with every key going straight to the shell.
  No blocks, no per-command copy, and no composer — everything else, including selection and
  copying, works exactly as it does with it.
- **The rest of the command, before you type it.** A pane opens with your shell's own history
  behind it — zsh's, bash's or fish's file, read once and never written — so the up arrow in a
  fresh tab reaches yesterday's commands, and the newest one that starts with what you have
  typed stands after the caret in the same dim ink a completion does. The right arrow takes
  it, the word arrow takes one word of it, and typing anything else leaves it behind. It is
  drawn rather than typed: nothing is in the line until you take it, and it never makes the
  composer grow a row. Settings → Shell → *Tab key* makes Tab take it too; with nothing
  standing after the caret Tab still asks the shell, and `ctrl-space` asks the shell whatever
  is standing.
- **Themes**, in a panel of their own. Thirteen built in — Crook Dark, Crook Light, Midnight,
  and ten of the palettes Omarchy dresses a desktop in (Catppuccin, Everforest, Gruvbox,
  Kanagawa, Nord, Rosé Pine, Tokyo Night and three more, each its own project's, read from
  Omarchy's files) — plus any number of your own from `<config>/crook/themes/*.yaml`, in
  **Warp's own theme file format**, so a theme written for Warp works here unchanged.

  A theme names a background, a foreground, an accent and the sixteen ANSI colours; everything
  else — surfaces, borders, overlays, muted text — is derived from those, which is why a theme
  file is twenty lines rather than sixty. Choosing one applies it to the chrome, to the grid
  and to every shell already running. A program that asks whether the terminal is dark or
  light (`CSI ? 996 n`) is told, from the background, and one that asked to hear of a change
  (mode `?2031`, which Claude Code, OpenCode and Copilot set) is told when a new theme, or the
  desktop's light and dark pair, takes the background from one to the other — so an agent that
  matches its colours to the terminal's matches the theme it is on now, not the one it started
  under.

  The **Themes panel** is a second side panel, opened from the settings page's current-theme
  row: preview cards, arrow keys that browse by applying, and a `+` that **makes a theme** out
  of the one you are looking at — five candidate colours clustered out of its palette, one
  click to choose the background, and everything else decided so the result is legible. What
  it writes is a file in your themes folder, in the same format as any other.
- **A tab's own menu.** Right-click any row and it opens over that row: pin tab, mark tab as
  waiting, why this status, new group with tab, copy pane title, copy working directory, copy
  git branch, rename tab, rename pane, close tab, close other tabs, close tabs below, close tabs
  above, a row of colours, and — inside a repository — show changes and the worktree list, with
  Finish task and Discard task beside them on a tab in a checkout Crook made; open pull request
  and check pull request join them on a row whose agent has named one, and Answer… on a row
  whose Claude Code is asking for a permission. **Answering** is the same as the palette's
  answer waiting agent, `cmd-i` (`ctrl-shift-i` off macOS) and the header's "to answer": a
  card with the whole of what Claude Code asks — a Bash command, or an Edit, MultiEdit or Write
  drawn as a diff of the file — its tab and directory, and Allow once, Deny, Go to tab and Not
  now. Not now is the default, the other three wait a second before a key or a press reaches
  them, and Claude Code's own dialog stays up in its tab, so whichever is answered first wins;
  nothing Crook does by itself ever allows. With several requests held it opens a list of them,
  oldest first, Enter opening one's card and Escape on the card coming back, with how the last
  ones ended under it; with none, the list says why — and when a tab's Claude Code says it
  needs a permission and handed Crook nothing, that either the request is one Crook does not
  show (a tool other than Bash, Edit, MultiEdit or Write, one too large, one from inside screen
  or zellij, or one that waited out its five minutes) or the plugin predates the permission
  hook and needs `claude plugin update crook@crook`. It needs Crook's Claude Code plugin, or the hooks `--agent-hooks claude`
  prints, from 0.1.29 on, and "Let agents ask in Crook" in Settings › Appearance turns it off.
  **Marking** a tab washes
  its row amber and counts it with the ones waiting for you, for a tab you mean to come back
  to: it stays until you arrive at the pane or clear it from the same entry, and it never
  answers for the agent. **Why this status** hangs a panel off the menu that says in words
  what the dot cannot: what the agent said and when, what asked for a look — a bell, a
  notification, a mark — and what the shell is running. The three closes after close tab are
  Chrome's, read down the list rather than across it: they leave pinned tabs where they are,
  and like any close they ask first when they would end something still working.
  **Pinning** holds a tab at the front of the block it is in rather than of the whole list,
  which is the one place this cannot be Warp's: a group is a contiguous block that says two
  checkouts are one piece of work, and pinning that lifted a member out of the middle would be
  pinning that takes a group apart. A drop can no more land an unpinned tab among the pinned
  ones than it can split a group. A **colour** is a stripe down the leading edge of a tab's
  rows, not a tinted status disc — the disc says what the agent is doing, and one dot cannot
  carry both — and it is named rather than written down, so a tab made red in one theme is red
  in a theme written years later. Renaming turns the entry itself into a field, in the
  column you pressed it in; Enter keeps the name, Escape drops it, and an emptied field puts
  back the name the tab was opened with. A name you typed beats the one the agent chose for
  its own work, which is the whole point of typing one, and it comes back with the window. Not one of those entries is written into the menu. It is a
  [slot](docs/plugins.md), `tab.menu.entries`, and every row in it is a contribution: they
  come from three plugins today, each entry is also a named command the palette lists and a
  chord can reach, and a plugin outside the binary puts a row there the same way. Escape is
  one step back — out of the submenu, then out of the menu.
- **Git worktrees, one entry away.** Open that menu on a tab inside a
  repository and `Worktrees` lists that repository's checkouts: the one this tab is in, the
  ones other tabs are in, and the rest. Choosing one opens a tab there — or brings forward the tab
  already in it, because two agents editing one checkout is exactly what a worktree exists to
  prevent. `New worktree…` asks for a branch name, fills one in that nothing is using, shows
  where the checkout will go, and asks where the branch starts: this tab's own branch, already
  picked, the repository's default branch (`origin/HEAD`, else `init.defaultBranch`, else
  `main` or `master`) one arrow below it, and every other local branch after that. Then it
  opens a tab in it — folded into a group with the tab that asked for it. Under all that it
  offers **an agent to start there**: the coding agents it finds installed (`claude`, `codex`,
  `gemini`, `copilot`, `opencode`, `aider` — looked for on `PATH` and where their own installers
  put them, never run to ask), with `Shell only` first and picked, which makes exactly the
  worktree it always made. Pick one and a
  prompt field appears; the branch is named after the prompt as you type it
  (`worktree/fix-the-login-bug`) until you type a name of your own. Create, or Enter from either
  field, opens the tab with the agent's line — `claude 'fix the login bug'`, the prompt quoted
  as one word for the shell — **in the composer, unsent**, for you to read and send; `Start`
  sends it for you. Tab moves the keyboard between the name and the agents, and the arrows walk
  the list on whichever side has it, so every agent is a Tab and an arrow away from `Shell
  only`. `New task…` in the palette (`crook/worktrees/new-task`) is the same creator with the
  first agent found already picked and the keyboard in the prompt, so a task is a sentence and
  Enter; it ships with no chord, and `{ "key": "cmd+shift+n", "command":
  "crook/worktrees/new-task" }` (`ctrl+shift+alt+n` off macOS, where `ctrl+shift+n` opens a
  new window) is one that nothing else takes. In a
  shell whose quoting Crook has not proven — anything but sh, dash, bash, zsh and fish, and
  every shell on Windows for now — the line is the agent's name alone and there is no prompt
  field. A repository with a [`.worktreeinclude`](https://code.claude.com/docs/en/worktrees) at
  the root of its main checkout — Claude Code's file, in `.gitignore` syntax — gets the files it
  names copied in before that tab's shell starts, so the `.env` an agent's first run needs is
  there: only files git ignores (a tracked file is already in the checkout), and only where the
  new checkout ignores it too, so an agent's `git add -A` cannot commit it; never into one of
  its submodules, never through a symbolic link, never over a file already there, nothing at all
  past a thousand files or 256 MB, and no single file over 64 MB. What did not arrive is said on
  the new tab's worktree menu. A checkout made that way is locked (`crook: <branch>`) until no
  pane in the window is working in it any more, or the window closes, so `git worktree remove`,
  `prune` and other tools' tidy-ups leave it alone. Removal is offered only for a checkout that
  nobody else has locked, not the main one, and not one a tab is working in; it says what it
  will
  delete first, and it never deletes the branch. One row down, `Remove 3 free checkouts…`
  does the same to all of them at once — it looks in each one first, names the branches that
  will actually go, and leaves anything with work in it exactly where it is. That row is there
  only when there is something for it to take. A checkout whose branch has already landed on
  the base — merged, or squashed in by the forge, which git itself cannot tell — is marked
  `merged`, worked out from the repository alone with nothing sent anywhere, and
  `Remove 2 merged checkouts…` takes just those, on the same terms. A removal also takes the
  directories of the store it leaves empty. One git can only half do — over a file it may not
  delete, such as one a container wrote as root — is said as that rather than as a refusal: git
  no longer lists the checkout, the menu names the folder the rest is left in, and
  `Show the folder` opens it. Deleting a *branch* is a gesture of its own, and
  only on that proof: `Delete 4 merged branches…` names each one with the commit on the base
  that has its work, proves it again the moment before it goes, and keeps any that moved in
  between; a branch the proof cannot reach is never offered. On a tab in a checkout Crook made,
  **Finish task** is the end of the task in one confirmation — it closes the tabs working
  there, removes the checkout, and deletes the branch if its work has landed, or keeps it and
  says `kept feat/x: not on main yet`. It will not while an agent or a command is still working
  there. **Discard task** deletes the branch whether or not it has landed, after a second
  question that names the commits and files that go with it; a checkout that changed while
  either question was up is asked about again rather than acted on. Checkouts go in a store of
  Crook's own — neither inside the repository, where git will happily let you put one and every
  build and every search then trips over it, nor beside it in a directory somebody else laid
  out.

- **What the agent changed, beside it.** `Show changes` — on a tab's menu, or
  `crook/changes/toggle` from the palette — docks a **Changes** column between the tabs and the
  work, about the focused tab's repository: what it is compared with (where the branch left the
  base, the merge-base a pull request would compare from), the commits since then, newest
  first, and every file that differs — committed, staged, only saved, or never added. Press a
  file and its hunks open under it, in the theme's own added and removed colours; press it
  again and they fold away. A shown file offers **Open** in your `$VISUAL` or `$EDITOR` — as
  Crook itself was started with them, which a launch from the desktop may not share with your
  shell's rc file — at the first line that changed for an editor Crook knows how to tell a line
  (VS Code and its forks, Sublime Text, Zed, the JetBrains IDEs, Kate, gVim, Emacs, gedit…) and
  at the top of the file for any other, **Copy path**, and **Copy diff**. It
  follows you from tab to tab, reads again whenever the tab rows' git facts do and on its
  **Refresh**, and never on a timer of its own. Every read is on the background pool under a
  deadline, with `--no-ext-diff` and `--no-textconv` so a repository's configuration cannot
  make it run a program, and a file's diff is read only when you open it and cut at half a
  megabyte or three thousand lines, saying so. The list is windowed, so a five-thousand-file
  diff costs a screenful. **It is read-only on purpose**: there is no stage, no revert and no
  edit, because the tree it shows is one an agent is writing to, and a write from here races
  the agent — asking the agent, or your editor, is how the work changes. An editor that draws
  in a terminal (`vim`, `nvim`, `nano`, `hx`, `emacs -nw`) has nothing to draw in when Crook
  starts it, so Open is offered only for one that opens a window.
  **Asking the agent is one press.** Press a changed line — or a hunk's header — and type a
  comment in the field that opens under it: Enter keeps it, Escape drops it, one line each.
  Comments belong to the tab (and the repository and base it is looking at), live in memory
  only — a restart or closing the tab loses them — and follow their line while the agent keeps
  writing: a refresh moves each one to the line that still says what it said, and drops the
  ones whose line is gone, saying which (one not found in a long diff that was cut short is
  kept, and listed where the diff stops with its ×). A comment still being typed when a
  refresh takes its line keeps its words and the keyboard, moved to the top of the column until
  a refresh finds the line again, and Enter there adds nothing — so nothing typed for the
  comment reaches the agent. **Send N comments to the agent**, at the top
  of the column, makes them one message — `Review of <branch> since <base>:`, then each comment
  as `path:line`, the line quoted, and what you said — pastes it into the tab's agent as one
  bracketed paste **with no Enter**, and puts the keyboard there: you read it, and you send it.
  The agent is the tab's pane whose agent has reported a status and has not exited back to its
  shell's prompt, else the focused one — an agent started in a line the shell says nothing
  about (bash's `( … )`, or any line in a shell without Crook's marks) counts once it has
  reported since that line. It is never pasted into a shell prompt, where its lines
  would be commands, nor into a program that has not turned bracketed paste on, where every
  line would arrive as its own Enter; the column says so and keeps the comments. **Copy
  review** puts the same text on the clipboard, for an agent in another tool. Checked by hand
  to take a multi-line bracketed paste into the prompt unsent: Claude Code 2.1.280 and OpenCode
  1.18.33. Codex 0.149.1 and Gemini CLI 0.61.0 turn bracketed paste on when they start, but a
  paste into their prompts is not verified.

- **A settings page**, which is a **section of the sidebar** rather than a window or a tab:
  `cmd/ctrl-,`, the **Settings** button at the foot of the panel or the View options menu's
  last entry puts its rail of pages where the tab list was and the page where the panes were,
  and **Sessions** beside that button puts the work back. Five pages: Appearance, Shell,
  Notifications, Keyboard Shortcuts and About — and a plugin's page arrives on the same rail
  beside them.
  Every option on it is one the application actually reads; there is nothing there that does
  not do something. Changes apply on the click and are
  written to `<config>/crook/settings.json`, which is the same eight keys that menu writes, the
  status marks, the theme, the light and dark pair it follows the desktop between, the
  terminal's type size, whether the tabs come back, whether Crook opens at login (a
  LaunchAgent on macOS, an XDG autostart entry on Linux, the `Run` key on Windows — each the
  person's own and taken out again when switched off), whether a bell plays the system's alert
  sound (off, as in Warp; a pane you are not looking at is marked either way, and a burst is
  one sound), which stops post a notification, whether a close asks before ending agents that
  are still working, the Shell page's login shell, Tab key, Option as Meta and — on Linux —
  middle-click paste, whether the tabs panel shows and how wide it is, and — set in the file
  rather than on the page — its font family and the line each agent is resumed with.
  Beside them, **Default terminal** makes Crook the terminal the system opens — Launch
  Services' shell handler on macOS, what Finder runs a script with no extension in (each in a
  tab of its own, started in the script's folder), and the first line of
  `xdg-terminals.list` on Linux — and says so once it is; it is the system's, so it is asked
  afresh each time the page is shown, and not offered on Windows, which has no such setting.
  The type size is also on `cmd/ctrl-plus`, `-minus` and `-0`, and every pane resizes with it:
  a pane's columns and rows are its box divided by a cell, so the ptys follow.
  There is no shell under it, and every control on it is reachable from the keyboard as well
  as by a click. The keyboard starts in the search box below; **Tab** walks from it to the rail
  and on through the page's controls in the order they are drawn, **Shift-Tab** walks back,
  and neither comes round from the end. **Down** from the box goes straight to the first row
  it found, the arrows move between pages on the rail and between controls on the page, and
  **Enter** or **Space** presses the control with the ring around it. **Left** and **Right**
  push a switch's knob, choose a segment and step the type size. **Escape** walks back out:
  from a control to the box, then empties the box, then puts the tabs back. A letter typed on
  a control goes into the box. Choosing a setting in the palette lands the keyboard on its row.

  At the top of its rail is a **search box**, and it narrows both halves of the page at once:
  the rail keeps only the pages that hold an answer and says how many each of them holds, and
  the page keeps only the rows that are one. A row is found by its own name, by the line under
  it, by the value on its right — so `cmd-w` finds "Close the focused pane" and a path finds
  the settings file — by the page and category it is in, and by a hand-written list of the
  words somebody would actually type: nothing on the "Tab placement" row says *sidebar*.

- **Keybindings**, in VSCode's format and with VSCode's rules, in
  `<config>/crook/keybindings.json`: a list of `{ "key", "command", "when" }` rules, the last
  matching rule wins, a `-` in front of a command takes it off a chord, and a key may be a
  sequence like `"ctrl+k ctrl+s"`. A command is a name — the window's own are
  `crook/window/*`, and a plugin's are its own — so anything reachable by name is bindable,
  including things this build has never heard of. What a *pane* does with a key is not in it:
  `ctrl-c` interrupts and `ctrl-d` ends an input, and a binding that could take one of those
  away would be one that breaks a terminal. The **Keyboard Shortcuts** page lists every
  command, the chord that reaches it and where that chord came from — none of it written down
  by hand — and it **records new ones**: click a chord, press the keys you want, Enter to keep
  them and Escape to leave it alone. Keeping one writes VSCode's own two lines into the file
  — the command taken off the chords it had, then the chord you pressed — and touches nothing
  else in it, comments included. Beside each chord is whichever of "Reset" and "Unbind" that
  row can still be asked for. The same recording is reachable by name, as
  `crook/shortcuts/rebind`, which is how a plugin's own chip can offer "change this
  keybinding" without being able to write a file itself. Editing the file by hand works too:
  while the settings are showing, Crook re-reads it and the chords in it are in force — the
  page tells you where it is.

- **Everything the window does has a name, and most of it has a key.** The window registers
  sixty commands of its own and ships chords for forty-one; the other nineteen are
  reached by name, from the palette or from a chord of your own. That split is deliberate — a
  shipped chord is a key taken away from the shell in every pane, forever, so it is spent on
  what is pressed often and not on what is done once a week.

  | | macOS | Linux and Windows |
  |---|---|---|
  | Tab by position, and the last one | `cmd-1`…`cmd-8`, `cmd-9` | `alt-1`…`alt-8`, `alt-9` |
  | Back to the tab you were on, and again to return | `shift-cmd-j` | `ctrl-shift-alt-j` |
  | The next tab whose agent is waiting for you | `cmd-j` | `ctrl-shift-j` |
  | Answer the waiting agent's request, or list them | `cmd-i` | `ctrl-shift-i` |
  | Move to the pane beside this one on that side | `ctrl-shift-←↑↓→` | `alt-←↑↓→` |
  | The next pane, the previous one | `cmd-]`, `cmd-[` | `ctrl-shift-]`, `ctrl-shift-[` |
  | Page the output | `shift-PageUp`, `shift-PageDown` | the same |
  | The ends of the scrollback | `shift-cmd-PageUp`, `shift-cmd-PageDown` | `alt-Home`, `alt-End` |
  | Clear blocks | `shift-cmd-k` | `ctrl-shift-l` |

  Paging works over both surfaces, which is the split the wheel already makes: a pane drawing
  a list of commands moves its own offset, and one a full-screen program has taken moves the
  emulator's history. The bare page keys are left to the program — Shift has always been what
  takes the scrollback back from whatever is running.

  The arrows go by where the panes are on screen, not by the order they were made in: up from
  the bottom right of a 2×2 is the top right, and of two panes beside the focused one — the
  watchers stacked beside an agent — the one you were in last.

  A chord that cannot do anything **declines**, and the keystroke goes on to the shell. There
  is nothing above a pane along the top of the tab, so `ctrl-shift-↑` over one still means
  whatever it means in vim; a digit past the end of the strip selects nothing; and the commands about a block do
  nothing at the prompt, where none is selected. A chord is never silently swallowed.

  Without a chord, by name: `split-left` and `split-up`, `grow-pane`, `shrink-pane` and
  `even-panes`, a block's menu (`open-block-menu`) and every entry of it (`copy-block`,
  `copy-block-command`, `copy-block-output`, `copy-block-directory`, `copy-block-branch`,
  `rerun-block`, `bookmark-block`, `filter-block`, `scroll-to-block-top`,
  `scroll-to-block-bottom`), the
  bookmarks between blocks (`previous-bookmark`, `next-bookmark`), `resume-agents` (the
  agents a restart ended — see Tabs), every entry of a tab's
  (`crook/tabs/pin-tab`, `close-tab`, `open-menu`, `view-options`,
  `toggle-group`, `close-group`, the seven colours), the Changes column
  (`crook/changes/toggle`), the worktree list (`crook/worktrees/menu`) and its task creator
  (`crook/worktrees/new-task`), the end of a task (`crook/worktrees/finish-task`,
  `discard-task`) and every settings page (`crook/appearance/open-page` and its four
  neighbours). The block entries act on the block the menu is up on, or — with no menu — on the
  one the keyboard has selected, so each of them is a chord as well as a row.

- **The menus can be walked.** A tab's context menu opens with `crook/tabs/open-menu`, the
  arrows move down it, Enter runs the row and Escape takes it down. The worktree list inside
  it answers the same four, plus Delete to offer to remove the checkout the keyboard is on —
  and only where the × would be drawn, so a key cannot ask about one git is certain to refuse.
  A tab's menu, a block's menu and every row of the command palette print **the chord that
  reaches** the entry, right-aligned. None of them is told one: the key a row is built with is
  the name of the action it runs, so what is printed is whatever is in force — including a
  chord you rebound this morning, and including a chord bound to something a sandboxed plugin
  put in a block's menu. The worktree list is the one that prints none, because its rows are
  numbered rather than named: there is no action for a chord to be looked up under.

  **The palette is the one box for all of it.** The chord opens the commands, the tabs that
  are open and the rows of the settings together, each block under a heading of its own — a
  heading appears when there is a second kind of answer to tell it apart from, so a query only
  the commands answer is the flat launcher it has always been. A tab's row says what its row
  in the sidebar says and where it is working, and Enter goes there; a settings row says which
  page and category it lives on, and Enter opens that page with the row's own words in the
  rail's search box, which is what puts a row somewhere in the middle of a long page on screen,
  and the keyboard on the row, so Space changes it.

  A sigil at the front narrows the box to one list, and `tab` walks between them without
  retyping the query: `>` the commands, `@` the tabs, `#` the settings, and `?` **what is
  bound to what** — every command under the plugin that registered it, the ones no chord
  reaches marked `not bound`, then `Other bindings` for the chords in your file that name
  something which is not a command, and `In a pane` for the keys the program in the pane eats
  — `ctrl-c`, `ctrl-d`, the ones a binding could only break. `crook/palette/keys`,
  `crook/palette/tabs` and `crook/palette/settings` open each straight. None spends a chord of
  its own: a key pressed once a fortnight is not worth one taken from every shell, and a sigil
  costs nothing. The rows that cannot be run — a key a pane eats, a line naming a command this
  build has never heard of, which says `nothing answers to this` — are stepped over by the
  arrows, because Enter has to mean one thing.

Everything else is out of scope on purpose. There is no telemetry, and OSC 8 hyperlinks are
not read — though a URL a program *printed* is clickable, because the scan that finds one
works the same on a finished block as on the live grid, which an OSC 8 carried on the grid
alone would not. The keyboard steps through the blocks, selects one, pages the output and
runs every entry of a block's menu by name, and the composer types on the prompt's own line,
but the blocks are still short of a few things: no pointer click-to-select a block and no
jump-to-bottom *button* — the chord for it exists; [`docs/blocks.md`](docs/blocks.md) lists
those and says what each would touch.
A selection copies, and the paste chord puts the clipboard into the program as a bracketed
paste. With no text on the clipboard — a screenshot, say — the chord sends a program holding
the keyboard `ctrl-v` instead, on which Claude Code and Codex read the image off the clipboard
themselves. A program can write the clipboard too, with OSC 52 — what `tmux`, `nvim` and a
copy over `ssh` use — and it is the same clipboard `cmd-c` writes; an empty write is dropped rather than
clearing what somebody had copied. What a program cannot do is *read* it: that direction is
refused, since answering it would hand the clipboard to anything that can print to the pane.
On Linux a selection is also **PRIMARY**, as in every Linux terminal: the end of a drag — over
the output, the composer or any text field — is what a middle click in another program pastes,
and a middle click in a pane pastes what was selected elsewhere, at the composer's caret (beside
what is selected there, not over it) or, under a program, as a bracketed paste (Shift past one
that reads the mouse). The clipboard is left alone. Settings → Shell → *Middle-click pastes the selection* (`middle_click_paste`) turns
both off. On a Wayland desktop it goes through XWayland.
**Files dropped on the window** have their paths typed, each quoted as one word and a space
between two, into the focused pane — not the one under the pointer, since a drop arrives with no
position: into its composer at the caret, or with no composer to the pty as a paste, bracketed
when the program asked for that — what Claude Code's own advice to drag a screenshot into the
terminal counts on. Quoted for whoever reads it: the pane's shell, in its composer or at its
prompt — double quotes for cmd.exe, PowerShell's own spelling for PowerShell — or a program
the shell reported starting, or one on the alternate screen, in single quotes as a shell would
read them. cmd.exe and PowerShell report nothing, so Crook cannot tell their prompt from a
program started from them: whatever reads there on the main screen — `wsl`, `ssh`, a nested
shell, Claude Code — gets the shell's spelling. That spelling leaves out a file whose name a
shell started from it could run part of: under cmd.exe one holding `$`, `` ` ``, `!`, `%` or a
double quote, straight or curly, or ending in `\`; under PowerShell one with a `\` before a `'`,
or ending in `\`. A program under PowerShell may not undo the quotes it doubles — Claude Code
reads `Bob’’s` for `Bob’s` — and so misses a file named with one. A menu, a field holding the
keyboard or a section of the sidebar shown in place of the panes keeps a drop out of the pane,
as it keeps the paste chord out. On macOS, Windows and X11 only: winit, which Crook's window is
built on, has no drag-and-drop on Wayland, so there a drop does nothing.
A program can also print a **picture**, with iTerm2's inline images — OSC 1337 `File=`, and its
`MultipartFile` pieces — which `imgcat`, `viu` and `chafa -f iterm` speak. It is drawn over the
rows it pushed the cursor past: in the block it was printed in, or over the grid while the pane
draws one — for an `ssh` session without Crook's shell integration, or a REPL or a training run
whose output has gone past a screenful — where it moves with its rows through the scrollback,
past the ten thousand lines a pane keeps as well. It goes once its first row has scrolled out of
them, and the block keeps the blank rows rather than draw it over the oldest line; and every
picture in the block goes if a program scrolls more than a whole scrollback at once, in one
synchronized update, which is more than Crook can follow. A picture printed over every cell
of an earlier one replaces it, so a plot redrawn in place is one picture rather than one a frame.
The block being printed keeps 256 MB of pictures, counting each as at least 16 KB — sixteen
thousand icons — and lets its oldest go past that. It is sized the way iTerm2 sizes it: by
`width` and `height` in cells, pixels or a share of the pane, fitted with its shape kept unless
`preserveAspectRatio=0`, and never wider or taller than the pane. It is decoded away from the
thread that draws the window, at no more pixels than it covers — so a screen of large pictures
holds nothing up, and one across a Retina pane is as sharp as its file — and for the moment that
takes, a faint frame shows where it goes. PNG only, at most 16 MB and 4096 pixels a side; and not
on the alternate screen, which a full-screen program redraws cell by cell at will — so yazi's
previewer and an `imgcat` inside tmux, both of which draw from there, show none. A `clear` or a
`reset`, a resize that reflows the rows under it, or a shorter pane under a full-screen program
takes a picture away. Kitty's graphics protocol is not read yet. The list of what is absent — and
what adding each item would touch — is the last section of the architecture doc.

## Which files your shell reads

If your `PATH` inside Crook is not the `PATH` you get in your other terminal, or the banner
your `~/.zprofile` prints is missing, this is the section.

**Crook starts a login shell**, the same as Terminal.app, iTerm2 and WezTerm, and the same as
`login(1)` itself. A shell reads a different set of your files depending on how it was
started, and the login set is the one that holds the facts about your whole session:

| shell | a login shell reads | a non-login interactive shell reads |
|---|---|---|
| zsh | `/etc/zshenv`, `~/.zshenv`, `/etc/zprofile`, `~/.zprofile`, `/etc/zshrc`, `~/.zshrc`, `/etc/zlogin`, `~/.zlogin` | `/etc/zshenv`, `~/.zshenv`, `/etc/zshrc`, `~/.zshrc` |
| bash | `/etc/profile`, then the **first** of `~/.bash_profile`, `~/.bash_login`, `~/.profile` — and not `~/.bashrc`, which the profile usually sources itself | `~/.bashrc` |
| fish | `config.fish`, with `status is-login` true, which is what makes fish run `path_helper` | `config.fish` |

On macOS the missing half is the expensive one: `/etc/zprofile` is where `path_helper` builds
`PATH` out of `/etc/paths` and `/etc/paths.d`, and **nothing else runs it**. A non-login shell
on a Mac therefore has a `PATH` nobody assembled — different tools, different versions, in a
different order than every other terminal on the same desktop.

**Two things follow for your dotfiles.** Your `~/.zprofile` and `~/.bash_profile` now run
*once per pane* rather than once per login, so anything slow or noisy in them is slow and
noisy in every pane and every split — that is what the switch below is for. And a shell Crook
starts is otherwise exactly your shell: it reads your own files, in your shell's own order,
each exactly once. Crook never writes to a file you own; it points the shell at a scratch
directory of stubs that source yours, and deletes it when the pane closes.

**The default is per platform**, because the question is "what does the terminal beside it
do":

- **macOS and Windows: on.** Every macOS terminal starts a login shell, and `path_helper`
  lives where only a login shell will find it.
- **Linux: off.** GNOME Terminal, Konsole and xfce4-terminal all start a non-login shell, and
  Linux configurations are written to match — `PATH` and the prompt go in `~/.bashrc` or
  `~/.zshrc`, and a `~/.bash_profile` that does not source `~/.bashrc` is an ordinary thing to
  have. Turning it on there would read the profile, skip `~/.bashrc` — that is bash's rule,
  not Crook's — and leave you with no aliases and no prompt.

**The switch** is Settings → Shell → *Start a login shell* (`cmd/ctrl-,`, or
`crook --settings shell`), stored as `login_shell` in the settings file. It applies to the
next shell opened, not to the ones already running, because a shell reads its startup files
once and nothing can make it read them again.

**Two shells are special.**

- **bash** cannot be given `--rcfile` and `-l` at the same time: a login bash reads no rc
  file at all, so `bash --rcfile <file> -l` silently never opens the file, and the command
  marks are in that file. Crook's rc file therefore runs bash's own login sequence itself, in
  bash's own order, and its logout sequence — `~/.bash_logout` and the `logout` builtin — as
  well. What it cannot reproduce is `shopt -q login_shell`, which stays off, and `$0`, which
  is the shell's path rather than `-bash`.
- **A shell Crook has no `-l` for** — tcsh, ksh, dash, nushell, a wrapper script — is started
  the way `login(1)` starts one instead, with `argv[0]` set to the shell's name with a leading
  hyphen. That needs no option parsing, so it works on a shell nobody anticipated; `-l` would
  not, since tcsh answers ``Unknown option: `-l'``.

**Windows** has no login shell to start. `PATH` is in the registry and every process already
has all of it, and `$PROFILE` is read by every interactive PowerShell. There is no convention
to imitate, so nothing is imitated.

## Prerequisites

Crook has **zero build scripts and links no system library**. Every dependency is a crates.io
crate that compiles with nothing but a Rust toolchain and a linker. The per-platform
prerequisites are therefore small and, on two of the three platforms, already present.

Everywhere: a Rust toolchain via [rustup](https://rustup.rs). The exact channel is pinned in
`rust-toolchain.toml`; the rustup proxies download it on first use, so you do not select it
yourself.

### macOS

**Xcode Command Line Tools.** That is the whole list.

```sh
xcode-select --install
```

Crook does not need full Xcode, does not need `xcodebuild -runFirstLaunch`, and does not need
the separately-downloaded Metal Toolchain. It reaches Metal through `wgpu`, which ships
precompiled shader translation, so nothing in the build ever invokes `xcrun metal`.

### Linux

Build time: a C toolchain (rustc shells out to `cc` to link) and `pkg-config`.

```sh
sudo apt-get install -y build-essential pkg-config
```

A binary needs the glibc it was linked against, or a newer one, so what you build starts on your
distribution and on the ones after it, not on older ones. That is why a release is built in a
Debian 11 container (`.github/workflows/linux-binary.yml`) rather than on a current system.
`./script/glibc-floor 2.31 dist/linux/crook` says whether a build of yours would start everywhere a
release does.

Run time: the X11, Wayland, EGL and mesa shared objects that `winit` and `wgpu` `dlopen` when
the window is created. These are *not* build dependencies — the workspace compiles without
them and then fails to open a window — which is why they are listed separately and why none
of them is a `-dev` package.

```sh
sudo apt-get install -y \
  libx11-6 libxcb1 libxi6 libxcursor1 libxkbcommon-x11-0 \
  libwayland-client0 libwayland-egl1 \
  libegl1 libgl1 libgl1-mesa-dri mesa-vulkan-drivers \
  fontconfig
```

`fontconfig` is here for its *configuration files*, not its library: Crook's font stack parses
them in pure Rust to learn which directories hold fonts. Nothing links `libfontconfig`. You do
need at least one installed font family — on a minimal container image, add `fonts-dejavu-core`.

On distributions that are not Debian-derived, install the equivalent packages;
`./script/bootstrap` prints the list.

### Windows

**Visual Studio Build Tools** with the MSVC toolchain and the Windows SDK. The `-msvc` target
cannot link without them.

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
```

No CMake, no protoc, no LLVM/`libclang`, no Inno Setup. Crook takes no dependency that runs
`bindgen`, so nothing hunts for `libclang.dll`.

## Build and run

```sh
./script/bootstrap    # install or explain this platform's prerequisites
./script/run          # cargo run -p crook --bin crook-dev
```

Both scripts are POSIX `sh` and work identically on macOS, Linux, and Windows under Git Bash.

If you would rather drive Cargo directly — note that rustup was installed here with
`--no-modify-path`, so the scripts export it and a bare shell may not have it:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p crook --bin crook-dev
```

To produce a release binary:

```sh
./script/bundle                # dist/<platform>/crook  (crook.exe on Windows)
./script/bundle --check-only   # type-check with release flags, build nothing
```

`--check-only` is the fifth check, and what the workflow runs on all three platforms on every
pull request to `main` and every push to it. It compiles the workspace with the exact profile
and feature set a shipped build uses, which catches the class of bug where a release-only
feature combination does not compile — cheaply, and without producing artifacts.

## Repository layout

```
app/                     the `crook` library, plus two ~20-line channel binaries
crates/crookui_core/     entities, handles, contexts, elements, layout, Scene   (MIT)
crates/crookui/          winit windowing, wgpu renderer, cosmic-text font stack (MIT)
crates/crook_plugin/     identities, manifests, slots and registration guards   (MIT)
crates/crook_plugin_api/ the wire a sandboxed plugin and its host share (MIT, on crates.io)
crates/crook_wasm/       the wasmi sandbox: fuel, memory, and checked bytes (MIT, on crates.io)
crates/crook_terminal/   pty, emulator, and the snapshot the renderer draws     (MIT)
docs/architecture.md     the design, and the reasoning behind each divergence
docs/blocks.md           the block surface: what draws it, and what it does not do yet
docs/plugins.md          the two plugin tiers, and what each of them may do
docs/images/             the README's screenshots, rendered by `--snapshot`
assets/                  the app icon: pirate.svg, composed for Apple's grid
                         in icon-macos.svg, rendered into Crook.iconset
packaging/               the plugin for Claude Code, Codex and Copilot CLI, the Linux
                         desktop entry and AppStream record, and the AUR package
script/                  the scripts below, each explained in its own header comment
```

Two crates are meant to leave this repository. A plugin is written outside it, by somebody with
no checkout, and `crook_plugin_api` is the one thing they cannot do without — so it is packaged
for crates.io and versioned `0.<abi>.<patch>`, which makes `crook_plugin_api = "0.8"` Cargo's
way of writing "built against ABI 8". `crook_wasm` goes with it for one job: `cargo install
crook_wasm` puts `crook-plugin-info` on a machine, which is how a registry says what an
artifact it just built actually is, using the host's own reader rather than a second one.
`./script/publish` is what uploads them. Both are there, and every plugin, the template
included, depends on the published `crook_plugin_api` rather than carrying a copy of it.

| script | what it does |
|---|---|
| `bootstrap` | prepares this machine to build Crook, or prints the command where installing is not its to do |
| `run` | builds and runs the dev-channel binary, `crook-dev`, passing its arguments on |
| `bundle` | a release build for this platform or one named target; `--archive` packs it with its checksum, `--check-only` builds nothing |
| `install` | installs a released binary, checked against the release's `SHA256SUMS`; `--desktop` adds the Linux launcher entry |
| `glibc-floor` | fails when a Linux binary needs a newer glibc than the floor it promises |
| `release` | bumps the version, writes the changelog section, commits and tags — and refuses a commit Crook CI has not passed |
| `release-notes` | prints what a tag's release page says, out of `CHANGELOG.md` |
| `commit-lint` | checks that commit titles are the Conventional Commits the changelog is written from |
| `hooks/` | `commit-msg`, which runs `commit-lint` on every commit once `git config core.hooksPath script/hooks` points git at it |
| `publish` | publishes `crook_plugin_api` and `crook_wasm` to crates.io; a person runs it, never CI |
| `aur` | brings the `crook-bin` AUR package up to a release that is already out, and with `--push` pushes it |
| `probe-permission-hooks` | records what an agent hands its permission hooks and does to them while they wait, in a throwaway project, for `docs/answering.md`; a person runs it, since it spends the agent's usage |
| `icons` | regenerates the vendored Lucide icons from a pinned `lucide-static` |
| `app-icon` | renders the macOS app icon into `assets/Crook.iconset` |
| `macos-keychain` | puts a Developer ID certificate into a keychain `codesign` will find, on a CI runner |
| `macos-app` | wraps a macOS binary in a signed `Crook.app` |
| `notarize` | sends a signed app or disk image to Apple, waits for the answer and staples the ticket |
| `macos-dmg` | puts a stapled `Crook.app` into a signed disk image |
| `fixtures/` | what tests read: a plugin's tree for `--plugin-fixture`, and the `readelf` listing of the v0.1.13 binary `glibc-floor` is tested against |


## Checks

Whatever you touch, the gate is the four commands at the top of
[`CONTRIBUTING.md`](CONTRIBUTING.md#before-you-push), run in that order before you push, and
two more beside them for a change to a manifest, a feature gate or the plugin API. `Crook CI`
runs all six again on macOS, Linux and Windows, by itself on every pull request to `main`
and every push to `main`. A branch with no pull request to `main`, which includes one whose
pull request is stacked on another branch, starts nothing;
`gh workflow run "Crook CI" --ref <branch>` asks for it.

The rest of [`CONTRIBUTING.md`](CONTRIBUTING.md) is the conventions no linter enforces — what a
comment is for, where `ctx` goes, why nothing ticks while the window is idle — and the commit
titles the release notes are written from.

## Licensing

Crook is MIT throughout. The full text is in [`LICENSE-MIT`](LICENSE-MIT).

Warp is dual-licensed: its `warpui` and `warpui_core` crates are MIT, and everything else in
that repository is AGPL-3.0. Crook stays clear of the AGPL half:

- `crookui` and `crookui_core` port real code and shaders from Warp's two MIT crates. MIT
  permits that and asks one thing in return — that the copyright notice travel with the code.
  `LICENSE-MIT` therefore carries Denver Technologies' notice alongside this project's.
- The **pirate** — the artwork in `crates/crookui_core/src/icons/art.rs`, and the four
  `usage_*` theme roles beside it — descends from a Claude Code usage indicator written for a
  personal fork of Warp and never contributed upstream. It is its author's own work, licensed
  here by that author, and it borrows nothing from Warp beyond the shape of the surrounding
  app. The chip that used to draw it is a plugin in a repository of its own now; what stayed
  behind is the picture, which the host draws on any plugin's behalf when one asks for it by
  name.
- `app/` was written against a description of how Warp's tab strip and header behave, not by
  copying either. Where its comments mention Warp they are recording a divergence — an
  index-versus-identity bug not inherited, a public field not repeated.
- `crook_terminal` is Crook's own code over two crates.io dependencies: `portable-pty`
  (MIT) for the process side and **`alacritty_terminal` (Apache-2.0)** for the grid, the
  scrollback and the escape-sequence parser. Nothing in it comes from Warp, whose terminal
  lives in the AGPL half of that repository.

- The icons are **[Lucide](https://lucide.dev)**, whose licence is ISC — permissive, and
  compatible with MIT redistribution. Their geometry is vendored into
  `crates/crookui_core/src/icons/data.rs` by `script/icons`, which pins a version of the
  `lucide-static` package and rewrites its SVGs as path commands; the licence that came with
  it is [`LICENSE-ISC-lucide`](LICENSE-ISC-lucide), and it names Feather (MIT) for the icons
  Lucide inherited from it. Nothing else about the icons is anyone else's: the rasterizer
  that turns those commands into pixels is Crook's own.

[Alacritty](https://github.com/alacritty/alacritty) is a fast, cross-platform terminal
emulator by Joe Wilm and the Alacritty contributors, released under the Apache License 2.0.
Crook uses its `alacritty_terminal` crate — the emulator without the window — and would be a
great deal poorer without it. Apache-2.0 is permissive and compatible with MIT
redistribution; it asks that the licence and any `NOTICE` travel with the code, which
`Cargo.lock` and the dependency's own vendored licence do, and it grants a patent licence
MIT does not. A binary built from this repository therefore contains Apache-2.0 code, and
that fact belongs in whatever notice a distribution ships.

None of this is legal advice; it is a record of where each file came from, so that someone
who needs to answer the question properly has the facts to work from.
