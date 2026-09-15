mod commands;

use clap::{Parser, Subcommand};
use commands::*;

#[derive(Parser)]
#[command(name = "ctx", about = "C/C++ context tag analyzer")]
struct Cli {
    /// Path to the database file
    #[arg(short, long, global = true, default_value = ".ctxtag.db")]
    db: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a project directory
    Index(commands::index::IndexArgs),
    /// Incremental update of index
    Update(commands::index::UpdateArgs),
    /// Find symbol definition
    Def(commands::def::DefArgs),
    /// Find all references to a symbol
    Ref(commands::reference::RefArgs),
    /// List struct/class members
    Members(commands::members::MembersArgs),
    /// Find callers of a function
    Callers(commands::caller::CallerArgs),
    /// Find callees of a function
    Callees(commands::caller::CalleeArgs),
    /// Show detailed symbol info
    Info(commands::info::InfoArgs),
    /// List symbols with filters
    List(commands::list::ListArgs),
    /// Show includes of a file
    Includes(commands::includes::IncludeArgs),
    /// Show who includes a file
    IncludedBy(commands::includes::IncludedByArgs),
    /// Show macro definition
    Macro(commands::macro_cmd::MacroArgs),
    /// Database management
    Db(commands::db::DbArgs),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ctxtag=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Index(args) => cmd_index(args, &cli.db),
        Commands::Update(args) => cmd_update(args, &cli.db),
        Commands::Def(args) => cmd_def(args, &cli.db),
        Commands::Ref(args) => cmd_ref(args, &cli.db),
        Commands::Members(args) => cmd_members(args, &cli.db),
        Commands::Callers(args) => cmd_callers(args, &cli.db),
        Commands::Callees(args) => cmd_callees(args, &cli.db),
        Commands::Info(args) => cmd_info(args, &cli.db),
        Commands::List(args) => cmd_list(args, &cli.db),
        Commands::Includes(args) => cmd_includes(args, &cli.db),
        Commands::IncludedBy(args) => cmd_included_by(args, &cli.db),
        Commands::Macro(args) => cmd_macro(args, &cli.db),
        Commands::Db(args) => cmd_db(args, &cli.db),
    }
}
