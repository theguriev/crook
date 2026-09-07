# crook_wasm

The sandbox a [Crook](https://github.com/theguriev/crook-plugins) plugin runs in: one `.wasm`
module, an interpreter with no imports but a handful, per-call fuel, a memory ceiling, and every
offset a guest hands back checked against its own memory before it is read. The vocabulary the
two sides speak is [`crook_plugin_api`](https://docs.rs/crook_plugin_api); this is the half that
runs it.

It is published for one job outside the terminal's own repository, and that job is the reason
the binary below exists.

## `crook-plugin-info`

```sh
cargo install crook_wasm            # or: cargo install crook_wasm --version 0.8
crook-plugin-info plugin.wasm
```

```json
{"abi":8,"id":"theguriev/pirate","name":"Claude Code usage","description":"…","version":"0.3.0","capabilities":["net:api.anthropic.com","file:~/.claude/.credentials.json"],"asks":["Reach api.anthropic.com","Read ~/.claude/.credentials.json"]}
```

A registry that builds a plugin from source has to say what it built — the id, the version, the
ABI it speaks, what it asks to be allowed to do — and every one of those is *inside* the module,
readable only by instantiating it and calling two exports. Reimplementing that read is a second
thing to keep in step with the host that decides whether a plugin loads at all, so the registry
runs the host's own reader instead.

`capabilities` are the keys a grant is written down as, which is deliberately the same
vocabulary the terminal keeps in `settings.json`: a store that says "you have already allowed
this" is comparing those strings.

Exit code **0** read it, **2** the module speaks another ABI — its number is the whole of the
answer, because the manifest below it is encoded against a shape this reader does not have —
and **1** for anything that is not a plugin. Install the reader whose version matches the ABI
you are indexing: this crate is versioned `0.<abi>.<patch>`, so `--version 0.8` is the reader
for ABI 8.

## Using the sandbox

`Sandbox::open` instantiates a module, checks its ABI, and hands back its manifest before any
of the plugin's own work runs. Everything after that is the host's: what a contribution means,
what a request is allowed to reach, and what happens to a plugin that traps. Nothing here
grants anything.
