# crook-bin, the AUR package

Crook's release build for Arch Linux and Omarchy, as the AUR package `crook-bin`:

```sh
yay -S crook-bin               # or: omarchy pkg aur add crook-bin
```

It installs the binary from the release page's Linux archive (checked against that release's
`SHA256SUMS`), a `crook.desktop` entry tied to the window's app id (`crook`, so a launcher,
a dock and a window rule all find it), the icon in four sizes, and both licences. Nothing is
built from source: the release is the build.

The files here are what the AUR repository holds — `PKGBUILD`, `.SRCINFO` — and this directory
is where they are edited, so a change to the package is a pull request like any other. The third,
`crook.desktop`, is [`packaging/linux/crook.desktop`](../linux/crook.desktop): the one entry the
release archive and `script/install --desktop` install too, which `script/aur` copies in beside
the `PKGBUILD` when it builds and when it pushes.

## After a release

```sh
./script/aur 0.1.14            # bump, re-sum, build and check it here
git commit -am "chore(aur): crook-bin 0.1.14"   # through a pull request, as usual
./script/aur 0.1.14 --push     # publish to aur.archlinux.org
```

`--push` needs an AUR account whose SSH key is this machine's. `script/aur` refuses a version
that has no release yet, and a download whose digest is not the one its `SHA256SUMS` names.

`omarchy pkg add` itself installs only from the pacman repositories Omarchy configures
(`[core]`, `[extra]`, `[omarchy]`); getting there is the Omarchy maintainers' call, and an AUR
package people use is the usual first step.
