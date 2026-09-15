use clap::Parser;

#[derive(Parser)]
pub struct MacroArgs {
    /// Macro name
    pub name: String,
}

pub fn cmd_macro(_args: MacroArgs, _db_path: &str) -> anyhow::Result<()> {
    // TODO: Query macros table
    println!("Macro query for: {}", _args.name);
    Ok(())
}
