# Answering agents: what an agent does while its permission hook waits

Crook means to let a person answer an agent's permission request from a card of its own, while
the agent's own dialog stays the fallback. Every part of that plan rests on how the agents treat
a permission hook that has not answered yet: whether the dialog is on screen meanwhile, what a
late answer does, what is killed and when. This page is what was measured, not what was read
from binaries or documentation. Each finding names the milestone of the plan it gates, and a
milestone whose finding is missing here does not merge.

`script/probe-permission-hooks` is how it was measured, and how it is measured again for a new
version. The payloads the agents handed the hooks are checked in, scrubbed, as
`app/tests/fixtures/asks/<agent>-<version>-<case>.json`, and `app/tests/ask_fixtures.rs` holds
each one to the fields the plan reads.

---

## Contradicts the plan

**A background subagent's request is not refused when its hook says nothing** (C14, Claude Code
2.1.280; gates M9, which must not merge as planned). The owner decision on "requests from
`claude -p` and from background or non-interactive subagents" is never to put them on a card,
because "there an empty answer is Claude's own immediate deny, and holding it would turn a
refusal into a wait", and it leaves M9 the foreground subagents. That holds for `claude -p` (C1)
and not for a background subagent of an interactive session. There the hook ran first, the
subagent's own dialog ("Bash command · from the general-purpose agent") came up in the TUI only
once the hook had exited with nothing, ten seconds later, and a Yes in it ran the command. And
every subagent came up in the background, two asked for in the foreground included, so the
foreground case M9 was scoped to never occurred. A background subagent in an interactive
session is the sequential agent M9 means to hold, and holding it hides its dialog the way
holding Codex does. Which subagents M9 covers is the owner's to decide again.

**An `ask` rule does not override a hook's allow** (C7, Claude Code 2.1.280). The plan says
twice that Claude Code ignores a hook allow "under ask rules and in confined sessions" (a risk,
and the owner decision on the row's status after an answer). Only the second half holds. With
`"permissions": {"ask": ["Bash(touch probe-ask*)"]}` in the project's settings and the session in
`acceptEdits`, where the same `touch` without the rule runs unasked, the rule did open a dialog
and run the PermissionRequest hook, and the hook's allow three seconds later closed the dialog,
ran the command and printed `Allowed by PermissionRequest hook`. A confined session is the one
that ignores it, and in 2.1.280 the only switch for that found in the binary is the evaluation
variable `CLAUDE_CODE_EVAL_CONFINED`; with it set the allow was dropped and the dialog stayed.

This gates no milestone: C7 is M3's copy. The decision it supports stands on the confined case
alone, since the card still cannot know whether the agent took its answer, so leaving the row's
status to the next report is still right. Only the reason given for it needs the correction.

Nothing measured contradicts the gates of M2 to M5 and M7. Headless runs are told apart (C1),
allow and deny work and a late answer is dropped (C3, C5), Claude Code waits for no lingering
hook (C9), the request is byte-identical after the tool (C11), and the title says when a dialog
opens and when it was answered (C12).

---

## What the plan does not say yet

Measured on the way, and needed by the milestones named:

- **"No, and tell Claude what to do differently" leaves the hook alone** (C4; M3, M4). Like Yes,
  it sends the hook no signal, and the turn goes on with the person's words. Unlike Yes, no
  `PostToolUse` or `PostToolUseFailure` follows, so M4's done fingerprint never withdraws that
  card. It stays until the turn ends or a newer ask supersedes it. An Allow pressed on it then
  is dropped (C5), so it is safe, but it is the one stale card M4 does not clear.
- **The kill reaches the hook's whole tree, not only its process group** (C4, C9, C10; M2). A
  descendant the recorder had moved into a session of its own got the same SIGTERM as the
  group. A hook that ignores the TERM is SIGKILLed about 1.5 s later, and one that ignores it at
  `/exit` holds Claude Code's exit up by that much. Crook's hook should leave SIGTERM's default
  alone.
- **A SIGKILLed Claude Code orphans its hooks** (C9; M2). No signal reaches them; they are
  reparented to the user's init and run out their wait. The plan's `getppid` watch is what ends
  them.
- **While a subagent's hook holds, the title is the spinner** (C12, C14; M9). The subagent's
  dialog is not up yet, so neither the title nor a notification says anything waits; only the
  hook's own registration can.
- **`permission_suggestions` is optional** (C13; M2). A request that an ask rule raised carried
  none. PermissionRequest carries no `tool_use_id`, which `PreToolUse` and `PostToolUse` both
  do, and an MCP tool's request adds a top-level `mcp_server`.
- **The notification no longer names the tool** (C6). Its `message` is "Claude needs your
  permission", with `notification_type` `permission_prompt`.
- **The headless signal is double** (C1; M2). Beside `CLAUDE_CODE_ENTRYPOINT`, which is `cli`
  interactive and `sdk-cli` under `claude -p`, `CLAUDE_CODE_SESSION_ATTENDED` is `1` and `0`.

---

## How it was measured

Claude Code 2.1.280 (`claude --version`; the `claude` on `PATH`, mise's `installs/claude/latest`,
which was 2.1.280 that day), on 2026-10-16, with `--model haiku` (Haiku 4.5) and one-line
prompts that each asked for one or two tool calls. The owner approved live probes of Claude
Code only; Codex, OpenCode and Copilot are deferred below.

The TUI ran in a pane of a private tmux server (`tmux -L`, no configuration), so that its
dialogs could be answered with keys and its screen read back, started from a Crook pane with a
clean environment: `env -i` with `HOME`, `PATH`, `LANG`, `TERM=xterm-256color`,
`TERM_PROGRAM=Crook` and `CROOK_PANE_ID=probe`, and no `TMUX`. It is a pane like Crook's, a pty
whose slave is the agent's terminal, but not Crook's own. Claude Code was started with
`--setting-sources project,local`, so only the probe's hooks ran and none of the user's. Three
things watched it:

- the recorder of `script/probe-permission-hooks`, on every hook event around a session, a
  prompt, a turn, a subagent and a tool;
- a process watcher outside every agent's tree, reading `/proc` every 50 ms, for when each
  recorder, its group child and its other-session child were gone;
- `tmux pipe-pane`, which hands over the bytes Claude Code wrote, for every OSC title with the
  time it arrived.

Nine runs: two interactive sessions for most cases (s1, s2), one with a five-second hook
timeout (s3), one confined (s4), `claude -p` in a pane (print-tty) and from another agent's
Bash tool with no terminal (print-notty), one closed by its pane (s5), and two SIGKILLed, of
which only the second (s6b) still had a hook holding when the kill came.
The evidence is cited as `<session>/<pid>-<event>`: the recorder's files
`<epoch>-<pid>-<event>.json/.env/.ps/.sig` in that session's evidence directory. They stay with
the run, outside the repository; the fixtures are their payloads, scrubbed.

---

## Claude Code 2.1.280

Measured 2026-10-16. Times are from the hook's start unless they say otherwise.

| | Finding | Evidence | Gates |
|---|---|---|---|
| C1 | `CLAUDE_CODE_ENTRYPOINT` is in every hook's environment: `cli` interactive, `sdk-cli` under `claude -p`, both in a pane and with no terminal. `CLAUDE_CODE_SESSION_ATTENDED` is `1` and `0` likewise. Under `-p` the PermissionRequest hook runs and is waited for: printing nothing denied at once (the model was told the touch was blocked), and an allow two seconds in ran the command. The `-p` hook from another agent's Bash tool sits below a `claude` with no terminal, in that agent's shell's session; the nearest ancestor with a terminal is across the session boundary. | print-tty/4111442-PermissionRequest, print-notty/4112463-PermissionRequest, s2/3697444-PermissionRequest.env; fixture `claude-2.1.280-bash-print.json` | M2 headless rule, M3 |
| C2 | Yes. The dialog was on screen 23 ms after the hook started and stayed through a 60 s hold. The hook runs as the dialog opens, in parallel with it. | s1/3689244-PermissionRequest, screens c1-0s, c1-3s, c1-8s | M3 |
| C3 | Allow at 3 s: the dialog closed and the command ran 35 ms after the hook exited; the transcript says `Allowed by PermissionRequest hook`. Deny at 3 s: `Denied by PermissionRequest hook`, and the turn went on; asked to, the model quoted the deny's message word for word. No `PostToolUse` or `PostToolUseFailure` follows a deny. | s2/3769266-PermissionRequest, s2/3784771-PermissionRequest | M3 |
| C4 | Yes, Yes with a note ("Tab to amend" on Yes adds a note to Claude and does not edit the command) and "No, and tell Claude what to do differently": no signal; the hook runs out its hold and Claude Code goes on without it. No and Esc: SIGTERM 30–80 ms after the key to every process of the hook's tree (its group child and a child in a session of its own alike), then SIGKILL 1.5–1.6 s later to whatever ignored the TERM. Neither runs `Stop`; the turn is interrupted. | Yes s1/3689244, s2/3728878; Yes with a note s5/4114164; No with feedback s2/3705029; No s2/3702020; Esc s2/3707174 (each `-PermissionRequest.sig`), and the outside watcher's log | M3 |
| C5 | Dropped. An allow printed 15 s in, after "No, and tell Claude…": nothing ran, no `PostToolUse`. A deny printed 12 s in, after Yes: the command had run and nothing changed. After No or Esc the hook is killed before it can print. | s2/3709011-PermissionRequest, s2/3728878-PermissionRequest | M3 |
| C6 | Yes, 6.0 s after the dialog opened, every time, while the hook held: `Notification` with `notification_type` `permission_prompt` and the message "Claude needs your permission". For a subagent, whose dialog waits for the hook, it came 6 s after the dialog, not the hook. | s1/3689694-Notification, s2/3787621-Notification, s3/4069859-Notification | M3 copy |
| C7 | A command matching an `ask` rule, in `acceptEdits` where it would otherwise run unasked: a dialog, a PermissionRequest (`permission_mode` `acceptEdits`, no `permission_suggestions`), and the hook's allow at 3 s took effect. A confined session (`CLAUDE_CODE_EVAL_CONFINED=1`): the allow at 3 s was ignored and the dialog stayed. See "Contradicts the plan". | s2/3867313-PermissionRequest, s4/4109366-PermissionRequest; fixture `claude-2.1.280-bash-ask-rule-accept-edits.json` | M3 copy |
| C8 | One dialog at a time, and the PermissionRequests one after another. Of two Bash calls in one message, the second's `PreToolUse` and PermissionRequest ran only once the first had been answered, 12 s in, and its tool had run, while the first's hook was still holding. So two hooks of one session can be alive at once, the older one already answered in the TUI. | s2/3786859-PermissionRequest, s2/3788147-PermissionRequest | M2 supersession, M3 |
| C9 | Claude Code waits for no lingering hook. After a TUI Yes the tool ran at once, the next tool's PermissionRequest started 2 s later, and `Stop` ran, all while the first hook still held. At `/exit` every live hook's tree got SIGTERM within a quarter of a second, one that ignored it SIGKILL 1.55 s later, then `SessionEnd` ran and Claude Code exited 2.2 s after `/exit`. Closing the pane did the same. The hook outlives Claude Code only when Claude Code is SIGKILLed: then it is reparented to the user's init, gets no signal, and runs out its hold. | s1/3689244, s1/3691426, s2/3954886, s5/4114164, s6b/4125612 (each `-PermissionRequest.sig`) | M3 (critical) |
| C10 | Honoured. With `"timeout": 5` and a 30 s hold: SIGTERM to the hook's tree at 5.00 s, SIGKILL 1.6 s later to the recorder that ignored it. The dialog stayed and nothing ran. | s3/4064178-PermissionRequest | M3 |
| C11 | Byte-identical in all 23 pairs of a PermissionRequest and the same call's `PostToolUse` or `PostToolUseFailure`, Bash, Write, Edit, NotebookEdit, WebFetch and an MCP tool among them. `PostToolUse` adds `tool_response`, `tool_use_id` and `duration_ms`; the request alone has `permission_suggestions`, when it has them. The dialog offers no way to edit the command. | the pairs in s1, s2, s5, print-notty; fixtures `claude-2.1.280-bash.json` and `claude-2.1.280-bash-posttooluse.json` are one call | M4 |
| C12 | `✳` 7–10 ms before the PermissionRequest hook starts, as the dialog opens, held for as long as a dialog is up, and held so while a background subagent ran (two runs, 6.5 s and 7.4 s of overlap). The spinner (`◐`/`◑` before the session's name, every 0.96 s) 20–65 ms after a TUI Yes, and 20 ms after a hook's allow. While a subagent's hook held, before its dialog was up, the title was the spinner. Titles are OSC 0. | osc logs of s1, s2, with the driver's key times | M5 |
| C13 | Bash `tool_input` keys seen: `command`, `description`, `timeout`, `run_in_background` (and `dangerouslyDisableSandbox` in the binary, not observed: no sandbox). Edit: `file_path`, `old_string`, `new_string`, `replace_all`. Write: `file_path`, `content`. NotebookEdit: `notebook_path`, `cell_id`, `new_source`. WebFetch: `url`, `prompt`. An MCP tool: `tool_name` `mcp__<server>__<tool>`, its arguments as `tool_input`, and a top-level `mcp_server`. MultiEdit is not offered to the model. Top-level keys: `session_id`, `transcript_path`, `cwd`, `scratchpad_dir` (not under `-p`), `prompt_id`, `permission_mode`, `hook_event_name`, `tool_name`, `tool_input`, and `permission_suggestions`, `agent_id`, `agent_type`, `mcp_server` when they apply. In `acceptEdits` no PermissionRequest runs for Write or for a `touch` in the project; one runs when something still asks. | s2/3833460, 3791994, 3792158, 3793851, 3794129, 3794680 (each `-PermissionRequest`); fixtures `claude-2.1.280-{bash-timeout-background,write,edit,notebookedit,webfetch,mcp}.json` | M2 allowlist, M7 |
| C14 | All four subagents came up in the background, the two asked for in the foreground included. Their requests carry `agent_id` (such as `ac87da6c09324f43e`) and `agent_type` (`general-purpose`), as do their `PreToolUse` and `PostToolUse`. The dialog waits for the hook: it was not on screen 1 s or 5 s into a 10 s hold and was at 12 s, after the hook exited with nothing, and a Yes in it ran the command. Background sessions started with `claude --bg` were not measured. | s2/3909615-PermissionRequest, s2/3944554-PermissionRequest; fixture `claude-2.1.280-bash-subagent.json` | M9 |
| C15 | It exists: a failed `ls` ran `PostToolUseFailure` with the request's `tool_input` byte for byte, `error`, `is_interrupt`, `tool_use_id` and `duration_ms`. A TUI No, an Esc and a hook's deny run none. | s2/3836400-PostToolUseFailure; fixture `claude-2.1.280-bash-posttoolusefailure.json` | M4 |

The hook itself, which M2 binds by: it is Claude Code's child, in a session and process group of
its own, with no controlling terminal (`tty_nr` 0, `/dev/tty` does not open), and Claude Code is
the nearest ancestor with one. Its environment passes on Claude Code's own (`TERM_PROGRAM`,
`CROOK_PANE_ID`) and adds `CLAUDECODE`, `CLAUDE_PROJECT_DIR`, `CLAUDE_CODE_SESSION_ID` (the
payload's `session_id`) and `CLAUDE_PID`.

---

## Codex CLI 0.149.1

Not measured: deferred by the owner on 2026-10-16. `script/probe-permission-hooks codex`
installs the recorder in the `hooks.json` of a `CODEX_HOME` of the project's own,
`.probe/codex-home`, and prints the `CODEX_HOME=… codex` to start Codex with. Codex holds the
hooks for review until they are trusted in `/hooks`, and keeps that trust, and the directory's,
in its `CODEX_HOME`'s `config.toml`, so none of it reaches `~/.codex`. That home starts empty
rather than copied, so no login is copied into it: `CODEX_HOME=… codex login` first, and
`--clean` removes that login with the rest.

| | Finding | Evidence | Gates |
|---|---|---|---|
| X1 | Not measured. What the TUI shows while a `HOLD=30` hook waits. | | M8 |
| X2 | Not measured. Esc or an interrupt during the wait: is the recorder's background child orphaned? | | M8 |
| X3 | Not measured. Payloads for a shell escalation, apply_patch, network access and request_permissions. | | M8 |
| X4 | Not measured. A second PermissionRequest hook added beside the first (`PROBE_SECOND=1`): is only the new one held for trust review? | | M8 |
| X5 | Not measured. Do the two hooks run concurrently? | | M8 |
| X6 | Not measured. Is `timeout` honoured (`PROBE_TIMEOUT`), and what kills what? | | M8 |

## OpenCode 1.18.33

Not measured: deferred by the owner on 2026-10-16. The recorder is the plugin
`.opencode/plugins/record.ts`, which replies through `client.postSessionIdPermissionsPermissionId`
when the variant says to.

| | Finding | Evidence | Gates |
|---|---|---|---|
| O1 | Not measured. The `permission.asked` payload for bash and for edit. | | M10 |
| O2 | Not measured. Does a reply through the plugin's client close the TUI's prompt? | | M10 |
| O3 | Not measured. What does a late reply return? | | M10 |
| O4 | Not measured. Does `reject` also reject the session's other pending asks? | | M10 |

## GitHub Copilot CLI 1.0.89

Not measured: deferred by the owner on 2026-10-16. The recorder is registered under both
`permissionRequest` and `PermissionRequest` in `.github/hooks/probe.json`, since which one fires is
P1. `app/tests/ask_fixtures.rs` fails on a Copilot fixture until P2 has said what its payload
carries.

| | Finding | Evidence | Gates |
|---|---|---|---|
| P1 | Not measured. Which key fires, `permissionRequest` or `PermissionRequest`? | | M11 |
| P2 | Not measured. The payload. | | M11 |
| P3 | Not measured. Is `{"behavior":"allow"}` on stdout honoured? | | M11 |
| P4 | Not measured. What does the TUI show while the hook waits? | | M11 |
| P5 | Not measured. Is `timeoutSec` honoured? | | M11 |
| P6 | Not measured. Does the hook have a controlling terminal? (the recorder's `.ps` says) | | M11 |

## Gemini CLI

Not measured, and no milestone uses it: none of its hooks can approve.

---

## Measuring again

```sh
script/probe-permission-hooks claude          # the project, the recorder, and how to start the agent
script/probe-permission-hooks claude variant HOLD=60 AFTER=5 TRAP=stay
script/probe-permission-hooks claude timeline # every hook run and signal, in seconds from the first
script/probe-permission-hooks --clean         # the project, the recorder and its processes, gone
```

`--clean` sends a leftover recorder, its children and its watcher a TERM, and a second later a
KILL to whichever stayed, as a `TRAP=stay` recorder does.

The script's header says what each variant does. It spends the agent's usage, so a person runs
it: the cheapest model, a prompt a case, and `--clean` at the end, since a recorder left where
an agent runs it holds every permission dialog it sees. `PROBE_DIR` set at setup keeps the
evidence out of the project, which `--clean` removes. A case that has to watch a SIGKILL needs a
watcher outside the agent's tree: the recorder's own is let go of to init, so a kill of the
hook's tree does not reach it, but it polls every 0.2 s.
