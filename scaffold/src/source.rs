//! Fetching the skeleton from git.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};

/// Where the skeleton is fetched from: a repository and an optional branch or tag.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub repo: &'a str,
    pub reference: Option<&'a str>,
}

/// A throwaway shallow checkout of the skeleton, removed on drop.
#[derive(Debug)]
pub struct Checkout {
    directory: PathBuf,
    revision: String,
}

impl Checkout {
    pub fn fetch(source: Source<'_>) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let directory =
            std::env::temp_dir().join(format!("skeleton-new-{}-{nanos}", std::process::id()));
        fs::create_dir(&directory)
            .with_context(|| format!("could not create {}", directory.display()))?;
        // From here on `Drop` removes the directory, including on every error path.
        let mut checkout = Self {
            directory,
            revision: String::new(),
        };

        let mut clone = Command::new("git");
        clone.args(["clone", "--quiet", "--depth", "1", "--single-branch"]);
        if let Some(reference) = source.reference {
            // `--branch=<value>` keeps a value that starts with `-` from being an option.
            clone.arg(format!("--branch={reference}"));
        }
        // `--` keeps a repository value that starts with `-` from being an option.
        clone.arg("--").arg(source.repo).arg(&checkout.directory);
        run(&mut clone, "git clone").with_context(|| match source.reference {
            Some(reference) => format!("could not fetch {} at `{reference}`", source.repo),
            None => format!("could not fetch {}", source.repo),
        })?;

        let mut revision = Command::new("git");
        revision
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&checkout.directory);
        checkout.revision = run(&mut revision, "git rev-parse")?.trim().to_owned();
        Ok(checkout)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Short commit id of the fetched revision.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Tracked files relative to the checkout root, `/`-separated and sorted.
    /// Anything the repository's `.gitignore` excluded was never committed, so
    /// local state such as `.env` cannot appear here.
    pub fn tracked_files(&self) -> Result<Vec<String>> {
        let mut list = Command::new("git");
        list.args(["ls-files", "-z"]).current_dir(&self.directory);
        let output = run(&mut list, "git ls-files")?;
        let mut files: Vec<String> = output
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        files.sort();
        ensure!(!files.is_empty(), "the fetched repository has no files");
        Ok(files)
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn run(command: &mut Command, label: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("could not run `{label}`; is git installed and on PATH?"))?;
    ensure!(
        output.status.success(),
        "`{label}` failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
