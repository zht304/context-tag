use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct MembersArgs {
    /// Struct/class/union name
    pub name: String,

    /// Show detailed info (type, access)
    #[arg(long)]
    pub detailed: bool,

    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_members(args: MembersArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let members = store.find_struct_members(&args.name)?;

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = members
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "name": m.symbol.name,
                        "kind": m.symbol.kind.as_str(),
                        "type": m.symbol.declared_type,
                        "line": m.symbol.line_start,
                        "access": m.symbol.access_modifier,
                        "file": m.file_path,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        _ => {
            if args.detailed {
                for m in &members {
                    let access = m
                        .symbol
                        .access_modifier
                        .as_deref()
                        .unwrap_or("default");
                    let typ = m
                        .symbol
                        .declared_type
                        .as_deref()
                        .unwrap_or("???");
                    println!(
                        "  {} {} {} (line {})",
                        access,
                        typ,
                        m.symbol.name,
                        m.symbol.line_start + 1,
                    );
                }
            } else {
                for m in &members {
                    println!("{}", m.symbol.name);
                }
            }
        }
    }
    Ok(())
}
