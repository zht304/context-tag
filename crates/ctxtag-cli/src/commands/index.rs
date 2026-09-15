use clap::Parser;
use ctxtag_core::{DbStore, IndexEngine, indexer::IndexConfig};

#[derive(Parser)]
pub struct IndexArgs {
    /// Project root directory
    pub root: String,
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
    /// Project root directory
    pub root: String,
}

pub fn cmd_index(args: IndexArgs, db_path: &str) -> anyhow::Result<()> {
    let config = IndexConfig {
        root_dir: args.root,
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
    let config = IndexConfig {
        root_dir: args.root,
        db_path: db_path.to_string(),
        ..Default::default()
    };

    let engine = IndexEngine::new(config)?;
    let stats = engine.update()?;
    println!("Update complete:");
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}
