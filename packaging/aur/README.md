# AUR packaging

Source of truth for the `rusticprofile` AUR package. The AUR repository is a separate git
repository containing only `PKGBUILD` and `.SRCINFO`; these files are kept here so they are
reviewed alongside the code they package, and copied there on release.

## Use the recipes

```bash
just aur-verify           # build + namcap in a container; needs no Arch install
just aur-srcinfo          # regenerate .SRCINFO from the PKGBUILD
just aur-bump 0.2.0       # set pkgver, reset pkgrel, refresh sha256 from the real tarball
just aur-publish          # verify, confirm, clone, push
```

`aur-verify` installs `rustic` in the container **on purpose**. Without it the build still
passes while the rustic-backed integration tests skip themselves, so a green `makepkg`
proves considerably less than it looks like it does.

`.SRCINFO` is derived data — always `just aur-srcinfo`, never an editor. The AUR rejects a
`.SRCINFO` that disagrees with its `PKGBUILD`, and `aur-publish` catches that before the
push does.

## Releasing a new version

1. Tag and release upstream first — the PKGBUILD builds from the tag tarball, and
   `aur-bump` refuses a version that has no release.
2. `just aur-bump <version>` (bumps, resets `pkgrel`, refreshes the checksum, regenerates
   `.SRCINFO`).
3. `just aur-verify`.
4. `just aur-publish`.

`pkgver` tracks the **released** version, which is deliberately not the same as the version
in `Cargo.toml` on `main` — the repo moves on after a release, the package does not.

## Two things namcap warns about that are correct

- **`rustic` "may not be needed"** — namcap reads linked libraries, and rusticprofile links
  nothing against rustic. It *spawns* it. The whole design is that rustic does the backing
  up, so this is the one dependency that matters most and the one namcap cannot see.
- **`gcc-libs` "may not be needed"** — implicitly satisfied via `libgcc_s.so.1`, and the
  standard explicit dependency for Rust packages on Arch.
