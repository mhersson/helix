# Installing Helix

This fork is not packaged by any operating system's package manager. Install it
by downloading a [pre-built binary](#pre-built-binaries) or by
[building from source](./building-from-source.md).

Note that:

- Packages of Helix in your operating system's package manager install the
  upstream editor, not this fork.

- To take full advantage of Helix, install the language servers for your
  preferred programming languages. See [Language servers](./lsp.md) for how
  Helix finds and uses them, and the [language support](./lang-support.md)
  page for the servers each language is configured with.

## Pre-built binaries

Download pre-built binaries from the [GitHub Releases page](https://github.com/mhersson/helix/releases).
Releases are tagged `vYYYY.MM.N`, for example `v2026.10.0`.
The tarball contents include an `hx` binary and a `runtime` directory.
To set up Helix:

1. Add the `hx` binary to your system's `$PATH` to allow it to be used from the command line.
2. Copy the `runtime` directory to a location that `hx` searches for runtime files. A typical location on Linux/macOS is `~/.config/helix/runtime`.

To see the runtime directories that `hx` searches, run `hx --health`. If necessary, you can override the default runtime location by setting the `HELIX_RUNTIME` environment variable.
