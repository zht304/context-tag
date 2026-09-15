use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct InfoArgs {
    /// Symbol name
    pub name: String,
    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_info(args: InfoArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let symbols = store.symbol_info(&args.name)?;

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = symbols
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.symbol.name,
                        "qualified_name": r.symbol.qualified_name,
                        "kind": r.symbol.kind.as_str(),
                        "file": r.file_path,
                        "line": r.symbol.line_start,
                        "scope": r.symbol.scope_kind.as_str(),
                        "type": r.symbol.declared_type,
                        "signature": r.symbol.signature,
                        "is_definition": r.symbol.is_definition,
                        "access": r.symbol.access_modifier,
                        "storage": r.symbol.storage_class,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        _ => {
            for r in &symbols {
                println!("Name:          {}", r.symbol.name);
                println!("Qualified:     {}", r.symbol.qualified_name);
                println!("Kind:          {}", r.symbol.kind.as_str());
                println!("File:          {}:{}", r.file_path, r.symbol.line_start + 1);
                println!("Scope:         {}", r.symbol.scope_kind.as_str());
                println!("Definition:    {}", r.symbol.is_definition);
                if let Some(t) = &r.symbol.declared_type {
                    println!("Type:          {}", t);
                }
                if let Some(sig) = &r.symbol.signature {
                    println!("Signature:     {}", sig);
                }
                if let Some(access) = &r.symbol.access_modifier {
                    println!("Access:        {}", access);
                }
                if let Some(storage) = &r.symbol.storage_class {
                    println!("Storage:       {}", storage);
                }
                println!();
            }
        }
    }
    Ok(())
}
