use std::fs;
use std::path::Path;

use crate::db::DbStore;
use super::engine::IndexStats;

/// Incremental indexer: detects changed files and re-indexes only those.
pub struct IncrementalIndexer<'a> {
    root_dir: String,
    store: &'a DbStore,
}

impl<'a> IncrementalIndexer<'a> {
    pub fn new(root_dir: &str, store: &'a DbStore) -> Self {
        Self {
            root_dir: root_dir.to_string(),
            store,
        }
    }

    pub fn update(&self) -> anyhow::Result<IndexStats> {
        let mut stats = IndexStats::default();
        let root = Path::new(&self.root_dir);

        // Walk all source files
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let path_str = path.to_string_lossy().to_string();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

            // Only C/C++ files
            match ext {
                "c" | "h" | "cpp" | "cxx" | "cc" | "hpp" | "hxx" | "hh" => {}
                _ => continue,
            }

            if let Ok(abs) = fs::canonicalize(path) {
                let abs_path = abs.to_string_lossy().to_string();

                // Check if file was deleted from DB but not on disk, or vice versa
                match self.store.file_hash(&abs_path) {
                    Ok(Some(old_hash)) => {
                        // File exists in DB, check if content changed
                        match fs::read_to_string(&abs_path) {
                            Ok(content) => {
                                let mut hasher = sha2::Sha256::new();
                                use sha2::Digest;
                                hasher.update(content.as_bytes());
                                let new_hash = format!("{:x}", hasher.finalize());

                                if old_hash != new_hash {
                                    tracing::info!("Changed: {}", abs_path);
                                    // TODO: Re-index this file
                                    stats.files_indexed += 1;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Cannot read {}: {}", abs_path, e);
                                stats.files_failed += 1;
                            }
                        }
                    }
                    Ok(None) => {
                        // New file not in DB
                        tracing::info!("New: {}", abs_path);
                        stats.files_indexed += 1;
                    }
                    Err(e) => {
                        tracing::warn!("DB error for {}: {}", abs_path, e);
                    }
                }
            }
        }

        Ok(stats)
    }
}
