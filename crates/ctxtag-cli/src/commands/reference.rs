use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct RefArgs {
    /// Symbol name (or "Struct.member" for member refs)
    pub name: String,

    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_ref(args: RefArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;

    // Check if this is a member reference (Struct.member)
    if let Some(dot_pos) = args.name.rfind('.') {
        let struct_name = &args.name[..dot_pos];
        let member_name = &args.name[dot_pos + 1..];
        let refs = store.find_member_references(struct_name, member_name)?;
        format_member_refs(&refs, &args.format)?;
    } else {
        let refs = store.find_references(&args.name)?;
        format_refs(&refs, &args.format)?;
    }
    Ok(())
}

fn format_refs(
    refs: &[(i64, String, u32, u32, String, String)],
    format: &str,
) -> anyhow::Result<()> {
    match format {
        "json" => {
            let out: Vec<serde_json::Value> = refs
                .iter()
                .map(|(_, file, line, col, kind, ctx)| {
                    serde_json::json!({
                        "file": file,
                        "line": line,
                        "column": col,
                        "kind": kind,
                        "context": ctx,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        "short" => {
            for (_, file, line, _, _, _) in refs {
                println!("{}:{}", file, line + 1);
            }
        }
        _ => {
            for (_, file, line, col, kind, ctx) in refs {
                println!(
                    "[{}] {}:{}:{}",
                    kind,
                    file,
                    line + 1,
                    col + 1
                );
                if !ctx.is_empty() {
                    println!("  {}", ctx);
                }
            }
        }
    }
    Ok(())
}

fn format_member_refs(
    refs: &[(String, u32, u32, String)],
    format: &str,
) -> anyhow::Result<()> {
    match format {
        "json" => {
            let out: Vec<serde_json::Value> = refs
                .iter()
                .map(|(file, line, col, ctx)| {
                    serde_json::json!({
                        "file": file,
                        "line": line,
                        "column": col,
                        "context": ctx,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        "short" => {
            for (file, line, _, _) in refs {
                println!("{}:{}", file, line + 1);
            }
        }
        _ => {
            for (file, line, col, ctx) in refs {
                println!("{}:{}:{}", file, line + 1, col + 1);
                if !ctx.is_empty() {
                    println!("  {}", ctx);
                }
            }
        }
    }
    Ok(())
}
