use std::collections::HashSet;
use std::path::Path;

use crate::model::Include;

/// Resolves #include paths to actual file paths without compile_commands.json.
pub struct IncludeResolver {
    root_dirs: Vec<String>,
    /// Cache: include_text -> resolved_path
    cache: std::collections::HashMap<(String, String), String>,
}

impl IncludeResolver {
    pub fn new(root_dirs: Vec<String>) -> Self {
        Self {
            root_dirs,
            cache: std::collections::HashMap::new(),
        }
    }

    /// Resolve all includes for a project.
    pub fn resolve_all(
        &mut self,
        includes: &[Include],
        all_files: &[String],
    ) -> Vec<Include> {
        let all_dirs = self.collect_dirs(all_files);
        let mut resolved = Vec::new();

        for inc in includes {
            if let Some(resolved_path) = self.resolve_path(&inc.from_file, &inc.to_file, &all_dirs)
            {
                let mut r = inc.clone();
                r.to_file = resolved_path;
                resolved.push(r);
            }
        }

        resolved
    }

    fn resolve_path(
        &mut self,
        from_file: &str,
        include_text: &str,
        all_dirs: &HashSet<String>,
    ) -> Option<String> {
        let cache_key = (from_file.to_string(), include_text.to_string());
        if let Some(cached) = self.cache.get(&cache_key) {
            return Some(cached.clone());
        }

        let from_dir = Path::new(from_file)
            .parent()
            .map(|p| p.to_string_lossy().to_string());

        // Strategy 1: Same directory as including file
        if let Some(ref dir) = from_dir {
            let candidate = format!("{}/{}", dir, include_text);
            if Path::new(&candidate).exists() {
                self.cache.insert(cache_key, candidate.clone());
                return Some(candidate);
            }
        }

        // Strategy 2: Common subdirectories, under every root.
        let common_subdirs = ["", "include", "includes", "src", "lib", "libs", "headers", "inc"];
        for root in &self.root_dirs {
            for subdir in &common_subdirs {
                let candidate = if subdir.is_empty() {
                    format!("{}/{}", root, include_text)
                } else {
                    format!("{}/{}/{}", root, subdir, include_text)
                };
                if Path::new(&candidate).exists() {
                    self.cache.insert(cache_key, candidate.clone());
                    return Some(candidate);
                }
            }
        }

        // Strategy 3: All directories in the project
        for dir in all_dirs {
            let candidate = format!("{}/{}", dir, include_text);
            if Path::new(&candidate).exists() {
                self.cache.insert(cache_key, candidate.clone());
                return Some(candidate);
            }
        }

        // Not found - keep original path
        self.cache.insert(cache_key, include_text.to_string());
        Some(include_text.to_string())
    }

    fn collect_dirs(&self, all_files: &[String]) -> HashSet<String> {
        let mut dirs = HashSet::new();
        for file in all_files {
            if let Some(dir) = Path::new(file).parent() {
                dirs.insert(dir.to_string_lossy().to_string());
            }
        }
        dirs
    }
}
