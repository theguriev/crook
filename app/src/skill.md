---
name: crook
description: Work inside Crook, a terminal whose unit of work is an agent. Use only when the user mentions Crook or the task is about Crook — reporting an agent's status to its tab, worktrees, plugins, the palette or the find bar. Requires TERM_PROGRAM=Crook.
---

# Crook

Crook is a terminal whose unit of work is an agent: one tab per agent, a dot on the tab's row
saying what the agent is doing, and a panel down the left edge that lists them all. The
`crook` binary has flags, and one command, for what an agent can do from inside a pane, and
this file is the whole of them.

## Tell whether you are inside Crook

Every shell Crook starts has `TERM_PROGRAM=Crook` in its environment, with Crook's version in
`TERM_PROGRAM_VERSION`. Check before doing anything below:

```sh
[ "$TERM_PROGRAM" = "Crook" ]
```

`CROOK_PANE_ID` is there too: the number of the pane the shell runs in, the same for the life
of the pane and different from every other pane in the window. It is for telling panes apart
— a log file named after it, a lock keyed on it, your own row in `crook pane list` — and for
nothing else: no command takes it, and reporting status needs no id at all, because the
terminal you are in is the pane. `CROOK_SOCKET` is where the window answers `crook pane list`
(below), and it is empty when the window has none.

Use this skill only when the user mentions Crook or the task is about Crook. In any other
terminal, or for any other task, do nothing Crook-specific: the flags below write an escape
sequence only Crook reads, and a status reported anywhere else is noise.

## Say what you are doing

The dot on a tab's row is written by the program in the pane:

```sh
crook --agent running --title "port the tab bar"
crook --agent needs-input --message "wants to run rm -rf build"
crook --agent failed
crook --agent idle
```

- `running` — the work is under way. The row shows it, and any attention the row was asking
  for is cleared, since the stop it announced is over.
- `needs-input` — you have stopped for a person: a permission, a question, a prompt nobody
  has answered. The row is washed amber, the header counts it among the tabs waiting, and
  cmd-j (ctrl-shift-j off macOS) takes the person to it.
- `failed` — the work failed. The row keeps that until the next command starts.
- `idle` — the work is done.

`--title` names the work and becomes the tab's title. Keep it short: a row is one line and
cuts a title at about sixty characters.

`--message` says what you are waiting for, and only means something beside `needs-input`:
while you wait, the row's second line is that sentence rather than the directory or the
branch, since what you are asking is what decides whether the person comes now. It goes the
moment you report anything else. `--message -` reads it from standard input, for a hook
that is handed a notification's text there.

The report is one escape sequence (`OSC 6340`) written to the terminal the command runs in,
not to standard output, so it works from a hook whose output belongs to someone else. The
report needs no socket and no pane id: the terminal you have is the pane. The same sequence reaches the
pane over `ssh` and from inside a container, and every other terminal drops it unread. A
status never taken back goes when the shell's own marks say the command ended.

`crook --agent-hooks claude` prints the fragment of Claude Code's settings that reports all of
this by itself: running when a prompt is sent and around every tool, needing input on every
notification, idle on stop. Print it and let the person merge it into `~/.claude/settings.json`
or a project's `.claude/settings.json`. Do not write either file yourself. The same flag takes
`codex`, `gemini`, `copilot` and `opencode`, and prints each one's own hooks file or plugin
with a note on stderr saying where it goes; `aider` has no hooks, and it says what to do
instead.

## Tabs, groups and worktrees

The panel lists one row per tab, with its status dot and, under the title, the branch its
shell is on. Tabs that belong together fold into a group with a heading and a count. A git
worktree opened from a tab makes a group out of the two of them — the tab and the worktree
opened from it — which is what says two checkouts are one piece of work.

Worktrees are reached from a tab's own menu: right-click its row and, inside a repository,
`Worktrees` lists that repository's checkouts. Choosing one opens a tab there, or brings
forward the tab already in it; `New worktree…` asks for a branch name and opens a tab in a
checkout under Crook's own store, folded into a group with the tab that asked. There is no
command-line flag for any of this; tell the person where the menu is.

## See what is open

```sh
crook pane list          # every pane of this window: number, status, title, group, branch, directory
crook pane list --json   # the same as a JSON array, for a script
```

It asks the window your shell runs in, over the local socket `CROOK_SOCKET` names, and only
reads. Each entry has `pane_id` — the one equal to `$CROOK_PANE_ID` is your own — `tab_id`,
`title`, `tab_title`, `group`, `focused`, `status` (one of the four words above), `message`,
`cwd` and `branch`. Use it to see what the other agents in the window are doing — which one is
waiting and on what, which branch each is on — rather than guessing. It works on this machine
only: not from the far side of `ssh`, and not on Windows yet. When it says the window has no
socket, or that it is older than the command, there is nothing to ask; say so and go on.

## After Crook restarts

Every Crook update is a restart, and a restart ends every process in the window, an agent's
included. The window comes back with each pane's shell in the directory it was in. A pane that
was running Claude Code, Codex, Gemini CLI or Copilot comes back with that agent's resume line
typed into its command line and not sent: `claude --continue`, `codex resume --last`, `gemini
--resume latest` or Copilot's picker `copilot --resume` — and the agent's picker (`claude
--resume`, `codex resume`) where two panes of one agent shared a directory, or nothing for
Gemini CLI, which has no picker. OpenCode and aider come back with an empty command line
unless the person has given them a line under `resume_lines` in `settings.json`. Only the
agent's program name is remembered, never the prompt typed after it, and no output comes back.
Nothing runs until the person presses Enter in the pane, or chooses "Resume every agent" in
the palette, which sends every line Crook typed and nobody has edited. A resumed conversation
is in the same directory as before; the shell, its environment and anything that was running
in the background are new.

## Where things are

- The command palette (shift-cmd-p, or ctrl-shift-p off macOS) is one box for every command,
  every open tab and every settings row; `>` narrows it to commands, `@` to tabs, `#` to
  settings and `?` to what is bound to what.
- The find bar (cmd-f, or ctrl-shift-f off macOS) searches every block of a pane's output at
  once rather than the screenful; Enter and Shift-Enter step between matches, and Escape
  hands the keyboard back to the shell.
- The settings open with cmd-, (ctrl-, off macOS), and the Keyboard Shortcuts page there
  lists every command with the chord that reaches it.

## Plugins

```sh
crook --plugins                        # list the plugins installed as files
crook --plugins --json                 # the same as a JSON array: id, version, path, enabled, allowed
crook --install-plugin <path>          # copy a plugin's .wasm into the plugins directory
crook --uninstall-plugin <owner/name>  # remove one, with whatever it was allowed to do
crook --dev-plugin <path>              # run the plugin being written, from where it is built
```

Installing is not allowing: a plugin that has just arrived may do nothing until it is allowed
on the Plugins page in the settings. `--dev-plugin` takes a `.wasm` or the directory it is
built in and runs it again on every build; nothing is installed and nothing is left behind.

## Rules

- Never guess a pane id, a tab name or a worktree path. Reporting status needs none of them —
  the terminal it runs in is the pane — and `crook pane list` is where they are read when
  something else does.
- Never write `~/.claude/settings.json` or a project's `.claude/settings.json` yourself, nor
  another agent's hooks file. Print the hooks with `crook --agent-hooks <agent>` and let the
  person merge them.
- Do not claim a feature this file does not list. `crook --help` is the whole command line.
- Report only what is true: `running` when the work starts, `needs-input` only when you have
  actually stopped for a person, `failed` when the work failed, `idle` when it is done.
