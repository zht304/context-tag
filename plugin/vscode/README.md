# Context Tag VSCode Extension

C/C++ symbol analysis and navigation powered by the `ctx-lsp` language server.

## Prerequisites

Build the language server from the repo root:

```bash
cargo build --release
```

This produces `target/release/ctx-lsp`. The extension resolves the binary in this order:

1. The `ctxtag.serverPath` setting (absolute path), if set.
2. `<repo>/target/release/ctx-lsp`, then `<repo>/target/debug/ctx-lsp` (works when running
   the extension from the repo, e.g. via F5).
3. `ctx-lsp` on `PATH`.

> When installed as a packaged VSIX, the repo is not reachable via a relative path, so set
> `ctxtag.serverPath` to the absolute path of the binary.

## Database

`ctx-lsp` loads its SQLite database from `{workspaceRoot}/.ctxtag.db` on startup, where
`workspaceRoot` is the `rootUri` sent by the client. Build the index first:

```bash
ctx --db /path/to/project/.ctxtag.db index /path/to/project --bulk
```

By default the client sends the opened workspace folder as `rootUri`. To force a specific
root (e.g. always use the kernel index regardless of which folder is open), set
`ctxtag.rootUri`.

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `ctxtag.serverPath` | `""` | Absolute path to the `ctx-lsp` binary. Empty = auto-detect. |
| `ctxtag.rootUri` | `""` | Workspace root used to find `{root}/.ctxtag.db`. Empty = the opened folder. |
| `ctxtag.trace.server` | `off` | Trace LSP traffic (`off`/`messages`/`verbose`) in the Output panel. |

## Installation

### From a packaged VSIX

```bash
code --install-extension ctxtag-0.1.1.vsix --force
```

### Development (F5)

1. Open the `plugin/vscode` folder in VS Code.
2. `npm install`
3. Press `F5` to launch the Extension Development Host.

## Example: Linux kernel

`~/ti-linux-kernel/.vscode/settings.json`:

```json
{
  "ctxtag.serverPath": "/home/thomas/context-tag/target/release/ctx-lsp",
  "ctxtag.rootUri": "file:///home/thomas/ti-linux-kernel",
  "C_Cpp.intelliSenseEngine": "disabled",
  "clangd.enable": false
}
```

Then open `~/ti-linux-kernel` and use `F12` (definition), `Shift+F12` (references),
hover, `Ctrl+T` (workspace symbol), `Ctrl+Shift+O` (document symbol).

> Disable the Microsoft C/C++ IntelliSense engine and clangd (as above) to avoid duplicate
> navigation providers.

## Features

Provided by the ctxtag LSP server for C and C++ files:

- Go to Definition
- Find References
- Hover (signature + documentation)
- Document Symbols / Workspace Symbols
- Document Highlight
