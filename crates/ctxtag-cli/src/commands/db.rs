use clap::{Parser, Subcommand};
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct DbArgs {
    #[command(subcommand)]
    command: DbCommands,
}

#[derive(Subcommand)]
enum DbCommands {
    /// Show database statistics
    Stats,
    /// Remove entries for deleted files
    Prune {
        /// Project root to check against
        root: String,
    },
    /// Delete the database file
    Reset,
}

pub fn cmd_db(args: DbArgs, db_path: &str) -> anyhow::Result<()> {
    match args.command {
        DbCommands::Stats => {
            let store = DbStore::new(db_path)?;
            let stats = store.stats()?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        DbCommands::Prune { root } => {
            let store = DbStore::new(db_path)?;
            // TODO: Remove entries for files that no longer exist
            println!("Pruning {} from {}", root, db_path);
        }
        DbCommands::Reset => {
            std::fs::remove_file(db_path)?;
            println!("Database {} removed", db_path);
        }
    }
    Ok(())
}
