use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct IncludeArgs {
    /// File path
    pub file: String,
}

#[derive(Parser)]
pub struct IncludedByArgs {
    /// File path (header)
    pub file: String,
}

pub fn cmd_includes(args: IncludeArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let includes = store.find_includes(&args.file)?;

    for (path, line) in &includes {
        println!("{} (line {})", path, line + 1);
    }
    Ok(())
}

pub fn cmd_included_by(args: IncludedByArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let refs = store.find_included_by(&args.file)?;

    for (path, line) in &refs {
        println!("{} includes {} (line {})", path, args.file, line + 1);
    }
    Ok(())
}
