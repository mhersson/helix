# Changelog

This changelog covers the fork (mhersson/helix) since it branched from upstream
commit `ba40e5474` (2026-09-29). Earlier changes are in the Helix editor
project's own history.

## Unreleased

### Features

- Markdown rendering: in-buffer conceal of markup, tables, local images drawn
  with Kitty graphics placeholders (kitty, Ghostty) and Mermaid diagrams.
  Toggle per document with `:markdown-render [on|off]`.
- Markdown rendering: large h1–h3 headings in kitty with the text sizing
  protocol (OSC 66); `[editor] text-sizing = "auto" | "on" | "off"`. ATX
  closing sequences (`# Title ##`) are now concealed while rendering, with text
  sizing off too.
- Markdown rendering: tables get top and bottom borders (`┌─┬─┐`, `└─┴─┘`),
  each in a row of its own.
- Markdown rendering: with `[icons.fs]` enabled, code fences show their
  language's file icon in place of the opening `───` rule, and the closing
  fence is hidden. Fences without an icon keep the rules.
- mpls focus notifications: `mpls/editorDidChangeFocus` is sent to the Markdown
  preview language server (`[editor.lsp] mpls-focus-notify`).

### Ported upstream PRs

- #13133: inline git blame.
- #14876: inline completion (ghost text) via `textDocument/inlineCompletion`.
- #3958: configurable labels for custom menus and typable commands.
- #11497: `editor.rounded-corners` draws rounded borders.
- #14951: `native` cursor shape, drawn by the terminal.
- #15854: scrollable file preview in pickers.
- #15775: `undo_selection` and `redo_selection`.
- #14559: window resizing.
- #12320: custom typable commands in `[commands]`.
- #7215: trailing whitespace rendering (`whitespace.render = "trailing"`).
- #15573: breadcrumb navigation bar.
- #9951: diff provider autodetection and caching.
- #12022: Jujutsu diff provider.
- #14544: file watching and auto-reload.
- #12369: icons (`[icons]`).

### Fixes

- Control characters are no longer written raw to the terminal, so a document
  (or a file name, statusline or popup text) can't inject terminal escape
  sequences. In documents they are shown as Unicode control pictures (`␛`,
  `␇`, `␡`), and C1 controls as `�`.
- Fall back to git when a Jujutsu repository can't be used (colocated
  repository, untrusted workspace, `jj` missing or failing).
- Quote Jujutsu fileset paths and include `@` in the head revset, so paths with
  quotes or spaces get a diff base and the head name is correct.
- Window resizing no longer overflows the container (a panic in debug builds).
- `[editor.breadcrumb]` with only some keys set no longer fails to load the
  config, and the cursor column highlight stays below the bar.
- File watching no longer sends a duplicate notification for deleted paths or
  panics on stale watch interests.
- Auto-reload re-checks workspace trust before reading a diff base and updates
  every view of the reloaded document.
- Icon padding that doesn't fit no longer panics.
- Selection history no longer keeps entries inside concealed Markdown text.
- Trailing whitespace is not marked on Markdown hard breaks while Markdown is
  rendered.
- Markdown images and diagrams come back on `:redraw` after a program run from
  Helix (e.g. yazi or termagit through `:insert-output`) leaves the terminal's
  alternate screen.

### Breaking changes

- `[editor.statusline] separator` moved to `[icons.ui.statusline] separator`;
  a config that still sets it fails to load.
- `[editor.whitespace.characters]` moved to `[icons.ui.virtual]`; the old table
  is ignored.
- The editor-level `[editor.soft-wrap] wrap-indicator` is ignored; use
  `[icons.ui.virtual] wrap`. A language's `soft-wrap.wrap-indicator` still
  applies.
- The default wrap indicator is `↪` (was `↪ `).
- The theme key for trailing whitespace is `ui.virtual.trailing-whitespace`.
