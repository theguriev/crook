# crook, the plugin for Claude Code, Codex and Copilot CLI

The plugin that connects Claude Code, Codex CLI and GitHub Copilot CLI to Crook, from the
marketplace this repository is (`.claude-plugin/marketplace.json` at its root, which all three
read):

```sh
claude plugin marketplace add theguriev/crook
claude plugin install crook@crook

codex plugin marketplace add theguriev/crook
codex plugin add crook@crook

copilot plugin marketplace add theguriev/crook
copilot plugin install crook@crook
```

Installing it is the agent writing its own settings, so nobody merges JSON and Crook opens no
file it does not own. Codex holds the plugin's hooks back until they are trusted, as it does any
hooks: it asks on its next start, or in its `/hooks`, and asks again whenever a hook's command
changes.

Each agent reads a manifest of its own here, and the manifest says which hooks are its:

- `.claude-plugin/plugin.json` — Claude Code's. It names no hooks file, so Claude Code reads
  `hooks/hooks.json`: the hooks `crook --agent-hooks claude` prints.
- `.codex-plugin/plugin.json` — Codex's, read before Claude Code's, naming `hooks/codex.json`:
  the hooks `crook --agent-hooks codex` prints, with `PermissionRequest` for the approval Codex
  stops for, since Codex has no `Notification`.
- `.github/plugin/plugin.json` — Copilot CLI's, read before Claude Code's, naming
  `hooks/copilot.json`: the file `crook --agent-hooks copilot` prints, whose commands pass
  `--session -` so that a subagent's prompt names no tab and its stop ends no turn. A Crook older
  than 0.1.27 does not read `--session`, and reports nothing for them. Each is a `bash` command,
  for macOS and Linux.

Without a manifest of its own, Codex or Copilot reads Claude Code's and runs `hooks/hooks.json`
under Claude Code's event names: running and idle, but no approval from Codex and no session from
Copilot. Claude Code reads only its own file, so Codex's `PermissionRequest`, which says
needs-input, stays out of it.

Every command calls `"$CROOK_BIN"` (the binary every pane is told it belongs to, or `crook` on
`PATH` when a pane has none) and exits 0 without a word unless `TERM_PROGRAM` is `Crook` or
`CROOK_PANE_ID` is set, or it runs inside tmux; anywhere else it does nothing. Inside tmux
`crook --agent` asks tmux for the terminals of its clients and writes the report to each, and a
terminal that is not Crook drops it unread. In a pane it exits 0 after the report too, whatever
the report did, so a Crook that cannot be reached is a line of stderr in the agent's log rather
than a notice on every prompt and tool call, and never the reason Copilot refuses a tool call,
as it does when a `preToolUse` hook fails. The agents run each hook with no terminal of its own,
and `crook --agent` reaches the pane through the terminal of the program that ran it, which
needs a Crook newer than 0.1.13.

One of Claude Code's hooks waits rather than reports: `PermissionRequest`, which Claude Code runs
as it opens a permission dialog, runs `crook --agent ask claude` with a `timeout` of 330 seconds.
For a Bash command it hands the Crook window the pane is in the whole command, and for an Edit,
MultiEdit or Write the file's path and the whole change, drawn as a diff; a change longer than the
3000 lines a diff is drawn to, or over the socket's 64 KiB line, is not asked about. It holds for up
to five minutes while a person may open a card for it there — from the palette's "Answer waiting
agent" or the tab menu's "Answer…" — and press Allow once or Deny. Claude Code's own dialog stays
up the whole time, and whichever is answered first wins: Claude Code drops a later answer. Deny
tells Claude "Denied in Crook by the person at the keyboard."; there is no "always allow", and
the hook never changes the command or writes a permission rule. It reports no status, and passes
at once, printing nothing, wherever nobody could be asked in Crook: under `claude -p` (unless
`CLAUDE_CODE_ENTRYPOINT` is exactly `cli`), for a subagent, inside screen or zellij, outside a
Crook pane, with "Let agents ask in Crook" turned off in Crook's settings, and on
Windows. Inside tmux it asks the window holding tmux's client, whichever window started the
tmux server, and the card says the agent is inside tmux. It needs Claude Code 2.1.280 or newer, and a Crook that knows `--agent ask`; an older
one refuses it before reading anything, and the hook passes.

`.mcp.json` declares one MCP server, `crook`, which Claude Code starts with the plugin and Copilot
CLI through `mcpServers` in its manifest: `sh -c` running `"$CROOK_BIN" mcp`, or `crook mcp` from
`PATH`, and exiting 1 with a line on stderr when there is neither, which the agent shows as a
server that failed to start. `crook mcp` serves the control socket's verbs as tools —
`crook_tab_new`, `crook_pane_split`, `crook_pane_list`, `crook_pane_wait`, `crook_pane_output`,
`crook_pane_close` and `crook_events_wait` — asked as the pane the agent runs in, by the
socket's own rules; none types into a pane and none answers a permission request. Outside a
Crook pane it starts all the same, and every call says it is not in one. Do not give `crook_tab_new` or `crook_pane_split` "always allow": each runs any command it is given, so allowing either for good is allowing every command. The MCP server runs outside Claude Code's Bash sandbox, so a command opened through it is not sandboxed even when Claude's own Bash commands are. The variables are read
by the shell rather than by the agent, so nothing in it is `${…}` for Claude Code to expand.
Codex reads no MCP server from a plugin here: the README at the repository's root gives the
lines for `~/.codex/config.toml`. `app/tests/command_line.rs` starts the server the way the file
says, with the binary and without it.

`skills/crook/SKILL.md` is the skill `crook --skill` prints, and all three load it.

These are copies of what lives in `app/src`, and the tests in `app/src/agent.rs` fail when they
drift: each hooks file has to be what `crook --agent-hooks` prints for its agent, each manifest
has to name the file it does, and the commands of `hooks/hooks.json` are run under `sh` inside
and outside a pane; `SKILL.md` has to be `app/src/skill.md` byte for byte. Edit those, then copy
the result here — the failing test prints what it expects. `app/tests/claude_code_hooks.rs`
runs each command the way Claude Code does, with the real binary in a session of its own below a
process on a pty, and fails unless the report arrives on that pty — and once more below a program
with no terminal, the way `claude -p` is when another agent runs it, and fails if anything
arrives. Where tmux is installed it runs them again with the agent inside tmux on that pty, which
keeps the sequence to itself unless `crook --agent` goes round it, and once with a second tmux
client whose terminal nobody reads, which must not hold the hook. Claude Code's
`PermissionRequest` hook is held to the opposite there: it writes nothing to the pty and prints
nothing in every one of those places, with the binary gone, and with an older binary that
refuses `ask`. The commands
`--agent-hooks codex` and `copilot` print are run the same way, and once more with the binary
gone.

There is no `version` in the manifests on purpose: without one, Claude Code takes the commit the
plugin was installed from as its version, so `claude plugin update crook@crook` picks up
whatever `main` has rather than waiting for somebody to bump a number here.
