use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct DefArgs {
    /// Symbol name
    pub name: Option<String>,

    /// File path (for position-based lookup)
    #[arg(long)]
    pub file: Option<String>,

    /// Line number (0-based)
    #[arg(long)]
    pub line: Option<u32>,

    /// Column number (0-based)
    #[arg(long)]
    pub column: Option<u32>,

    /// Filter by kind
    #[arg(long)]
    pub kind: Option<String>,

    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_def(args: DefArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;

    let results = if let (Some(file), Some(line), Some(column)) =
        (&args.file, args.line, args.column)
    {
        if let Some(result) = store.find_definition_at(file, line, column)? {
            vec![result]
        } else {
            eprintln!("No symbol found at {}:{}:{}", file, line, column);
            return Ok(());
        }
    } else if let Some(ref name) = args.name {
        store.find_definitions(name)?
    } else {
        eprintln!("Provide --name or --file/--line/--column");
        return Ok(());
    };

    format_results(&results, &args.format)?;
    Ok(())
}

fn format_results(
    results: &[ctxtag_core::QueryResult],
    format: &str,
) -> anyhow::Result<()> {
    match format {
        "json" => {
            let out: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.symbol.name,
                        "kind": r.symbol.kind.as_str(),
                        "file": r.file_path,
                        "line": r.symbol.line_start,
                        "column": r.symbol.column_start,
                        "signature": r.symbol.signature,
                        "scope": r.symbol.scope_kind.as_str(),
                        "is_definition": r.symbol.is_definition,
                        "type": r.symbol.declared_type,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        "short" => {
            for r in results {
                println!("{}:{}", r.file_path, r.symbol.line_start + 1);
            }
        }
        "loc" => {
            for r in results {
                println!(
                    "{}:{}:{}",
                    r.file_path,
                    r.symbol.line_start + 1,
                    r.symbol.column_start + 1
                );
            }
        }
        _ => {
            for r in results {
                let def_tag = if r.symbol.is_definition { "(def) " } else { "(decl) " };
                println!(
                    "{}{}[{}:{}] {}",
                    def_tag,
                    r.symbol.kind.as_str(),
                    r.symbol.name,
                    r.file_path,
                    r.symbol.line_start + 1,
                );
                if let Some(sig) = &r.symbol.signature {
                    println!("  {}", sig);
                }
            }
        }
    }
    Ok(())
}
