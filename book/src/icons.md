## Icons

The `[icons]` section of `config.toml` sets the glyphs Helix draws in the UI:
file and directory icons, completion and symbol kinds, diagnostics, version
control status, debugger markers and the symbols of the editor's virtual text
(visible whitespace, soft-wrap indicator, rulers). `[editor]` decides *what*
is shown, `[icons]` decides how it *looks*.

- [Icon values](#icon-values)
- [`[icons.fs]` Section](#iconsfs-section)
- [`[icons.kind]` Section](#iconskind-section)
- [`[icons.diagnostic]` Section](#iconsdiagnostic-section)
- [`[icons.vcs]` Section](#iconsvcs-section)
- [`[icons.dap]` Section](#iconsdap-section)
- [`[icons.ui]` Section](#iconsui-section)
- [Theme scopes](#theme-scopes)

Icons from patched fonts (such as [Nerd Fonts](https://www.nerdfonts.com/))
are off by default: `fs`, `kind` and `vcs` icons are only drawn after setting
`enable = true` in their section. Every other icon has a plain Unicode
default.

Example:

```toml
[icons.fs]
enable = true

[icons.fs.file.extension]
rs = { glyph = "🦀", fg = "#ff0000" }

[icons.fs.directory]
".git" = ""

[icons.kind]
enable = true
color = "󰚍 "

[icons.vcs]
enable = true
branch = "⌬"

[icons.diagnostic]
warning = "▲"

[icons.ui]
workspace = ""

[icons.ui.statusline]
separator = ""

[icons.ui.virtual]
tab = "→"
tabpad = "·"
ruler = "┊"
```

### Icon values

An icon is either a string with the glyph, or a table:

| Key         | Description |
| ---         | ---         |
| `glyph`     | The text drawn for the icon (at most 28 bytes, padding included: padding that does not fit is dropped) |
| `fg`        | Foreground color as `#rrggbb` |
| `bg`        | Background color as `#rrggbb` |
| `modifiers` | A list of modifiers, as in themes (`["bold", "italic"]`) |
| `underline` | A table with `color` (`#rrggbb`) and `style` (`"line"`, `"curl"`, ...) |
| `padding`   | Spaces around the glyph, as a table with `left` and `right`. Counts toward the 28-byte limit of `glyph`; if glyph and padding do not fit, the glyph is drawn without padding |

```toml
[icons.fs.file.filename]
"Cargo.toml" = { glyph = "", fg = "#dea584", padding = { left = 1, right = 2 } }
```

An icon you set replaces the built-in one. For `fs` icons, the style and
padding you leave out are taken from the built-in icon of that name, if any.

### `[icons.fs]` Section

File and directory icons in the pickers, the file explorer, the statusline
`file-type` element and the bufferline.

| Key         | Description | Default |
| ---         | ---         | ---     |
| `enable`    | Draw file and directory icons | `false` |
| `file.filename` | Icons by exact file name, such as `"Cargo.toml"`. Takes precedence over `file.extension` | Built-in list |
| `file.extension` | Icons by file extension without the dot, such as `rs` | Built-in list |
| `directory` | Icons by directory name. `default` is used for directories without their own icon, `open` for the directory being viewed (`..`) | Built-in list |

### `[icons.kind]` Section

Completion item and symbol kind icons, drawn in the completion menu and the
symbol pickers. Keys are kind names such as `function`, `method`, `struct`,
`enum_member`, `type_param`, `variable`, `keyword`, `snippet`, `folder` or
`file`.

| Key      | Description | Default |
| ---      | ---         | ---     |
| `enable` | Draw kind icons | `false` |
| `color`  | Swatch drawn for color completion items and document colors. Used even when `enable` is `false` | `"■"` |
| `<kind>` | The icon of that kind | Built-in list |

### `[icons.diagnostic]` Section

Used in the diagnostics gutter, the statusline, inline diagnostics and the
diagnostics pickers.

| Key       | Default |
| ---       | ---     |
| `hint`    | `"○"`   |
| `info`    | `"●"`   |
| `warning` | `"▲"`   |
| `error`   | `"■"`   |

### `[icons.vcs]` Section

Used in the statusline `version-control` element and the changed-files picker.

| Key        | Description |
| ---        | ---         |
| `enable`   | Draw version control icons (default `false`) |
| `branch`   | Before the branch name in the statusline |
| `added`    | Untracked files |
| `modified` | Modified files |
| `removed`  | Deleted files |
| `renamed`  | Renamed files |
| `conflict` | Files with conflicts |
| `ignored`  | Ignored files (not drawn anywhere yet) |

### `[icons.dap]` Section

Debugger markers in the breakpoints gutter.

| Key          | Description | Default |
| ---          | ---         | ---     |
| `verified`   | Verified breakpoint | `"●"` |
| `unverified` | Breakpoint not (yet) verified by the debug adapter | `"◯"` |
| `play`       | Current stack frame | `"▶"` |

### `[icons.ui]` Section

| Key         | Description | Default |
| ---         | ---         | ---     |
| `workspace` | Prefix of the statusline `workspace-diagnostics` element | `"W"` |

#### `[icons.ui.statusline]` Section

| Key         | Description | Default |
| ---         | ---         | ---     |
| `separator` | Drawn by the statusline `separator` element | `"│"` |

#### `[icons.ui.indicator]` Section

| Key        | Description | Default |
| ---        | ---         | ---     |
| `modified` | Statusline `file-modification-indicator` and the bufferline | `"[+]"` |
| `readonly` | Statusline `read-only-indicator` | `"[readonly]"` |

An `insecure` key is accepted too, but not used yet: the workspace trust
indicator is always `[⚠]`.

#### `[icons.ui.gutter]` Section

Markers of the `diff` gutter.

| Key        | Default |
| ---        | ---     |
| `added`    | `"▍"`   |
| `modified` | `"▍"`   |
| `removed`  | `"▔"`   |

#### `[icons.ui.virtual]` Section

Symbols of the editor's virtual text. Whitespace is only drawn as configured
by [`editor.whitespace.render`](./editor.md#editorwhitespace-section); the
trailing whitespace marks of `render = "trailing"` use the same glyphs.

| Key       | Description | Default |
| ---       | ---         | ---     |
| `space`   | Space | `"·"` |
| `nbsp`    | Non-breaking space | `"⍽"` |
| `nnbsp`   | Narrow non-breaking space | `"␣"` |
| `tab`     | First cell of a tab | `"→"` |
| `tabpad`  | Remaining cells of a tab: with `tabpad = "·"` a tab looks like `→···` (depending on tab width) | `" "` |
| `newline` | Line ending | `"⏎"` |
| `wrap`    | Inserted before soft-wrapped lines, highlighted with `ui.virtual.wrap`. A language's `soft-wrap.wrap-indicator` in `languages.toml` takes precedence | `"↪"` |
| `ruler`   | Drawn in the [`rulers`](./editor.md#editor-section) columns | `" "` |

The indent guide character is still set with
[`editor.indent-guides.character`](./editor.md#editorindent-guides-section);
an `indentation` key is accepted here but not used yet.

### Theme scopes

Themes can color icons that are not styled in `config.toml`:

| Scope | Description |
| ---   | ---         |
| `icons.file.<name>` | A file icon, by file name or extension (`icons.file.rs`, `icons.file.Cargo.toml`) |
| `icons.file.<name>.active` | The same icon on the active bufferline tab |
| `icons.directory.<name>` | A directory icon, by directory name |
| `icons.kind.<kind>` | A kind icon (`icons.kind.function`) |
| `ui.statusline.version-control` | The statusline `version-control` element |
| `ui.statusline.version-control.branch` | The branch icon of that element |
