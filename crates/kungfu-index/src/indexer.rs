use anyhow::Result;
use chrono::Utc;
use kungfu_config::KungfuConfig;
use kungfu_parse::Parser;
use kungfu_storage::JsonStore;
use kungfu_types::file::{FileEntry, Language};
use kungfu_types::symbol::Symbol;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::scanner;

pub struct Indexer {
    root: std::path::PathBuf,
    config: KungfuConfig,
    store: JsonStore,
    parser: Parser,
}

pub struct IndexStats {
    pub total_files: usize,
    pub new_files: usize,
    pub changed_files: usize,
    pub removed_files: usize,
    pub symbols_extracted: usize,
}

impl Indexer {
    pub fn new(root: &Path, config: KungfuConfig, store: JsonStore) -> Self {
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

        for path in &paths {
            match self.index_file(path) {
                Ok((entry, symbols)) => {
                    fingerprints.insert(entry.path.clone(), entry.hash.clone());
                    all_symbols.extend(symbols);
                    files.push(entry);
                }
                Err(e) => {
                    warn!("failed to index {}: {}", path.display(), e);
                }
            }
        }

        let stats = IndexStats {
            total_files: files.len(),
            new_files: files.len(),
            changed_files: 0,
            removed_files: 0,
            symbols_extracted: all_symbols.len(),
        };

        self.store.save_files(&files)?;
        self.store.save_symbols(&all_symbols)?;
        self.store.save_fingerprints(&fingerprints)?;

        info!(
            "indexed {} files, {} symbols",
            stats.total_files, stats.symbols_extracted
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

            match self.index_file(path) {
                Ok((entry, symbols)) => {
                    new_fingerprints.insert(entry.path.clone(), entry.hash.clone());
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

        self.store.save_files(&new_files)?;
        self.store.save_symbols(&new_symbols)?;
        self.store.save_fingerprints(&new_fingerprints)?;

        info!(
            "incremental index: {} total, {} new, {} changed, {} removed, {} symbols",
            stats.total_files,
            stats.new_files,
            stats.changed_files,
            stats.removed_files,
            stats.symbols_extracted
        );
        Ok(stats)
    }

    fn index_file(&mut self, path: &Path) -> Result<(FileEntry, Vec<Symbol>)> {
        let content = std::fs::read(path)?;
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

        // Extract symbols if it's a code file
        let symbols = if language.is_code() {
            let content_str = String::from_utf8_lossy(&content);
            match self.parser.extract_symbols(&content_str, language, &file_id, &rel_path) {
                Ok(syms) => {
                    debug!("extracted {} symbols from {}", syms.len(), rel_path);
                    syms
                }
                Err(e) => {
                    debug!("symbol extraction failed for {}: {}", rel_path, e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Ok((entry, symbols))
    }
}
