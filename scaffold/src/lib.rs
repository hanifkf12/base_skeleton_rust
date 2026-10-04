//! Generates a new project from the skeleton repository.

mod lockfile;
mod name;
mod source;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use aho_corasick::{AhoCorasick, MatchKind};
use anyhow::{Context, Result, bail, ensure};

pub use name::ProjectName;
pub use source::{Checkout, Source};

/// What the skeleton calls itself; every occurrence is rewritten.
const SKELETON_SNAKE: &str = "base_skeleton";
const SKELETON_KEBAB: &str = "base-skeleton";
const SKELETON_CRATE: &str = "base_skeleton_rust";
const SKELETON_IMAGE: &str = "base-skeleton-rust";

/// This tool lives in the skeleton repository but is not part of a project.
const EXCLUDED_ROOT: &str = "scaffold";

/// Result of a successful generation.
#[derive(Debug)]
pub struct Generated {
    pub directory: PathBuf,
    pub files: usize,
    /// Short commit id of the skeleton the project was generated from.
    pub revision: String,
}

/// Fetch the skeleton from `source` and create `<parent>/<name>` from it.
///
/// The target directory must not exist or must be empty; nothing is overwritten.
/// It is checked before anything is fetched so a bad target fails immediately.
pub fn generate(name: &ProjectName, parent: &Path, source: Source<'_>) -> Result<Generated> {
    let directory = parent.join(name.as_str());
    ensure_empty_target(&directory)?;

    let checkout = Checkout::fetch(source)?;
    let renamer = Renamer::new(name);
    let mut files = 0;
    for relative in checkout.tracked_files()? {
        if relative.split('/').next() == Some(EXCLUDED_ROOT) {
            continue;
        }
        let from = checkout.directory().join(&relative);
        let bytes =
            fs::read(&from).with_context(|| format!("could not read {}", from.display()))?;
        let destination = directory.join(&relative);
        if let Some(folder) = destination.parent() {
            fs::create_dir_all(folder)
                .with_context(|| format!("could not create {}", folder.display()))?;
        }
        fs::write(&destination, render(&relative, bytes, &renamer, name)?)
            .with_context(|| format!("could not write {}", destination.display()))?;
        // Keep the executable bit of scripts.
        let permissions = fs::metadata(&from)?.permissions();
        fs::set_permissions(&destination, permissions)
            .with_context(|| format!("could not set permissions on {}", destination.display()))?;
        files += 1;
    }
    ensure!(
        files > 0,
        "the skeleton repository contains nothing besides `{EXCLUDED_ROOT}/`"
    );

    Ok(Generated {
        directory,
        files,
        revision: checkout.revision().to_owned(),
    })
}

/// Run `cargo fmt` in a generated project.
///
/// Renaming changes identifier lengths and import sort order, so the renamed
/// sources are not rustfmt-clean until formatted; CI's `fmt --check` would fail.
pub fn format_project(directory: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["fmt", "--all"])
        .current_dir(directory)
        .output()
        .context("could not run `cargo fmt`; install it with `rustup component add rustfmt`")?;
    ensure!(
        output.status.success(),
        "`cargo fmt` failed in {}: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn ensure_empty_target(directory: &Path) -> Result<()> {
    match fs::read_dir(directory) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                bail!("{} already exists and is not empty", directory.display());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("could not inspect {}", directory.display()))
        }
    }
}

fn render(
    relative: &str,
    bytes: Vec<u8>,
    renamer: &Renamer,
    name: &ProjectName,
) -> Result<Vec<u8>> {
    // Binary files are copied verbatim.
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(bytes);
    };
    let renamed = renamer.apply(text);
    if relative == "Cargo.lock" {
        return Ok(lockfile::reposition_root_package(&renamed, &name.snake())?.into_bytes());
    }
    Ok(renamed.into_bytes())
}

/// Single-pass rewrite of the skeleton's names. One pass with leftmost-longest
/// matching means `base_skeleton_rust` is never half-rewritten as `base_skeleton`
/// plus a stray `_rust`, and a project name that itself contains a skeleton name
/// is not rewritten twice.
struct Renamer {
    matcher: AhoCorasick,
    replacements: [String; 4],
}

impl Renamer {
    fn new(name: &ProjectName) -> Self {
        let patterns = [
            SKELETON_CRATE,
            SKELETON_IMAGE,
            SKELETON_SNAKE,
            SKELETON_KEBAB,
        ];
        let replacements = [name.snake(), name.kebab(), name.snake(), name.kebab()];
        let matcher = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .expect("static patterns are valid");
        Self {
            matcher,
            replacements,
        }
    }

    fn apply(&self, text: &str) -> String {
        self.matcher.replace_all(text, &self.replacements)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectName, Renamer};

    fn rename(project: &str, text: &str) -> String {
        Renamer::new(&ProjectName::parse(project).unwrap()).apply(text)
    }

    #[test]
    fn rewrites_each_skeleton_spelling_with_the_matching_style() {
        let text = "base_skeleton_rust base-skeleton-rust base_skeleton base_skeleton_test \
                    base-skeleton-api";
        assert_eq!(
            rename("orders-api", text),
            "orders_api orders-api orders_api orders_api_test orders-api-api"
        );
    }

    #[test]
    fn does_not_rewrite_its_own_output() {
        assert_eq!(
            rename("base-skeleton-x", "base_skeleton_rust"),
            "base_skeleton_x"
        );
    }
}
