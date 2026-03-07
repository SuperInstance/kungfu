use anyhow::{Context, Result};
use kungfu_types::file::FileEntry;
use kungfu_types::symbol::Symbol;
use kungfu_types::relation::Relation;
use kungfu_types::chunk::Chunk;
use std::path::Path;
use tracing::debug;

pub struct JsonStore {
    base_dir: std::path::PathBuf,
}

impl JsonStore {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
        }
    }

    pub fn save_files(&self, files: &[FileEntry]) -> Result<()> {
        let path = self.base_dir.join("files.json");
        let json = serde_json::to_string_pretty(files)?;
        std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
        debug!("saved {} files to index", files.len());
        Ok(())
    }

    pub fn load_files(&self) -> Result<Vec<FileEntry>> {
        let path = self.base_dir.join("files.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        let files: Vec<FileEntry> = serde_json::from_str(&content)?;
        Ok(files)
    }

    pub fn save_symbols(&self, symbols: &[Symbol]) -> Result<()> {
        let path = self.base_dir.join("symbols.json");
        let json = serde_json::to_string_pretty(symbols)?;
        std::fs::write(&path, json)?;
        debug!("saved {} symbols to index", symbols.len());
        Ok(())
    }

    pub fn load_symbols(&self) -> Result<Vec<Symbol>> {
        let path = self.base_dir.join("symbols.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save_relations(&self, relations: &[Relation]) -> Result<()> {
        let path = self.base_dir.join("relations.json");
        let json = serde_json::to_string_pretty(relations)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_relations(&self) -> Result<Vec<Relation>> {
        let path = self.base_dir.join("relations.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save_chunks(&self, chunks: &[Chunk]) -> Result<()> {
        let path = self.base_dir.join("chunks.json");
        let json = serde_json::to_string_pretty(chunks)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_chunks(&self) -> Result<Vec<Chunk>> {
        let path = self.base_dir.join("chunks.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save_fingerprints(&self, fingerprints: &std::collections::HashMap<String, String>) -> Result<()> {
        let path = self.base_dir.join("fingerprints.json");
        let json = serde_json::to_string_pretty(fingerprints)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn load_fingerprints(&self) -> Result<std::collections::HashMap<String, String>> {
        let path = self.base_dir.join("fingerprints.json");
        if !path.exists() {
            return Ok(std::collections::HashMap::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    }
}
