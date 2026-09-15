use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct CallerArgs {
    /// Function name
    pub name: String,
    /// Max recursion depth
    #[arg(long, default_value = "1")]
    pub depth: u32,
    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

#[derive(Parser)]
pub struct CalleeArgs {
    /// Function name
    pub name: String,
    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_callers(args: CallerArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let callers = store.find_callers(&args.name, args.depth)?;

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = callers
                .iter()
                .map(|(file, caller, line)| {
                    serde_json::json!({ "file": file, "caller": caller, "line": line })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        _ => {
            for (file, caller, line) in &callers {
                println!("{} calls {} at {}:{}", caller, args.name, file, line + 1);
            }
        }
    }
    Ok(())
}

pub fn cmd_callees(args: CalleeArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let callees = store.find_callees(&args.name)?;

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = callees
                .iter()
                .map(|(file, line, callee)| {
                    serde_json::json!({ "file": file, "callee": callee, "line": line })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        _ => {
            for (file, line, callee) in &callees {
                println!("{} calls {} at {}:{}", args.name, callee, file, line + 1);
            }
        }
    }
    Ok(())
}
