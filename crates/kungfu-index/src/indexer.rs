use anyhow::Result;
use chrono::Utc;
use kungfu_config::KungfuConfig;
use kungfu_parse::{Parser, RawCall, RawImport};
use kungfu_storage::JsonStore;
use kungfu_types::file::{FileEntry, Language};
use kungfu_types::relation::{Relation, RelationKind};
use kungfu_types::symbol::Symbol;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::scanner;

pub struct Indexer<'a> {
    root: std::path::PathBuf,
    config: KungfuConfig,
    store: &'a JsonStore,
    parser: Parser,
}

pub struct IndexStats {
    pub total_files: usize,
    pub new_files: usize,
    pub changed_files: usize,
    pub removed_files: usize,
    pub symbols_extracted: usize,
}

impl<'a> Indexer<'a> {
    pub fn new(root: &Path, config: KungfuConfig, store: &'a JsonStore) -> Self {
        Self {
            root: root.to_path_buf(),
            config,
            store,
            parser: Parser::new(),
        }
    }

    pub fn index_full(&mut self) -> Result<IndexStats> {
        info!("starting full index of {}", self.root.display());

        let paths = scanner::scan_files(&self.root, &self.config)?;
        let mut files = Vec::new();
        let mut fingerprints = HashMap::new();
        let mut all_symbols = Vec::new();
        let mut all_imports: Vec<(String, Vec<RawImport>)> = Vec::new();
        let mut all_calls: Vec<RawCall> = Vec::new();

        for path in &paths {
            match self.index_file(path) {
                Ok((entry, symbols, imports, calls)) => {
                    fingerprints.insert(entry.path.clone(), entry.hash.clone());
                    if !imports.is_empty() {
                        all_imports.push((entry.path.clone(), imports));
                    }
                    all_calls.extend(calls);
                    all_symbols.extend(symbols);
                    files.push(entry);
                }
                Err(e) => {
                    warn!("failed to index {}: {}", path.display(), e);
                }
            }
        }

        let relations = Self::build_relations(&files, &all_imports, &all_symbols, &all_calls);

        let stats = IndexStats {
            total_files: files.len(),
            new_files: files.len(),
            changed_files: 0,
            removed_files: 0,
            symbols_extracted: all_symbols.len(),
        };

        self.store.save_files(&files)?;
        self.store.save_symbols(&all_symbols)?;
        self.store.save_relations(&relations)?;
        self.store.save_fingerprints(&fingerprints)?;

        info!(
            "indexed {} files, {} symbols, {} relations",
            stats.total_files, stats.symbols_extracted, relations.len()
        );
        Ok(stats)
    }

    pub fn index_incremental(&mut self) -> Result<IndexStats> {
        let old_fingerprints = self.store.load_fingerprints()?;
        let old_files = self.store.load_files()?;
        let old_symbols = self.store.load_symbols()?;

        let paths = scanner::scan_files(&self.root, &self.config)?;

        let mut new_fingerprints = HashMap::new();
        let mut new_files = Vec::new();
        let mut new_symbols = Vec::new();
        let mut all_imports: Vec<(String, Vec<RawImport>)> = Vec::new();
        let mut all_calls: Vec<RawCall> = Vec::new();

        let mut stats = IndexStats {
            total_files: 0,
            new_files: 0,
            changed_files: 0,
            removed_files: 0,
            symbols_extracted: 0,
        };

        // Build set of current paths
        let current_paths: std::collections::HashSet<String> = paths
            .iter()
            .filter_map(|p| p.strip_prefix(&self.root).ok())
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        for path in &paths {
            let rel_path = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let content = match std::fs::read(path) {
                Ok(c) => c,
                Err(e) => {
                    warn!("cannot read {}: {}", path.display(), e);
                    continue;
                }
            };
            let hash = blake3::hash(&content).to_hex().to_string();

            if let Some(old_hash) = old_fingerprints.get(&rel_path) {
                if *old_hash == hash {
                    // Unchanged — keep old data
                    if let Some(old_file) = old_files.iter().find(|f| f.path == rel_path) {
                        new_files.push(old_file.clone());
                        let file_symbols: Vec<_> = old_symbols
                            .iter()
                            .filter(|s| s.file_id == old_file.id)
                            .cloned()
                            .collect();
                        new_symbols.extend(file_symbols);
                    }
                    new_fingerprints.insert(rel_path, hash);
                    continue;
                }
                stats.changed_files += 1;
            } else {
                stats.new_files += 1;
            }

            match self.index_file_with_content(path, content) {
                Ok((entry, symbols, imports, calls)) => {
                    new_fingerprints.insert(entry.path.clone(), entry.hash.clone());
                    if !imports.is_empty() {
                        all_imports.push((entry.path.clone(), imports));
                    }
                    all_calls.extend(calls);
                    new_symbols.extend(symbols);
                    new_files.push(entry);
                }
                Err(e) => {
                    warn!("failed to index {}: {}", path.display(), e);
                }
            }
        }

        // Count removed
        for old_path in old_fingerprints.keys() {
            if !current_paths.contains(old_path) {
                stats.removed_files += 1;
            }
        }

        stats.total_files = new_files.len();
        stats.symbols_extracted = new_symbols.len();

        // Rebuild relations from all imports and calls
        let relations = Self::build_relations(&new_files, &all_imports, &new_symbols, &all_calls);

        self.store.save_files(&new_files)?;
        self.store.save_symbols(&new_symbols)?;
        self.store.save_relations(&relations)?;
        self.store.save_fingerprints(&new_fingerprints)?;

        info!(
            "incremental index: {} total, {} new, {} changed, {} removed, {} symbols, {} relations",
            stats.total_files,
            stats.new_files,
            stats.changed_files,
            stats.removed_files,
            stats.symbols_extracted,
            relations.len()
        );
        Ok(stats)
    }

    /// Index only the specified files (by relative path), keeping everything else unchanged.
    pub fn index_only(&mut self, changed_paths: &[String]) -> Result<IndexStats> {
        let old_fingerprints = self.store.load_fingerprints()?;
        let old_files = self.store.load_files()?;
        let old_symbols = self.store.load_symbols()?;
        let old_relations = self.store.load_relations()?;

        let changed_set: std::collections::HashSet<&str> =
            changed_paths.iter().map(|s| s.as_str()).collect();

        let mut new_fingerprints = old_fingerprints.clone();
        let mut new_files: Vec<FileEntry> = Vec::new();
        let mut new_symbols: Vec<Symbol> = Vec::new();
        let mut all_imports: Vec<(String, Vec<RawImport>)> = Vec::new();
        let mut all_calls: Vec<RawCall> = Vec::new();

        let mut stats = IndexStats {
            total_files: 0,
            new_files: 0,
            changed_files: 0,
            removed_files: 0,
            symbols_extracted: 0,
        };

        // Keep unchanged files
        for f in &old_files {
            if !changed_set.contains(f.path.as_str()) {
                new_files.push(f.clone());
                let file_syms: Vec<_> = old_symbols
                    .iter()
                    .filter(|s| s.file_id == f.id)
                    .cloned()
                    .collect();
                new_symbols.extend(file_syms);
            }
        }

        // Re-index changed files
        for rel_path in changed_paths {
            let abs_path = self.root.join(rel_path);
            if !abs_path.exists() {
                // File was deleted
                new_fingerprints.remove(rel_path);
                stats.removed_files += 1;
                continue;
            }

            if old_fingerprints.contains_key(rel_path) {
                stats.changed_files += 1;
            } else {
                stats.new_files += 1;
            }

            match self.index_file(&abs_path) {
                Ok((entry, symbols, imports, calls)) => {
                    new_fingerprints.insert(entry.path.clone(), entry.hash.clone());
                    stats.symbols_extracted += symbols.len();
                    if !imports.is_empty() {
                        all_imports.push((entry.path.clone(), imports));
                    }
                    all_calls.extend(calls);
                    new_symbols.extend(symbols);
                    new_files.push(entry);
                }
                Err(e) => {
                    warn!("failed to index {}: {}", abs_path.display(), e);
                }
            }
        }

        stats.total_files = new_files.len();

        // Merge: keep old relations for unchanged files, add new ones for changed files
        let changed_file_ids: std::collections::HashSet<&str> = new_files
            .iter()
            .filter(|f| changed_set.contains(f.path.as_str()))
            .map(|f| f.id.as_str())
            .collect();
        let mut relations: Vec<Relation> = old_relations
            .into_iter()
            .filter(|r| !changed_file_ids.contains(r.source_id.as_str()))
            .collect();
        let new_relations = Self::build_relations(&new_files, &all_imports, &new_symbols, &all_calls);
        relations.extend(new_relations);

        self.store.save_files(&new_files)?;
        self.store.save_symbols(&new_symbols)?;
        self.store.save_relations(&relations)?;
        self.store.save_fingerprints(&new_fingerprints)?;

        info!(
            "changed-only index: {} changed, {} new, {} removed",
            stats.changed_files, stats.new_files, stats.removed_files
        );
        Ok(stats)
    }

    fn index_file(&mut self, path: &Path) -> Result<(FileEntry, Vec<Symbol>, Vec<RawImport>, Vec<RawCall>)> {
        let content = std::fs::read(path)?;
        self.index_file_with_content(path, content)
    }

    fn index_file_with_content(&mut self, path: &Path, content: Vec<u8>) -> Result<(FileEntry, Vec<Symbol>, Vec<RawImport>, Vec<RawCall>)> {
        let hash = blake3::hash(&content).to_hex().to_string();

        let rel_path = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        let language = Language::from_extension(&ext);
        let size = content.len() as u64;
        let file_id = format!("f:{}", &hash[..12]);

        let entry = FileEntry {
            id: file_id.clone(),
            path: rel_path.clone(),
            extension: if ext.is_empty() { None } else { Some(ext) },
            language: Some(language.to_string()),
            size,
            hash,
            indexed_at: Utc::now(),
            tags: Vec::new(),
        };

        // Extract symbols, imports, and calls if it's a code file
        let (symbols, imports, calls) = if language.is_code() {
            let content_str = String::from_utf8_lossy(&content);
            match self.parser.parse(&content_str, language, &file_id, &rel_path) {
                Ok(result) => {
                    debug!(
                        "extracted {} symbols, {} imports, {} calls from {}",
                        result.symbols.len(),
                        result.imports.len(),
                        result.calls.len(),
                        rel_path
                    );
                    (result.symbols, result.imports, result.calls)
                }
                Err(e) => {
                    debug!("parsing failed for {}: {}", rel_path, e);
                    (Vec::new(), Vec::new(), Vec::new())
                }
            }
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };

        Ok((entry, symbols, imports, calls))
    }

    /// Resolve collected imports and calls into Relations.
    fn build_relations(
        files: &[FileEntry],
        file_imports: &[(String, Vec<RawImport>)],
        symbols: &[Symbol],
        calls: &[RawCall],
    ) -> Vec<Relation> {
        let mut relations = Vec::new();

        // Build lookup maps
        let path_to_id: HashMap<&str, &str> = files
            .iter()
            .map(|f| (f.path.as_str(), f.id.as_str()))
            .collect();

        // Stem lookup: "foo" → ["src/foo.rs", "src/foo/mod.rs", ...]
        let mut stem_to_paths: HashMap<String, Vec<&str>> = HashMap::new();
        // Suffix lookup: "foo/bar.rs" → ["src/foo/bar.rs", "lib/foo/bar.rs", ...]
        let mut suffix_to_paths: HashMap<&str, Vec<&str>> = HashMap::new();
        for f in files {
            let p = Path::new(&f.path);
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                stem_to_paths
                    .entry(stem.to_string())
                    .or_default()
                    .push(&f.path);
            }
            // Index all suffixes starting after each '/'
            let path_str = f.path.as_str();
            let mut pos = 0;
            while let Some(slash) = path_str[pos..].find('/') {
                let suffix = &path_str[pos + slash + 1..];
                suffix_to_paths.entry(suffix).or_default().push(path_str);
                pos += slash + 1;
            }
            // Also index the full path itself
            suffix_to_paths.entry(path_str).or_default().push(path_str);
        }

        for (source_path, imports) in file_imports {
            let source_id = match path_to_id.get(source_path.as_str()) {
                Some(id) => *id,
                None => continue,
            };
            let source_dir = Path::new(source_path)
                .parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy();

            for imp in imports {
                let resolved = resolve_import(&imp.path, &source_dir, &path_to_id, &stem_to_paths, &suffix_to_paths);
                for target_path in resolved {
                    if let Some(&target_id) = path_to_id.get(target_path) {
                        if target_id != source_id {
                            relations.push(Relation {
                                source_id: source_id.to_string(),
                                target_id: target_id.to_string(),
                                kind: RelationKind::Imports,
                                weight: 1.0,
                            });
                        }
                    }
                }
            }
        }

        // Add test/config relations
        Self::build_test_relations(&files, &mut relations);
        Self::build_config_relations(&files, &mut relations);

        // Build Calls relations from extracted call data
        Self::build_call_relations(symbols, calls, &mut relations);

        // Deduplicate
        relations.sort_by(|a, b| {
            (&a.source_id, &a.target_id, &a.kind)
                .cmp(&(&b.source_id, &b.target_id, &b.kind))
        });
        relations.dedup_by(|a, b| {
            a.source_id == b.source_id
                && a.target_id == b.target_id
                && a.kind == b.kind
        });

        relations
    }

    /// Resolve extracted calls into Calls relations by matching callee names to known symbols.
    fn build_call_relations(symbols: &[Symbol], calls: &[RawCall], relations: &mut Vec<Relation>) {
        // Build name → symbol info lookup (only functions/methods/classes)
        struct SymInfo<'a> {
            id: &'a str,
            file_id: &'a str,
            path: &'a str,
        }
        let mut name_to_syms: HashMap<&str, Vec<SymInfo>> = HashMap::new();
        for sym in symbols {
            if matches!(
                sym.kind,
                kungfu_types::symbol::SymbolKind::Function
                    | kungfu_types::symbol::SymbolKind::Method
                    | kungfu_types::symbol::SymbolKind::Class
                    | kungfu_types::symbol::SymbolKind::Struct
            ) {
                name_to_syms.entry(sym.name.as_str()).or_default().push(SymInfo {
                    id: &sym.id,
                    file_id: &sym.file_id,
                    path: &sym.path,
                });
            }
        }

        // Build caller_id → file_id/path lookup
        let caller_info: HashMap<&str, (&str, &str)> = symbols.iter()
            .map(|s| (s.id.as_str(), (s.file_id.as_str(), s.path.as_str())))
            .collect();

        // Resolve each call with scoping: same-file > same-dir > cross-project
        for call in calls {
            if let Some(targets) = name_to_syms.get(call.callee_name.as_str()) {
                let (caller_file_id, caller_path) = caller_info.get(call.caller_id.as_str())
                    .copied()
                    .unwrap_or(("", ""));
                let caller_dir = caller_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

                // Partition targets by proximity
                let mut same_file: Vec<&SymInfo> = Vec::new();
                let mut same_dir: Vec<&SymInfo> = Vec::new();
                let mut other: Vec<&SymInfo> = Vec::new();

                for target in targets {
                    if target.id == call.caller_id {
                        continue; // skip self-calls
                    }
                    if target.file_id == caller_file_id {
                        same_file.push(target);
                    } else {
                        let target_dir = target.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                        if target_dir == caller_dir && !caller_dir.is_empty() {
                            same_dir.push(target);
                        } else {
                            other.push(target);
                        }
                    }
                }

                // Use most specific match: same_file > same_dir > other
                // Only fall through if no matches at higher specificity
                let (chosen, weight) = if !same_file.is_empty() {
                    (same_file, 1.0)
                } else if !same_dir.is_empty() {
                    (same_dir, 0.8)
                } else if other.len() <= 2 {
                    // Only resolve cross-dir calls if target name is unique enough
                    (other, 0.5)
                } else {
                    // Too many candidates — ambiguous, skip
                    continue;
                };

                for target in chosen {
                    relations.push(Relation {
                        source_id: call.caller_id.clone(),
                        target_id: target.id.to_string(),
                        kind: RelationKind::Calls,
                        weight: weight as f32,
                    });
                }
            }
        }
    }

    /// Detect test files and create TestFor relations to their source files.
    fn build_test_relations(files: &[FileEntry], relations: &mut Vec<Relation>) {
        // Build stem→file lookup (only non-test source files)
        let mut source_by_stem: HashMap<String, Vec<&FileEntry>> = HashMap::new();
        for f in files {
            if !is_test_file(&f.path) {
                let stem = extract_stem(&f.path);
                if !stem.is_empty() {
                    source_by_stem.entry(stem).or_default().push(f);
                }
            }
        }

        for f in files {
            if !is_test_file(&f.path) {
                continue;
            }
            // Extract base stem: foo_test.rs → foo, foo.spec.ts → foo, foo.test.js → foo
            let stem = extract_test_base_stem(&f.path);
            if stem.is_empty() {
                continue;
            }

            // Find matching source files
            if let Some(sources) = source_by_stem.get(&stem) {
                for source in sources {
                    // Prefer same directory or parent
                    relations.push(Relation {
                        source_id: f.id.clone(),
                        target_id: source.id.clone(),
                        kind: RelationKind::TestFor,
                        weight: 1.0,
                    });
                }
            }
        }
    }

    /// Detect config files and create ConfigFor relations to nearby source files.
    fn build_config_relations(files: &[FileEntry], relations: &mut Vec<Relation>) {
        let config_files: Vec<&FileEntry> = files
            .iter()
            .filter(|f| is_config_file(&f.path))
            .collect();

        if config_files.is_empty() {
            return;
        }

        // For each config file, link to source files in the same or parent directory
        for config in &config_files {
            let config_dir = Path::new(&config.path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            for f in files {
                if f.id == config.id || is_config_file(&f.path) {
                    continue;
                }
                let f_dir = Path::new(&f.path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Same directory or config is in parent
                if f_dir == config_dir || f_dir.starts_with(&format!("{}/", config_dir)) {
                    // Only link to code files in same/child dir
                    let ext = Path::new(&f.path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    if Language::from_extension(ext).is_code() {
                        relations.push(Relation {
                            source_id: config.id.clone(),
                            target_id: f.id.clone(),
                            kind: RelationKind::ConfigFor,
                            weight: 0.5,
                        });
                    }
                }
            }
        }
    }
}

/// Try to resolve an import path to actual file paths in the index.
fn resolve_import<'a>(
    import_path: &str,
    source_dir: &str,
    path_to_id: &HashMap<&'a str, &str>,
    stem_to_paths: &HashMap<String, Vec<&'a str>>,
    suffix_to_paths: &HashMap<&'a str, Vec<&'a str>>,
) -> Vec<&'a str> {
    let mut results = Vec::new();

    // 1. Relative imports: ./foo, ../foo
    if import_path.starts_with('.') {
        let base = if source_dir.is_empty() {
            import_path.to_string()
        } else {
            format!("{}/{}", source_dir, import_path)
        };
        // Normalize path (remove ./ and resolve ../)
        let normalized = normalize_path(&base);

        // Try common extensions — use direct HashMap lookup instead of linear scan
        let candidates = [
            normalized.clone(),
            format!("{}.ts", normalized),
            format!("{}.tsx", normalized),
            format!("{}.js", normalized),
            format!("{}.jsx", normalized),
            format!("{}.py", normalized),
            format!("{}/index.ts", normalized),
            format!("{}/index.js", normalized),
            format!("{}/__init__.py", normalized),
        ];
        for candidate in &candidates {
            if let Some((&path, _)) = path_to_id.get_key_value(candidate.as_str()) {
                results.push(path);
            }
        }
        return results;
    }

    // 2. Rust crate-internal: crate::foo::bar, super::foo, self::foo
    if import_path.starts_with("crate")
        || import_path.starts_with("super")
        || import_path.starts_with("self")
    {
        let stripped = import_path
            .trim_start_matches("crate::")
            .trim_start_matches("super::")
            .trim_start_matches("self::");

        // Convert module path to file path: foo::bar → foo/bar
        let module_path = stripped.replace("::", "/");

        // Try: module_path.rs, module_path/mod.rs, module_path/lib.rs
        let candidates = [
            format!("{}.rs", module_path),
            format!("{}/mod.rs", module_path),
            format!("{}/lib.rs", module_path),
        ];

        // Use suffix index for O(1) lookup instead of scanning all paths
        for candidate in &candidates {
            if let Some(paths) = suffix_to_paths.get(candidate.as_str()) {
                results.extend(paths.iter());
            }
        }

        // Also try matching the last segment as a stem
        if results.is_empty() {
            let last_segment = stripped.rsplit("::").next().unwrap_or(stripped);
            if let Some(paths) = stem_to_paths.get(last_segment) {
                results.extend(paths.iter().take(2));
            }
        }

        return results;
    }

    // 3. Python dotted imports: foo.bar.baz → foo/bar/baz.py
    if import_path.contains('.') && !import_path.contains('/') {
        let file_path = import_path.replace('.', "/");
        let candidates = [
            format!("{}.py", file_path),
            format!("{}/__init__.py", file_path),
        ];
        // Use suffix index for O(1) lookup
        for candidate in &candidates {
            if let Some(paths) = suffix_to_paths.get(candidate.as_str()) {
                results.extend(paths.iter());
            }
        }
        if !results.is_empty() {
            return results;
        }
    }

    // 4. Fallback: try matching the last segment as a file stem
    let last = import_path
        .rsplit(|c| c == '/' || c == ':' || c == '.')
        .next()
        .unwrap_or(import_path);

    if !last.is_empty() && last.len() >= 2 {
        if let Some(paths) = stem_to_paths.get(last) {
            // Return at most 2 matches to avoid noise
            results.extend(paths.iter().take(2));
        }
    }

    results
}

fn is_test_file(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let lower = filename.to_lowercase();

    // foo_test.rs, foo_test.go
    lower.contains("_test.")
        // foo.test.ts, foo.test.js
        || lower.contains(".test.")
        // foo.spec.ts, foo.spec.js
        || lower.contains(".spec.")
        // files in tests/ or test/ or __tests__/ directories
        || path.contains("/tests/")
        || path.contains("/test/")
        || path.contains("/__tests__/")
        // test_foo.py
        || lower.starts_with("test_")
}

fn is_config_file(path: &str) -> bool {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let lower = filename.to_lowercase();

    matches!(
        lower.as_str(),
        "cargo.toml"
            | "package.json"
            | "tsconfig.json"
            | "pyproject.toml"
            | "setup.py"
            | "setup.cfg"
            | "go.mod"
            | "go.sum"
            | "makefile"
            | "dockerfile"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | ".env"
            | ".env.example"
    ) || lower.ends_with(".config.js")
        || lower.ends_with(".config.ts")
        || lower.ends_with(".config.mjs")
}

fn extract_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

fn extract_test_base_stem(path: &str) -> String {
    let stem = extract_stem(path);
    // foo_test → foo
    if let Some(base) = stem.strip_suffix("_test") {
        return base.to_string();
    }
    // foo.test → foo, foo.spec → foo (stem already stripped extension once)
    if let Some(base) = stem.strip_suffix(".test") {
        return base.to_string();
    }
    if let Some(base) = stem.strip_suffix(".spec") {
        return base.to_string();
    }
    // test_foo → foo
    if let Some(base) = stem.strip_prefix("test_") {
        return base.to_string();
    }
    // If in tests/ dir, use stem as-is for matching
    if path.contains("/tests/") || path.contains("/test/") || path.contains("/__tests__/") {
        return stem;
    }
    String::new()
}

/// Normalize a file path: resolve `.` and `..` components.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}
