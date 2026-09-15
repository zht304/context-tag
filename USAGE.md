# Context Tag Usage

C/C++ 代码索引和符号分析工具，基于 tree-sitter 解析，支持符号定义、引用查找、调用图等。

## Build

```bash
cargo build -p ctxtag-cli
# Binary: target/debug/ctx
```

## Workflow

### 1. Index a project

```bash
# Index a project directory
ctx index /path/to/project

# Custom output path and file extensions
ctx index /path/to/project --db ./my_index.db -e c,h,cpp,hpp
```

Progress is logged every 1% (e.g. "Pass 1: 100/59312 files (1%)").
For large projects (Linux kernel ~60k files) this uses a two-pass streaming
approach that keeps memory under 300 MB regardless of project size.

### 2. Query symbols

```bash
# Find symbol definition
ctx def kmalloc

# Find all references (including stdlib functions with no local definition)
ctx ref kmalloc

# Reference with short output (file:line only)
ctx ref kmalloc --format short

# JSON output for tool integration
ctx ref kmalloc --format json

# Struct member references
ctx ref my_struct.my_field

# List struct/class members
ctx members task_struct

# Find callers/callees of a function
ctx callers schedule
ctx callees kmalloc
```

### 3. Browse symbols

```bash
# List all symbols (first 100)
ctx list

# Filter by kind (function, struct, variable, etc.)
ctx list --kind function --limit 50

# Filter by file
ctx list --file src/main.c

# Search by name (FTS)
ctx list --query "kmalloc"
```

### 4. Database management

```bash
# Show stats
ctx db stats

# Remove entries for deleted files
ctx db prune

# Delete the database
ctx db reset
```

## LSP Server

```bash
# Start the LSP server ( communicates over stdio )
ctx-lsp
```

Supported capabilities: Go to Definition, Find References, Hover,
Document Symbols, Workspace Symbols.

The server reads the database at the project root (`.ctxtag.db` by default).
