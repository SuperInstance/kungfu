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

pub fn staged_files(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(root)
        .output()
        .context("failed to run git diff --cached")?;

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().map(String::from).filter(|s| !s.is_empty()).collect())
}
