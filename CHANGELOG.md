# Changelog

Written by `script/release` from the commits between one tag and the next, and
lifted into the notes on the [releases page](https://github.com/theguriev/crook/releases)
by `release.yml`. Sections start at the first release cut that way; the ones
before it were described on their release pages and are listed here by tag.

## v0.1.20

[compare changes](https://github.com/theguriev/crook/compare/v0.1.19...v0.1.20)

### Features

- **tabs:** Show a working agent on its tab without hovering ([#468](https://github.com/theguriev/crook/pull/468))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.19

[compare changes](https://github.com/theguriev/crook/compare/v0.1.18...v0.1.19)

### Features

- **agent:** Say a pull request is a draft when the check finds one ([#421](https://github.com/theguriev/crook/pull/421))
- **tabs:** Reopen the tab closed last, where it was ([#459](https://github.com/theguriev/crook/pull/459))
- **blocks:** Bookmark a block and jump between bookmarks ([#460](https://github.com/theguriev/crook/pull/460))
- **find:** Tell capitals apart, or read the query as a regular expression ([#461](https://github.com/theguriev/crook/pull/461))
- **blocks:** Pin a block's command over the list while its top is scrolled away ([#462](https://github.com/theguriev/crook/pull/462))
- **blocks:** Filter a block's output to the lines that match ([#463](https://github.com/theguriev/crook/pull/463))
- **settings:** Open Crook at login ([#464](https://github.com/theguriev/crook/pull/464))
- **settings:** Make Crook the default terminal ([#465](https://github.com/theguriev/crook/pull/465))
- **settings:** An audible bell ([#466](https://github.com/theguriev/crook/pull/466))
- **terminal:** Draw iTerm2 inline images ([#467](https://github.com/theguriev/crook/pull/467))

### Performance

- **input:** Collapse a history's repeats with a set rather than a scan of what is kept ([#418](https://github.com/theguriev/crook/pull/418))

### Fixes

- **worktrees:** Keep clear of the COM0, LPT0 and superscript port names on Windows ([#416](https://github.com/theguriev/crook/pull/416))
- **input:** Move a command run again to the newest end of the history instead of repeating it ([#419](https://github.com/theguriev/crook/pull/419))
- **input:** Narrow the shell's completions by a lowercase word in any script, not only ASCII ([#420](https://github.com/theguriev/crook/pull/420))
- **tabs:** Count a row's changed lines under git's read deadline, like every other git call ([#422](https://github.com/theguriev/crook/pull/422))
- **plugins:** Give up on a keychain read that waits on a dialog, and read the file ([#423](https://github.com/theguriev/crook/pull/423))
- **links:** End a link at full-width punctuation instead of opening an address with it ([#424](https://github.com/theguriev/crook/pull/424))
- **links:** Reap the process that opens a link instead of leaving a zombie per click ([#426](https://github.com/theguriev/crook/pull/426))
- **themes:** Claim a new theme file by creating it, so a save never writes through a link ([#428](https://github.com/theguriev/crook/pull/428))
- **find:** Match the output's case in every script, not only ASCII ([#429](https://github.com/theguriev/crook/pull/429))
- **input:** Keep plain Enter, Tab and Backspace legacy under kitty's disambiguate mode ([#430](https://github.com/theguriev/crook/pull/430))
- **plugins:** Skip a tallied line that is not UTF-8 instead of dropping the rest of its file ([#431](https://github.com/theguriev/crook/pull/431))
- **input:** Paste several lines into a one-line field as one line ([#432](https://github.com/theguriev/crook/pull/432))
- **settings:** Keep a config file's mode when a save replaces it ([#433](https://github.com/theguriev/crook/pull/433))
- **control:** Line up crook pane list by display columns, so wide titles stay under their headings ([#434](https://github.com/theguriev/crook/pull/434))
- **control:** Keep the first whole line of a cut block when the cut lands on its start ([#435](https://github.com/theguriev/crook/pull/435))
- **shell:** Leave a running window's scratch directory alone when another window sweeps ([#436](https://github.com/theguriev/crook/pull/436))
- **store:** Keep "Checked just now" across a relaunch when the registry answers 304 ([#437](https://github.com/theguriev/crook/pull/437))
- **git:** Leave lock files and dot files out of a repository's branches ([#438](https://github.com/theguriev/crook/pull/438))
- **blocks:** Lay out a wiped block's command row the way the grid drew it ([#439](https://github.com/theguriev/crook/pull/439))
- **selection:** Stop a double click at a space the terminal folded in the last column ([#440](https://github.com/theguriev/crook/pull/440))
- **input:** Type what an input method composes into a program that took the screen ([#441](https://github.com/theguriev/crook/pull/441))
- **selection:** End a drag the pane was resized under instead of flickering an invisible selection ([#442](https://github.com/theguriev/crook/pull/442))
- **agent:** See a pull request opened with gh -R or --repo ([#443](https://github.com/theguriev/crook/pull/443))
- **agent:** Keep the OpenCode plugin from passing a lone dash as a title ([#444](https://github.com/theguriev/crook/pull/444))
- **plugins:** Let a plugin see the machine's time zone while it builds ([#445](https://github.com/theguriev/crook/pull/445))
- **tabs:** Keep a compact row from printing its directory on both lines ([#446](https://github.com/theguriev/crook/pull/446))
- **worktrees:** Ask about one kept checkout in the singular ([#447](https://github.com/theguriev/crook/pull/447))
- **tabs:** Leave a name alone when a rename is committed unchanged ([#448](https://github.com/theguriev/crook/pull/448))
- **plugins:** Give every plugin a toggle command of its own ([#449](https://github.com/theguriev/crook/pull/449))
- **plugins:** Draw a hover note at a panel's size, not at the size of the mark it explains ([#450](https://github.com/theguriev/crook/pull/450))
- **cli:** Read a short flag after --settings as a flag, not as a page name ([#451](https://github.com/theguriev/crook/pull/451))
- **cli:** Make a relative --shell path whole whether or not the window moves ([#452](https://github.com/theguriev/crook/pull/452))
- **render:** Draw nothing for a layer whose clip rounds to no pixels ([#453](https://github.com/theguriev/crook/pull/453))
- **input:** End a drag in the command field wherever the button comes up ([#454](https://github.com/theguriev/crook/pull/454))
- **tabs:** Make dragging a split divider, and double-clicking it, actually move it ([#455](https://github.com/theguriev/crook/pull/455))
- **tabs:** Keep a press on a divider's grab band out of the pane beside it ([#456](https://github.com/theguriev/crook/pull/456))
- **input:** Answer a press on the prompt row after a long command has scrolled the field ([#457](https://github.com/theguriev/crook/pull/457))
- **ui:** Stretch a flexible child across a Stretch flex, keeping its share ([#458](https://github.com/theguriev/crook/pull/458))

### Refactors

- **process:** Reap the programs Crook starts and forgets in one place ([#427](https://github.com/theguriev/crook/pull/427))

### Tests

- **workspace:** Hold a closed shell's pty open without leaving it a job to refuse exit over ([#417](https://github.com/theguriev/crook/pull/417))
- **control:** Wait for a dead socket to refuse before a test relies on it being dead ([#425](https://github.com/theguriev/crook/pull/425))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.18

[compare changes](https://github.com/theguriev/crook/compare/v0.1.17...v0.1.18)

### Fixes

- **plugins:** Read Claude Code's credentials from the keychain on macOS ([#415](https://github.com/theguriev/crook/pull/415))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.17

[compare changes](https://github.com/theguriev/crook/compare/v0.1.16...v0.1.17)

### Features

- **tabs:** Resize the tabs panel by dragging its edge ([#413](https://github.com/theguriev/crook/pull/413))
- **input:** Choose whether tab completes or takes the suggestion ([#414](https://github.com/theguriev/crook/pull/414))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.16

[compare changes](https://github.com/theguriev/crook/compare/v0.1.15...v0.1.16)

### Features

- **tabs:** Read Claude Code's status from its title without the plugin ([#412](https://github.com/theguriev/crook/pull/412))

### Fixes

- **input:** Paste an image into an agent with cmd-v ([#411](https://github.com/theguriev/crook/pull/411))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.15

[compare changes](https://github.com/theguriev/crook/compare/v0.1.14...v0.1.15)

### Features

- **update:** Replace a Crook.app whole from the release's disk image ([#409](https://github.com/theguriev/crook/pull/409))

### Fixes

- **tabs:** Open a new tab where the focused pane is working ([#410](https://github.com/theguriev/crook/pull/410))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.14

[compare changes](https://github.com/theguriev/crook/compare/v0.1.13...v0.1.14)

### Features

- **packaging:** Add crook-bin, the AUR package ([#377](https://github.com/theguriev/crook/pull/377))
- **agent:** Count a pane waiting behind another window and ask the desktop for a look ([#385](https://github.com/theguriev/crook/pull/385))
- **agent:** Post a desktop notification when a pane needs you behind another window ([#387](https://github.com/theguriev/crook/pull/387))
- **agent:** Post to Notification Center from Crook.app and badge the dock with the count ([#390](https://github.com/theguriev/crook/pull/390))
- **agent:** Read OSC 9, 777 and 99 as a pane asking for a look ([#386](https://github.com/theguriev/crook/pull/386))
- **session:** Bring agents back after a restart as an unsent resume line ([#389](https://github.com/theguriev/crook/pull/389))
- **control:** Answer `crook pane list` over a per-user socket ([#401](https://github.com/theguriev/crook/pull/401))
- **control:** Open a worker's tab from a pane with `crook tab new` ([#403](https://github.com/theguriev/crook/pull/403))
- **control:** Let a pane wait on, read and follow the tabs it opened ([#407](https://github.com/theguriev/crook/pull/407))
- **agent:** Connect Claude Code through a plugin in the crook repository ([#388](https://github.com/theguriev/crook/pull/388))
- **tabs:** Ask before a close ends agents that are still working ([#391](https://github.com/theguriev/crook/pull/391))
- **worktrees:** Ask where a new worktree's branch starts ([#392](https://github.com/theguriev/crook/pull/392))
- **worktrees:** Start an agent in a new worktree from the creator ([#394](https://github.com/theguriev/crook/pull/394))
- **worktrees:** Copy what .worktreeinclude names into a new worktree ([#395](https://github.com/theguriev/crook/pull/395))
- **diagnostics:** Keep a log file and a local crash report, and say so next launch ([#393](https://github.com/theguriev/crook/pull/393))
- **agent:** Show the pull request an agent opened on its row ([#400](https://github.com/theguriev/crook/pull/400))
- **launch:** Let a launcher start Crook with -e or a directory, and ship a desktop entry ([#406](https://github.com/theguriev/crook/pull/406))
- **worktrees:** Mark checkouts whose work has landed, and offer to tidy them away ([#396](https://github.com/theguriev/crook/pull/396))
- **tabs:** Count a branch's commits and lines since its base on the row ([#397](https://github.com/theguriev/crook/pull/397))
- **changes:** Show what the agent in a tab changed, in a read-only column ([#399](https://github.com/theguriev/crook/pull/399))
- **changes:** Send line comments to the tab's agent as one unsent paste ([#405](https://github.com/theguriev/crook/pull/405))
- **worktrees:** Finish a task, and delete branches proved to have landed ([#402](https://github.com/theguriev/crook/pull/402))

### Performance

- **panes:** Stop rebuilding the window for output in a pane nobody can see ([#398](https://github.com/theguriev/crook/pull/398))

### Fixes

- **worktrees:** Lock the checkout Crook makes while its window works in it ([#380](https://github.com/theguriev/crook/pull/380))
- **terminal:** Publish the snapshot before letting go of the terminal ([#381](https://github.com/theguriev/crook/pull/381))
- **shell:** Keep the shell-integration scratch private to its user ([#382](https://github.com/theguriev/crook/pull/382))
- **plugins:** Bound how deep a plugin's tree is decoded and drawn ([#383](https://github.com/theguriev/crook/pull/383))
- **plugins:** Stop a plugin's deeds from freezing the window ([#384](https://github.com/theguriev/crook/pull/384))

### Refactors

- **terminal:** Talk to the pty through a PtyLink seam ([#404](https://github.com/theguriev/crook/pull/404))

### Tests

- **agent:** Read the plugin's pull-request hook as the status it reports ([#408](https://github.com/theguriev/crook/pull/408))

### CI

- **release:** Build the Linux binary in Debian 11 and hold it to glibc 2.31 ([#378](https://github.com/theguriev/crook/pull/378))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

## v0.1.13

[compare changes](https://github.com/theguriev/crook/compare/v0.1.12...v0.1.13)

### Performance

- **about:** Ask whether the binary is writable only when there is an update ([#279](https://github.com/theguriev/crook/pull/279))
- **find:** Count up to each match once, not from the start every time ([#285](https://github.com/theguriev/crook/pull/285))

### Fixes

- **update:** Refuse to write over a binary that is already gone ([#278](https://github.com/theguriev/crook/pull/278))
- **tabs:** Keep a new or hopped tab out from among the pinned ones ([#280](https://github.com/theguriev/crook/pull/280))
- **about:** Say a plugin built for any other ABI is refused, not only a later one ([#281](https://github.com/theguriev/crook/pull/281))
- **agent:** Keep a semicolon at a title's edge from forging the message cut ([#282](https://github.com/theguriev/crook/pull/282))
- **keybindings:** Find an entry's comma past comments when cutting it out ([#283](https://github.com/theguriev/crook/pull/283))
- **keybindings:** Stop printing a chord that a longer or shorter one shadows ([#284](https://github.com/theguriev/crook/pull/284))
- **settings:** Write through a symlinked settings or keybindings file ([#286](https://github.com/theguriev/crook/pull/286))
- **completion:** Offer bash file names with a directory's slash and quoted spaces ([#287](https://github.com/theguriev/crook/pull/287))
- **shell:** Keep a subshell line's exit status when history skips the line ([#288](https://github.com/theguriev/crook/pull/288))
- **cli:** Refuse a flag --agent does not read instead of dropping it ([#289](https://github.com/theguriev/crook/pull/289))
- **worktrees:** Keep a detached checkout's commits from going with it ([#290](https://github.com/theguriev/crook/pull/290))
- **git:** Read a reftable repository's placeholder HEAD as no branch ([#291](https://github.com/theguriev/crook/pull/291))
- **plugins:** Refuse a module installed under one of Crook's own ids ([#294](https://github.com/theguriev/crook/pull/294))
- **store:** Check a downloaded module is the plugin the list offered, on every path ([#295](https://github.com/theguriev/crook/pull/295))
- **ui:** Draw a switch's knob in a colour that reads on the accent ([#312](https://github.com/theguriev/crook/pull/312))
- **plugins:** Keep a card's pictures when an older decode lands after them ([#313](https://github.com/theguriev/crook/pull/313))
- **settings:** Keep a failing settings save on the page past a keybindings save ([#314](https://github.com/theguriev/crook/pull/314))
- **keybindings:** Say on the page when the keybindings file cannot be read ([#315](https://github.com/theguriev/crook/pull/315))
- **worktrees:** Hand a removal's answer only to the menu that asked ([#316](https://github.com/theguriev/crook/pull/316))
- **blocks:** Keep what a command prints after clearing the screen ([#317](https://github.com/theguriev/crook/pull/317))
- **blocks:** Reflow the shell's block when its screen comes back ([#318](https://github.com/theguriev/crook/pull/318))
- **store:** Offer no update to a plugin running from its build ([#319](https://github.com/theguriev/crook/pull/319))
- **plugins:** Forget a closed picker when its row is chosen ([#321](https://github.com/theguriev/crook/pull/321))
- **tabs:** Pin the only tab left in a group without moving the group ([#322](https://github.com/theguriev/crook/pull/322))
- **tabs:** Keep pins in front when two ungrouped runs join ([#323](https://github.com/theguriev/crook/pull/323))
- **session:** Save a tab or pane rename as soon as it is made ([#324](https://github.com/theguriev/crook/pull/324))
- **session:** Name a new tab past every name that came back ([#325](https://github.com/theguriev/crook/pull/325))
- **history:** Read a multi-line zsh command as one entry ([#326](https://github.com/theguriev/crook/pull/326))
- **history:** Unmetafy a zsh history before reading it as UTF-8 ([#327](https://github.com/theguriev/crook/pull/327))
- **links:** One link from every cell, and none from inside a word ([#329](https://github.com/theguriev/crook/pull/329))
- **keys:** Send the capital for Alt+Shift+letter ([#330](https://github.com/theguriev/crook/pull/330))
- **mouse:** Send nothing for a mouse's side buttons ([#331](https://github.com/theguriev/crook/pull/331))
- **links:** Keep a URL whole across a wide character in it ([#332](https://github.com/theguriev/crook/pull/332))
- **copy:** Copy a tab as one tab, not the blanks it jumped ([#333](https://github.com/theguriev/crook/pull/333))
- **blocks:** Keep a scrolled-up list on its rows when old blocks go ([#334](https://github.com/theguriev/crook/pull/334))
- **update:** Unpack into a directory this run made, and nobody else ([#335](https://github.com/theguriev/crook/pull/335))
- **update:** Let the archive take as long as it keeps arriving ([#336](https://github.com/theguriev/crook/pull/336))
- **zoom:** Tell the pty the new cell size even when the grid holds ([#337](https://github.com/theguriev/crook/pull/337))
- **tabs:** Bring the tabs up before going to the pane that is waiting ([#339](https://github.com/theguriev/crook/pull/339))
- **tabs:** Wash a tab's row for any of its panes that is asking ([#341](https://github.com/theguriev/crook/pull/341))
- **plugins:** Quote a typed argument so fish reads it as one word too ([#342](https://github.com/theguriev/crook/pull/342))
- **plugins:** Forget a plugin's grant from the settings as well ([#343](https://github.com/theguriev/crook/pull/343))
- **fish:** Install Crook's completion on a fish that marks its own prompt ([#344](https://github.com/theguriev/crook/pull/344))
- **fish:** Escape a completion the way fish's own Tab does ([#345](https://github.com/theguriev/crook/pull/345))
- **zsh:** Offer a completion the way it has to be typed ([#347](https://github.com/theguriev/crook/pull/347))
- **zsh:** Read a completion's word as text, not as a pattern ([#349](https://github.com/theguriev/crook/pull/349))
- **completion:** Complete past a space with a backslash before it ([#350](https://github.com/theguriev/crook/pull/350))
- **agent:** Cut a report where the parser would, and say so ([#351](https://github.com/theguriev/crook/pull/351))
- **bell:** Hand a burst of bells over as one ([#352](https://github.com/theguriev/crook/pull/352))
- **theme:** Let a trailing comment on a block line be a comment ([#353](https://github.com/theguriev/crook/pull/353))
- **theme:** Read backslash escapes in a double-quoted value ([#354](https://github.com/theguriev/crook/pull/354))
- **keybindings:** Keep the comments in an array with no entries ([#355](https://github.com/theguriev/crook/pull/355))
- **keybindings:** Let the recorder skip AltGr as a held modifier ([#356](https://github.com/theguriev/crook/pull/356))
- **settings:** Read a plugin grant back exactly as it was written ([#357](https://github.com/theguriev/crook/pull/357))
- **settings:** Remove the temporary when a save fails at the write ([#358](https://github.com/theguriev/crook/pull/358))
- **find:** Keep a space printed in the last column of a folded row ([#359](https://github.com/theguriev/crook/pull/359))
- **session:** Keep a session file that is not UTF-8 ([#360](https://github.com/theguriev/crook/pull/360))
- **session:** Restore a pinned tab's group in place, and the selection with it ([#361](https://github.com/theguriev/crook/pull/361))
- **history:** Keep the backslash a command really ends in ([#362](https://github.com/theguriev/crook/pull/362))
- **history:** Find fish's history where fish keeps it on macOS ([#363](https://github.com/theguriev/crook/pull/363))
- **history:** Drop the whole zsh entry a tail read starts inside ([#364](https://github.com/theguriev/crook/pull/364))
- **history:** Read zsh history from the file the pane's zsh writes ([#365](https://github.com/theguriev/crook/pull/365))
- **git:** Stop taking a submodule under worktrees/ for a linked worktree ([#367](https://github.com/theguriev/crook/pull/367))
- **git:** Find the repository a symlinked directory is in ([#368](https://github.com/theguriev/crook/pull/368))
- **tabs:** Keep a lone member's move from splitting another group ([#369](https://github.com/theguriev/crook/pull/369))
- **editor:** Leave the caret after a character an edit joined ([#370](https://github.com/theguriev/crook/pull/370))
- **editor:** Keep word motion on character boundaries ([#371](https://github.com/theguriev/crook/pull/371))
- **completion:** Drop an answer about a line that has since changed ([#372](https://github.com/theguriev/crook/pull/372))
- **completion:** Keep the space a candidate escaped at its end ([#373](https://github.com/theguriev/crook/pull/373))
- **input:** Send F3 and report text the way the kitty protocol says ([#374](https://github.com/theguriev/crook/pull/374))
- **osc7:** Refuse a working directory that is not a path ([#375](https://github.com/theguriev/crook/pull/375))

### Documentation

- **readme:** Count the window's commands as the tables do, and check it ([#292](https://github.com/theguriev/crook/pull/292))
- **plugin api:** Say what a tally's grant is, and what crook_run is handed ([#293](https://github.com/theguriev/crook/pull/293))
- **skill:** Teach --message, and fail the test when the skill drops a flag ([#366](https://github.com/theguriev/crook/pull/366))
- **readme:** Say OSC 52 writes reach the clipboard, and reads do not ([#376](https://github.com/theguriev/crook/pull/376))

### Tests

- **shells:** Wait for a quarter second of quiet, not four quiet reads ([#296](https://github.com/theguriev/crook/pull/296))
- **terminal:** Prove the shell printed, not that the pty echoed the command ([#297](https://github.com/theguriev/crook/pull/297))
- **workspace:** Sleep past the timeout where only an interrupt may end it ([#298](https://github.com/theguriev/crook/pull/298))
- **terminal:** Check a child started at $HOME, not somewhere under it ([#299](https://github.com/theguriev/crook/pull/299))
- **terminal:** Check a paste's bracket in the echo, where an escape can be seen ([#300](https://github.com/theguriev/crook/pull/300))
- **tabs:** Make three tab-strip tests fail when what they name breaks ([#301](https://github.com/theguriev/crook/pull/301))
- **session:** Restore a side-by-side split as well as a stacked one ([#302](https://github.com/theguriev/crook/pull/302))
- **keybindings:** Check a plugin's chord cannot take one the window ships ([#303](https://github.com/theguriev/crook/pull/303))
- **palette:** Bind the row the alphabet puts last, not first ([#304](https://github.com/theguriev/crook/pull/304))
- **store:** Make four index tests fail when what they name breaks ([#305](https://github.com/theguriev/crook/pull/305))
- **palette:** Pin the panes' directory in the waiting-rows test ([#306](https://github.com/theguriev/crook/pull/306))
- **plugins:** Make three sandbox boundary tests fail when the boundary breaks ([#307](https://github.com/theguriev/crook/pull/307))
- **workspace:** Make four window tests fail when what they name breaks ([#308](https://github.com/theguriev/crook/pull/308))
- **plugin host:** Make three sandbox limit tests fail when the limit goes ([#309](https://github.com/theguriev/crook/pull/309))
- **plugin host:** Cover the table ceiling and the host's reads of guest pointers ([#310](https://github.com/theguriev/crook/pull/310))
- **ui:** Make the flex, container and nested-scroll tests fail when those break ([#311](https://github.com/theguriev/crook/pull/311))
- **selection:** Keep the triple-click line off the command's row ([#320](https://github.com/theguriev/crook/pull/320))
- **shells:** Retype the line after ctrl-c until the shell runs it ([#328](https://github.com/theguriev/crook/pull/328))
- **palette:** A setting chosen in the palette opens with its row found ([#338](https://github.com/theguriev/crook/pull/338))
- **search:** Keep the checkout's path out of the status-word searches ([#340](https://github.com/theguriev/crook/pull/340))

### CI

- Install fish so the fish tests run instead of skipping ([#346](https://github.com/theguriev/crook/pull/346))
- Run the zsh tests on Linux too, and expect Linux's history file ([#348](https://github.com/theguriev/crook/pull/348))

### ❤️ Contributors

- Eugen Guriev ([@theguriev](https://github.com/theguriev))

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
