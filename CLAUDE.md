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

- **Markdown rendering**: in-buffer rendering of
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
  - Tables: `table()` records a `TableRef` (rows, delimiter cell widths, open
    sides, quote/indent prefix); `TableRows` reserves a top-border row below the
    line before the table and a bottom-border row below its last line, and
    `helix-term/src/ui/text_decorations/tables.rs` draws `┌─┬─┐`/`└─┴─┘` there.
    Both are registered last (after inline diagnostics) so the top border is
    always right above the table; revealed tables keep blank rows.
  - Large headings: in kitty, h1–h3 are drawn with the text sizing protocol
    (OSC 66) at 2×/1.5×/1.25× with one reserved row each
    (`helix-view/src/markdown_render.rs` heading records, `HeadingRows`,
    `map_click`; `helix-term/src/ui/text_decorations/headings.rs`). helix-tui
    knows multicell cells (`helix-tui/src/multicell.rs`): covered cells are
    never written and a changed band is erased, then redrawn. Payloads with
    control characters are never sent as OSC 66 (`is_safe_payload`,
    `write_origin`); such headings stay normal size. The heading on a cursor
    line is drawn at normal size with its row kept: in the focused view, and
    in other views while cursor-line blame or ghost text shows. Support is
    probed with CPR when the termina backend claims the terminal, only if
    `[editor] text-sizing = "auto"` at startup (`"auto"|"on"|"off"`,
    `helix-view/src/text_sizing.rs`).
  - Planned: Markdown-aware `w`/`b`/`e`.
- **mpls focus notifications**: `helix-term/src/handlers/mpls_focus.rs`,
  `[editor.lsp] mpls-focus-notify`.
- **Upstream PRs ported as single squashed commits** (author = PR author, body
  has `Upstream-PR:`/`Upstream-head:` and a "Fork adaptations:" list — read it
  before changing that code): #13133 inline blame, #14876 inline completion,
  #3958 keymap labels, #11497 rounded corners, #14951 native cursor, #15854
  scrollable picker preview, #15775 selection undo/redo, #14559 window resize,
  #12320 `[commands]`, #7215 trailing whitespace, #15573 breadcrumbs, #9951
  diff-source autodetect, #12022 Jujutsu provider, #14544 file watching /
  auto-reload, #12369 `[icons]`. Fork-specific behaviour to keep:
  - Conceal cursor snapping replaces the selection-history entry
    (`Document::set_selection_replacing_history`) instead of pushing one.
  - Trailing whitespace (`ui.virtual.trailing-whitespace`, glyphs from
    `[icons.ui.virtual]`) is not marked on Markdown hard breaks while
    rendering is on (`helix-view/src/markdown_render.rs`).
  - Image/diagram layout uses the view's inner height, below the breadcrumb
    bar.
  - helix-vcs: per-repo, refcounted provider registry. In a colocated
    `.jj`+`.git` repo, jj is used only when installed and the workspace is
    fully trusted, otherwise git (at Reduced trust, as before #12022).
    Revoking trust drops jj; jj args are escaped (TOML strings,
    `root-file:"..."` filesets). Live jj tests in
    `helix-vcs/src/jj/live_test.rs` skip without `jj` on PATH.
  - Auto-reload (`helix-term/src/handlers/auto_reload.rs`) goes through the
    `:reload` path and re-applies workspace trust on HEAD changes; the file
    watcher is disabled in integration tests.
  - Breaking config: `[editor.statusline] separator` moved to `[icons]` and
    fails to load if left in the old place.
  - Control characters are drawn as control pictures (␛ ␇ ␡, C1 as �) and
    never written raw to the terminal (`helix-core/src/control_chars.rs`,
    helix-tui `write_symbol`) — a file must never be able to inject escape
    sequences.

## Building

`cargo build` or `cargo check --package helix-term` for faster iteration.
