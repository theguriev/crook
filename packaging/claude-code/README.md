# crook, the Claude Code plugin

The plugin that connects Claude Code to Crook, from the marketplace this repository is
(`.claude-plugin/marketplace.json` at its root):

```sh
claude plugin marketplace add theguriev/crook
claude plugin install crook@crook
```

It carries two things, and installing it is Claude Code writing its own settings, so nobody
merges JSON and Crook opens no file it does not own:

- `hooks/hooks.json` — the hooks `crook --agent-hooks claude` prints, calling `"$CROOK_BIN"`
  (the binary every pane is told it belongs to, or `crook` on `PATH` when a pane has none) and
  exiting 0 without a word unless `TERM_PROGRAM` is `Crook` or `CROOK_PANE_ID` is set. In any
  other terminal they do nothing.
- `skills/crook/SKILL.md` — the skill `crook --skill` prints.

Both are copies of what lives in `app/src`, and the tests in `app/src/agent.rs` fail when they
drift: `hooks.json` has to be what `CLAUDE_EVENTS` builds behind the guard, and each command is
run under `sh` inside and outside a pane; `SKILL.md` has to be `app/src/skill.md` byte for
byte. Edit those, then copy the result here — the failing test prints the `hooks` it expects.

There is no `version` in `plugin.json` on purpose: without one, Claude Code takes the commit
the plugin was installed from as its version, so `claude plugin update crook@crook` picks up
whatever `main` has rather than waiting for somebody to bump a number here.
