use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub fn is_git_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

pub fn changed_files(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(root)
        .output()
        .context("failed to run git diff")?;

    if !output.status.success() {
        // Try without HEAD for initial commit
        let output = Command::new("git")
            .args(["diff", "--name-only"])
            .current_dir(root)
            .output()
            .context("failed to run git diff")?;

        let text = String::from_utf8_lossy(&output.stdout);
        return Ok(text.lines().map(String::from).filter(|s| !s.is_empty()).collect());
    }

    let text = String::from_utf8_lossy(&output.stdout);

    // Also get untracked files
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .context("failed to run git ls-files")?;

    let untracked_text = String::from_utf8_lossy(&untracked.stdout);

    let mut files: Vec<String> = text
        .lines()
        .chain(untracked_text.lines())
        .map(String::from)
        .filter(|s| !s.is_empty())
        .collect();

    files.sort();
    files.dedup();
    Ok(files)
}

/// Compact git log for a file: last N commits with date, author, message.
pub fn file_log(root: &Path, file_path: &str, max_entries: usize) -> Result<Vec<LogEntry>> {
    let output = Command::new("git")
        .args([
            "log",
            "--follow",
            &format!("-{}", max_entries),
            "--format=%H|%ai|%an|%s",
            "--",
            file_path,
        ])
        .current_dir(root)
        .output()
        .context("failed to run git log")?;

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() == 4 {
                Some(LogEntry {
                    hash: parts[0][..8].to_string(),
                    date: parts[1].to_string(),
                    author: parts[2].to_string(),
                    message: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect())
}

/// Git blame for a line range: who last changed each line.
pub fn blame_lines(
    root: &Path,
    file_path: &str,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<BlameLine>> {
    let output = Command::new("git")
        .args([
            "blame",
            "--porcelain",
            &format!("-L{},{}", start_line, end_line),
            "--",
            file_path,
        ])
        .current_dir(root)
        .output()
        .context("failed to run git blame")?;

    let text = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let mut current_author = String::new();
    let mut current_date = String::new();
    let mut current_hash = String::new();
    let mut current_summary = String::new();

    for line in text.lines() {
        if line.len() >= 40 && line.chars().take(40).all(|c| c.is_ascii_hexdigit()) {
            current_hash = line[..8].to_string();
        } else if let Some(author) = line.strip_prefix("author ") {
            current_author = author.to_string();
        } else if let Some(date) = line.strip_prefix("author-time ") {
            // Unix timestamp — convert to date string
            if let Ok(ts) = date.parse::<i64>() {
                current_date = format_timestamp(ts);
            }
        } else if let Some(summary) = line.strip_prefix("summary ") {
            current_summary = summary.to_string();
        } else if line.starts_with('\t') {
            // Content line — emit blame entry
            results.push(BlameLine {
                hash: current_hash.clone(),
                author: current_author.clone(),
                date: current_date.clone(),
                summary: current_summary.clone(),
            });
        }
    }

    // Deduplicate consecutive identical blame entries
    results.dedup_by(|a, b| a.hash == b.hash);
    Ok(results)
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub hash: String,
    pub date: String,
    pub author: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub summary: String,
}

fn format_timestamp(ts: i64) -> String {
    // Simple UTC date formatting without chrono
    let days = ts / 86400;
    let y = 1970 + (days * 4 + 2) / 1461; // rough year
    format!("{}", y)
}

pub fn staged_files(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(root)
        .output()
        .context("failed to run git diff --cached")?;

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().map(String::from).filter(|s| !s.is_empty()).collect())
}
