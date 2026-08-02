# AUR packaging

Source of truth for the `rusticprofile` AUR package. The AUR repository is a separate git
repository containing only `PKGBUILD` and `.SRCINFO`; these files are kept here so they are
reviewed alongside the code they package, and copied there on release.

## Verifying a change

Never push an unverified PKGBUILD — a broken one fails on the user's machine, not on yours.
This needs no Arch install:

```bash
podman run --rm -v "$PWD/packaging/aur:/pkg:ro,Z" archlinux:base-devel bash -c '
  pacman -Syu --noconfirm --quiet >/dev/null 2>&1
  pacman -S --noconfirm --quiet --needed namcap rust rustic gcc-libs >/dev/null 2>&1
  useradd -m builder; mkdir -p /home/builder/b && cp /pkg/PKGBUILD /home/builder/b/
  chown -R builder /home/builder/b; cd /home/builder/b
  su builder -c "makepkg --noconfirm" && namcap PKGBUILD && namcap *.pkg.tar.zst'
```

Install `rustic` in the container as shown: without it, `check()` still passes but the
rustic-backed integration tests silently skip themselves, so the build proves less than it
appears to.

Regenerate `.SRCINFO` with `makepkg --printsrcinfo` — never by hand. The AUR rejects a
`.SRCINFO` that disagrees with the `PKGBUILD`.

## Releasing a new version

1. Tag and release upstream first; the PKGBUILD builds from the tag tarball.
2. Bump `pkgver`, reset `pkgrel=1`, update `sha256sums` from the new tarball:
   `curl -sL <url> | sha256sum`
3. Rebuild and lint as above, regenerate `.SRCINFO`.
4. Copy both files into the AUR clone, commit, push.

`pkgver` tracks the **released** version, which is deliberately not the same as the version
in `Cargo.toml` on `main` — the repo moves on after a release, the package does not.

## Two things namcap warns about that are correct

- **`rustic` "may not be needed"** — namcap reads linked libraries, and rusticprofile links
  nothing against rustic. It *spawns* it. The whole design is that rustic does the backing
  up, so this is the one dependency that matters most and the one namcap cannot see.
- **`gcc-libs` "may not be needed"** — implicitly satisfied via `libgcc_s.so.1`, and the
  standard explicit dependency for Rust packages on Arch.
