# Helix Fork

A hard fork of helix-editor/helix (github.com/mhersson/helix), developed as
its own project on `main`. It forked from upstream at `ba40e5474`
(2026-09-29) and is never merged or rebased onto upstream again.

## Code Quality

Both `cargo fmt` and `cargo clippy` **must** pass before every commit. Run:

```sh
cargo fmt
cargo clippy --workspace --all-targets --features helix-term/integration
```

Upstream's integration tests carry a few existing clippy warnings
(`helix-term/tests/test/{helpers.rs,commands/write.rs,splits.rs}`); leave
those alone, but add no new ones. CI runs clippy with `-D warnings` but
without the `integration` feature for this reason.

After adding or changing commands or language options, run
`cargo xtask docgen` and commit the regenerated `book/src/generated/*.md`
(CI fails if they are out of date).

Integration tests: `cargo test -p helix-term --features integration --test integration`.

Dependabot PRs are reviewed and merged like any other change.

## Taking Changes from Upstream

The `upstream` remote is kept only for occasionally taking individual
commits we want (grammar/query updates, bug fixes, etc.):

```sh
git fetch upstream
git cherry-pick -x <commit>
```

Never merge or rebase onto upstream. Resolve any conflicts, then run
`cargo fmt`, `cargo clippy` and the tests before committing.

## Commit Ordering

Feature commits should come before README/docs commits that describe them.

Keep hunks in upstream hot spots (`helix-term/src/commands.rs`,
`helix-term/src/ui/editor.rs`, `helix-view/src/document.rs`,
`helix-view/src/view.rs`, `helix-core/src/doc_formatter.rs`) small: put
logic in the fork's own modules and call it with fully qualified paths
instead of editing the files' `use` blocks. This keeps cherry-picks from
upstream cheap.

## Releases

Tag `vYYYY.MM.N` (e.g. `v2026.10.0`) on `main` and push the tag; that
triggers `.github/workflows/release.yml`, which publishes the GitHub release.

## Fork Features

- **Markdown rendering** (`markdown-render` branch): in-buffer rendering of
  Markdown, toggled per document with `:markdown-render [on|off]` /
  `toggle_markdown_render`, on by default via `markdown-render = true` in
  `languages.toml`. Normal mode is fully rendered; insert mode shows the
  cursor's block raw.
  - `helix-core/src/text_annotations.rs` (`Conceal`) and
    `helix-core/src/conceal.rs`: generic conceal layer, cursor snapping and
    conceal-aware deletion.
  - `helix-view/src/markdown_render.rs`: decorations from the tree-sitter
    `markdown`/`markdown.inline` trees, computed on demand for the viewport.
  - Images: local `![alt](path)` images are drawn below their caption with
    Kitty graphics Unicode placeholders (kitty, Ghostty), see
    `helix-view/src/images/`. `[editor] kitty-graphics = "auto"|"on"|"off"`;
    inside tmux it needs `set -g allow-passthrough on`.
  - Mermaid diagrams: ```` ```mermaid ```` fences are rendered with merman
    (SVG) and resvg into the same image pipeline and drawn over the fence's
    own lines (`helix-view/src/images/diagram.rs`). The diagram theme follows
    the editor background unless the diagram sets its own `%%{init}%%` or a
    `theme:` in YAML frontmatter; syntax errors show `⚠ <message>` after the
    opening fence. merman needs the fork's Rust toolchain at 1.95
    (`rust-toolchain.toml`, `rust-version`); when upstream bumps its
    toolchain, keep the higher version.
  - Planned: Markdown-aware `w`/`b`/`e`.

## Building

`cargo build` or `cargo check --package helix-term` for faster iteration.
