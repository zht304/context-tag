use clap::Parser;
use ctxtag_core::DbStore;

#[derive(Parser)]
pub struct ListArgs {
    /// Filter by kind
    #[arg(long)]
    pub kind: Option<String>,
    /// Filter by file
    #[arg(long)]
    pub file: Option<String>,
    /// Filter by scope/container
    #[arg(long)]
    pub scope: Option<String>,
    /// Max results
    #[arg(long, default_value = "100")]
    pub limit: usize,
    /// Output format
    #[arg(long, default_value = "text")]
    pub format: String,
}

pub fn cmd_list(args: ListArgs, db_path: &str) -> anyhow::Result<()> {
    let store = DbStore::new(db_path)?;
    let symbols = store.list_symbols(
        args.kind.as_deref(),
        args.file.as_deref(),
        args.scope.as_deref(),
        args.limit,
    )?;

    match args.format.as_str() {
        "json" => {
            let out: Vec<serde_json::Value> = symbols
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.symbol.name,
                        "kind": r.symbol.kind.as_str(),
                        "file": r.file_path,
                        "line": r.symbol.line_start,
                        "scope": r.symbol.scope_kind.as_str(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        _ => {
            for r in &symbols {
                println!(
                    "[{:12}] {:30} {}:{} ",
                    r.symbol.kind.as_str(),
                    r.symbol.name,
                    file_short(&r.file_path),
                    r.symbol.line_start + 1,
                );
            }
        }
    }
    Ok(())
}

fn file_short(path: &str) -> String {
    use std::path::Path;
    let p = Path::new(path);
    let mut parts = vec![];
    if let Some(parent) = p.parent() {
        for component in parent.components().rev().take(2) {
            parts.insert(0, component.as_os_str().to_string_lossy().to_string());
        }
    }
    parts.push(p.file_name().unwrap_or_default().to_string_lossy().to_string());
    parts.join("/")
}
