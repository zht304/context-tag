use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct MacroArgs {
    /// Macro name
    pub name: String,

    /// Output format (text|json|short)
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_macro(args: MacroArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let macros = store.find_macro(&args.name)?;

    if macros.is_empty() {
        eprintln!("No macro named '{}' found", args.name);
        return Ok(());
    }

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = macros
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.symbol.name,
                        "kind": "macro",
                        "file": r.file_path,
                        "line": r.symbol.line_start + 1,
                        "signature": r.symbol.signature,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        "short" => {
            for r in &macros {
                println!("{}:{}", r.file_path, r.symbol.line_start + 1);
            }
        }
        _ => {
            for r in &macros {
                println!(
                    "(macro) {} [{}:{}]",
                    r.symbol.name,
                    r.file_path,
                    r.symbol.line_start + 1
                );
                if let Some(sig) = &r.symbol.signature {
                    println!("  {}", sig);
                }
            }
        }
    }
    Ok(())
}
