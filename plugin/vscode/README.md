# Context Tag VSCode Extension

C/C++ symbol analysis and navigation powered by the ctxtag LSP server.

## Prerequisites

The `ctx-lsp` binary must be built and available at one of the following paths relative to the project root:

- `target/release/ctx-lsp` (preferred)
- `target/debug/ctx-lsp` (fallback)

To build the binary, run from the project root:

```bash
cargo build --release
```

## Installation (Development)

1. Open the `plugin/vscode` folder in VSCode.
2. Run `npm install` to install dependencies.
3. Press `F5` to launch the Extension Development Host.

The extension will activate when you open a `.c`, `.cpp`, or related file and will start the `ctx-lsp` server over stdio.

## Features

- Go to Definition
- Find References
- Hover information
- Document Symbols / Workspace Symbols
- Signature Help

All provided by the ctxtag LSP server for C and C++ source files.
