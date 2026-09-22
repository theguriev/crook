---
name: crook
description: Work inside Crook, a terminal whose unit of work is an agent. Use only when the user mentions Crook or the task is about Crook — reporting an agent's status to its tab, worktrees, plugins, the palette or the find bar. Requires TERM_PROGRAM=Crook.
---

# Crook

Crook is a terminal whose unit of work is an agent: one tab per agent, a dot on the tab's row
saying what the agent is doing, and a panel down the left edge that lists them all. The
`crook` binary has flags for what an agent can do from inside a pane, and this file is the
whole of them.

## Tell whether you are inside Crook

Every shell Crook starts has `TERM_PROGRAM=Crook` in its environment, with Crook's version in
`TERM_PROGRAM_VERSION`. Check before doing anything below:

```sh
[ "$TERM_PROGRAM" = "Crook" ]
```

Use this skill only when the user mentions Crook or the task is about Crook. In any other
terminal, or for any other task, do nothing Crook-specific: the flags below write an escape
sequence only Crook reads, and a status reported anywhere else is noise.

## Say what you are doing

The dot on a tab's row is written by the program in the pane:

```sh
crook --agent running --title "port the tab bar"
crook --agent needs-input
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

The report is one escape sequence (`OSC 6340`) written to the terminal the command runs in,
not to standard output, so it works from a hook whose output belongs to someone else. There
is no socket and no pane id: the terminal you have is the pane. The same sequence reaches the
pane over `ssh` and from inside a container, and every other terminal drops it unread. A
status never taken back goes when the shell's own marks say the command ended.

`crook --agent-hooks claude` prints the fragment of Claude Code's settings that reports all of
this by itself: running when a prompt is sent and around every tool, needing input on every
notification, idle on stop. Print it and let the person merge it into `~/.claude/settings.json`
or a project's `.claude/settings.json`. Do not write either file yourself.

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
crook --install-plugin <path>          # copy a plugin's .wasm into the plugins directory
crook --uninstall-plugin <owner/name>  # remove one, with whatever it was allowed to do
crook --dev-plugin <path>              # run the plugin being written, from where it is built
```

Installing is not allowing: a plugin that has just arrived may do nothing until it is allowed
on the Plugins page in the settings. `--dev-plugin` takes a `.wasm` or the directory it is
built in and runs it again on every build; nothing is installed and nothing is left behind.

## Rules

- Never guess a pane id, a tab name or a worktree path. Crook hands an agent none of them and
  needs none: the terminal it runs in is the pane.
- Never write `~/.claude/settings.json` or a project's `.claude/settings.json` yourself.
  Print the hooks with `crook --agent-hooks claude` and let the person merge them.
- Do not claim a feature this file does not list. `crook --help` is the whole command line.
- Report only what is true: `running` when the work starts, `needs-input` only when you have
  actually stopped for a person, `failed` when the work failed, `idle` when it is done.
