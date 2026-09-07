# crook_plugin_api

The vocabulary a sandboxed [Crook](https://github.com/theguriev/crook-plugins) plugin and its
host say to each other. Both sides link this crate: the terminal, and every plugin compiled to
`wasm32-unknown-unknown`. That is the whole reason it exists — a wire format written down twice
is a wire format that will disagree with itself.

```toml
[dependencies]
crook_plugin_api = "0.8"
```

## What a plugin is

A `.wasm` module in Crook's plugins directory, run in an interpreter with no imports but a
handful: no filesystem, no network, no clock of its own. It **describes** what it wants drawn —
a label, a badge, a meter, a hairline, something pressable, a panel hung under it, one of
Crook's own icons by name — and the host paints it. Nothing here is a colour, a pixel or a
font, which is why a plugin written today comes out right in a theme written years after it.

Anything it wants from the machine it has to **ask** for, with a [`Request`], and every request
is checked against what a person granted on the Plugins page. A [`Capability`] is phrased as
the sentence that dialog says, and each names what it covers rather than a category: one host
rather than "the network", one path rather than "your files", `cd {}` rather than "run
commands".

## The version is the ABI

The number after the zero is [`ABI_VERSION`]. A host accepts one ABI exactly and refuses every
other by name, so `crook_plugin_api = "0.8"` — Cargo's compatibility range for a `0.x` crate —
is the same statement as "built against Crook's plugin ABI 8". Depending on a wider range would
be a plugin that compiles and is then turned away at load.

## Writing one

```rust,ignore
use crook_plugin_api::{Manifest, Node, Size, Tone, ABI_VERSION};

fn manifest() -> Manifest {
    Manifest {
        abi: ABI_VERSION,
        id: "you/hello".into(),
        name: "Hello".into(),
        description: "A word in the header.".into(),
        version: "0.1.0".into(),
        // Nothing asked for is nothing to allow, and a plugin that asks for
        // nothing draws from the day it is installed.
        capabilities: Vec::new(),
    }
}

fn render() -> Node {
    Node::Text { text: "hello".into(), size: Size::Small, tone: Tone::Muted }
}
```

The exports a module has to have, the imports it is given, and the shape of a registration are
documented on the items in this crate. Working plugins to read:
[crook-emoji](https://github.com/theguriev/crook-emoji) (asks for nothing),
[crook-worktree](https://github.com/theguriev/crook-worktree) (one capability),
[crook-pirate](https://github.com/theguriev/crook-pirate) (a network host, a file, a panel and
a clock), [crook-chips](https://github.com/theguriev/crook-chips) (a plugin that acts).

Install one with `crook --install-plugin <path>`, or list it in
[the registry](https://github.com/theguriev/crook-plugins) so that everybody else can.

[`ABI_VERSION`]: https://docs.rs/crook_plugin_api/latest/crook_plugin_api/constant.ABI_VERSION.html
[`Capability`]: https://docs.rs/crook_plugin_api/latest/crook_plugin_api/enum.Capability.html
[`Request`]: https://docs.rs/crook_plugin_api/latest/crook_plugin_api/enum.Request.html
