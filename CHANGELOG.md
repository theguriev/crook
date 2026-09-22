# Changelog

Written by `script/release` from the commits between one tag and the next, and
lifted into the notes on the [releases page](https://github.com/theguriev/crook/releases)
by `release.yml`. Sections start at the first release cut that way; the ones
before it were described on their release pages and are listed here by tag.

## v0.1.12

[compare changes](https://github.com/theguriev/crook/compare/v0.1.11...v0.1.12)

### Features

- **tabs:** Hang a group's members off a rail under its branch ([#277](https://github.com/theguriev/crook/pull/277))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.11

[compare changes](https://github.com/theguriev/crook/compare/v0.1.10...v0.1.11)

### Fixes

- **ci:** Two things #274 left red on every platform ([#276](https://github.com/theguriev/crook/pull/276), [#274](https://github.com/theguriev/crook/issues/274))
- **panes:** Draw no chips over a screen a program has taken ([#275](https://github.com/theguriev/crook/pull/275))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.10

[compare changes](https://github.com/theguriev/crook/compare/v0.1.9...v0.1.10)

### Features

- Update Crook and its plugins, from the command line and from the window ([#274](https://github.com/theguriev/crook/pull/274))

### Fixes

- **plugins:** Leave a contribution that draws nothing out of its slot ([#273](https://github.com/theguriev/crook/pull/273))

### Refactors

- **release:** Read the release notes through script/release-notes ([#272](https://github.com/theguriev/crook/pull/272))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.9

[compare changes](https://github.com/theguriev/crook/compare/v0.1.8...v0.1.9)

### Features

- **tabs:** Mark a tab as waiting again from its menu ([#255](https://github.com/theguriev/crook/pull/255))
- **window:** Zoom the focused pane over the whole tab ([#257](https://github.com/theguriev/crook/pull/257))
- **shell:** Set CROOK_PANE_ID in every pane's shell ([#258](https://github.com/theguriev/crook/pull/258))
- **cli:** Print a skill file that teaches an agent to use Crook with --skill ([#260](https://github.com/theguriev/crook/pull/260))
- **tabs:** Find tabs by their status word in the search box and the palette ([#259](https://github.com/theguriev/crook/pull/259))
- **tabs:** Show a tab's number beside its title on request ([#263](https://github.com/theguriev/crook/pull/263))
- **agent:** Say what a waiting agent is waiting for ([#264](https://github.com/theguriev/crook/pull/264))
- **window:** Hide the tabs panel and bring it back on one chord ([#265](https://github.com/theguriev/crook/pull/265))
- **tabs:** Draw a row's status as a glyph on request ([#266](https://github.com/theguriev/crook/pull/266))
- **cli:** Print the installed plugins as JSON with --plugins --json ([#267](https://github.com/theguriev/crook/pull/267))
- **agent:** Print hooks for codex, gemini, copilot and opencode ([#268](https://github.com/theguriev/crook/pull/268))
- **tabs:** Roll the worst member's status up to a folded group's heading ([#269](https://github.com/theguriev/crook/pull/269))
- **tabs:** Say why a row's dot is what it is from its menu ([#270](https://github.com/theguriev/crook/pull/270))

### Fixes

- **session:** Keep a copy of a session file that did not read whole ([#254](https://github.com/theguriev/crook/pull/254))
- **links:** Follow a URL the terminal folded onto the next row ([#256](https://github.com/theguriev/crook/pull/256))
- **release:** Send a notarization upload again when it stalls ([#261](https://github.com/theguriev/crook/pull/261))
- **tabs:** Spell a test's attention as the enum #270 made it ([#271](https://github.com/theguriev/crook/pull/271), [#270](https://github.com/theguriev/crook/issues/270))

### Documentation

- **skill:** Say what CROOK_PANE_ID is for, and what it is not ([#262](https://github.com/theguriev/crook/pull/262))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.8

[compare changes](https://github.com/theguriev/crook/compare/v0.1.7...v0.1.8)

### CI

- Generate the changelog from commit titles with changelogen ([#253](https://github.com/theguriev/crook/pull/253))

### Before the convention

The other 46 commits in this release predate the title convention the
generator reads, so they are not grouped above. They are all in the compare
link: shortcut matching by physical key under a non-Latin layout, sub-line
trackpad scrolling, completion across a case difference, Windows links opened
without a shell, the plugin host bounding what a module can register and how
fast it can tick, and a run of tabs, find and palette fixes.

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## Before the changelog

- [v0.1.7](https://github.com/theguriev/crook/releases/tag/v0.1.7) — 2026-09-16
- [v0.1.6](https://github.com/theguriev/crook/releases/tag/v0.1.6) — 2026-09-11
- [v0.1.5](https://github.com/theguriev/crook/releases/tag/v0.1.5) — 2026-09-11
- [v0.1.4](https://github.com/theguriev/crook/releases/tag/v0.1.4) — 2026-09-11
- [v0.1.3](https://github.com/theguriev/crook/releases/tag/v0.1.3) — 2026-09-11
- [v0.1.2](https://github.com/theguriev/crook/releases/tag/v0.1.2) — 2026-09-11
- [v0.1.1](https://github.com/theguriev/crook/releases/tag/v0.1.1) — 2026-09-10
- [v0.1.0](https://github.com/theguriev/crook/releases/tag/v0.1.0) — 2026-09-10
