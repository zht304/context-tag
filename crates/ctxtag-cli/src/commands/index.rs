use clap::Parser;
use ctxtag_core::{DbStore, IndexEngine, indexer::IndexConfig};

#[derive(Parser)]
pub struct IndexArgs {
    /// One or more project root directories (all merged into one index).
    /// Defaults to the current directory when omitted.
    pub roots: Vec<String>,
    /// Path to a file listing source files (one per line, `#`/blank ignored),
    /// e.g. `find src -name '*.c' > files.txt`. When given, this list is
    /// authoritative and the roots are ignored.
    #[arg(short = 'f', long, value_name = "PATH")]
    pub file_list: Option<String>,
    /// Comma-separated file extensions
    #[arg(short, long, default_value = "c,h,cpp,cxx,cc,hpp,hxx,hh")]
    pub ext: String,
    /// Comma-separated exclude patterns
    #[arg(short = 'x', long, default_value = "vendor,build,target,.git,node_modules")]
    pub exclude: String,
    /// Use bulk indexing mode (deferred index creation, ~1.7x faster)
    #[arg(long)]
    pub bulk: bool,
}

#[derive(Parser)]
pub struct UpdateArgs {
    /// One or more project root directories. Pass every root that belongs to
    /// this database — files under roots not listed here are treated as deleted.
    pub roots: Vec<String>,
    /// Path to a file listing source files (see `index --file-list`).
    #[arg(short = 'f', long, value_name = "PATH")]
    pub file_list: Option<String>,
}

fn resolve_roots(roots: Vec<String>) -> Vec<String> {
    if roots.is_empty() {
        vec![".".to_string()]
    } else {
        roots
    }
}

pub fn cmd_index(args: IndexArgs, db_path: &str) -> anyhow::Result<()> {
    let config = IndexConfig {
        root_dirs: resolve_roots(args.roots),
        file_list: args.file_list,
        db_path: db_path.to_string(),
        extensions: args.ext.split(',').map(|s| s.trim().to_string()).collect(),
        exclude_patterns: args.exclude.split(',').map(|s| s.trim().to_string()).collect(),
        bulk_mode: args.bulk,
    };

    let mut engine = IndexEngine::new(config)?;
    let stats = engine.index()?;
    println!("Indexing complete:");
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}

pub fn cmd_update(args: UpdateArgs, db_path: &str) -> anyhow::Result<()> {
    // Unlike `index`, an empty roots list must NOT default to "." — that would
    // register the cwd as a root and pull unrelated files in. Empty means "use
    // the roots already registered in this database".
    let config = IndexConfig {
        root_dirs: args.roots,
        file_list: args.file_list,
        db_path: db_path.to_string(),
        ..Default::default()
    };

    let mut engine = IndexEngine::new(config)?;
    let stats = engine.update()?;
    println!("Update complete:");
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}
