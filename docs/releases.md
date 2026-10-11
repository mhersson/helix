## Checklist

Releases are versioned with the tag `vYYYY.MM.N`: the year, the month and a
counter that starts at 0 within the month. For example, the first release of
October 2026 is `v2026.10.0` and the next one that month is `v2026.10.1`.

* Make sure `main` is in the state to release and CI is green
* Write release notes by hand from `git log --oneline <previous-tag>..main`
* Tag and push
    * Switch to `main` and pull
    * `git tag -a v2026.10.0 -m "v2026.10.0" && git push origin v2026.10.0`
* Wait for the Release workflow to finish
    * Pushing a `v*` tag runs `.github/workflows/release.yml`
    * It builds `hx` with the `opt` profile for six targets: x86_64 and
      aarch64 for each of Linux, macOS and Windows. Each gets a `.tar.xz`
      (`.zip` on Windows) containing the binary and the `runtime` directory
    * On x86_64 Linux it also builds an AppImage and a `.deb`
    * It publishes everything, plus a source archive, as a GitHub release on
      `mhersson/helix` with attested build provenance
* Edit the new release on GitHub: use the tag as the title and put the release
  notes in the release body

The Cargo package versions are not bumped for a release. Archive and AppImage
names come from the tag (`helix-v2026.10.0-x86_64-linux.tar.xz`), and the
`.deb` version is the tag without the leading `v`.

The workflow runs only on `v*` tag pushes; the `env.preview` flag in it is just
a guard. A dry run would need a `workflow_dispatch` trigger added to
`release.yml`.

## Release notes

The notes are written by hand from the commits since the previous tag, and go
in the GitHub release body. `git log --oneline <previous-tag>..main` lists
them. Small changes like dependency or documentation updates, refactors and CI
work can be left out.
