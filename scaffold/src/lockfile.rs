use anyhow::{Context, Result, ensure};

const MARKER: &str = "[[package]]\n";

/// Move the (renamed) root package to where Cargo sorts it.
///
/// Cargo writes `[[package]]` entries ordered by name. Renaming the root package
/// in place leaves the file out of order, and `cargo build --locked` rejects a
/// lockfile it would rewrite. Dependency versions and checksums are untouched.
pub fn reposition_root_package(lock: &str, root: &str) -> Result<String> {
    let first = lock
        .find(MARKER)
        .context("Cargo.lock contains no packages")?;
    let (header, packages) = lock.split_at(first);

    let mut blocks: Vec<&str> = {
        let starts: Vec<usize> = packages.match_indices(MARKER).map(|(i, _)| i).collect();
        starts
            .iter()
            .enumerate()
            .map(|(n, &start)| {
                let end = starts.get(n + 1).copied().unwrap_or(packages.len());
                packages[start..end].trim_end()
            })
            .collect()
    };

    let current = blocks
        .iter()
        .position(|block| package_name(block) == Some(root) && !block.contains("\nsource = "))
        .with_context(|| format!("Cargo.lock has no root package named {root}"))?;
    let root_block = blocks.remove(current);
    let target = blocks
        .iter()
        .position(|block| package_name(block).is_some_and(|name| name > root))
        .unwrap_or(blocks.len());
    blocks.insert(target, root_block);

    let mut output = String::with_capacity(lock.len());
    output.push_str(header);
    output.push_str(&blocks.join("\n\n"));
    output.push('\n');
    ensure!(
        output.len() == lock.len(),
        "repositioning the root package must not change the lockfile size"
    );
    Ok(output)
}

fn package_name(block: &str) -> Option<&str> {
    block
        .lines()
        .nth(1)?
        .strip_prefix("name = \"")?
        .strip_suffix('"')
}

#[cfg(test)]
mod tests {
    use super::reposition_root_package;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"alpha\"\nversion = \"1.0.0\"\nsource = \"registry+x\"\n\n[[package]]\nname = \"base\"\nversion = \"0.1.0\"\ndependencies = [\n \"alpha\",\n]\n\n[[package]]\nname = \"zeta\"\nversion = \"2.0.0\"\nsource = \"registry+x\"\n";

    #[test]
    fn keeps_cargo_ordering_after_a_rename() {
        let renamed = LOCK.replace("name = \"base\"", "name = \"mid\"");
        assert_eq!(reposition_root_package(&renamed, "mid").unwrap(), renamed);

        let renamed = LOCK.replace("name = \"base\"", "name = \"yankee\"");
        let sorted = reposition_root_package(&renamed, "yankee").unwrap();
        let names: Vec<_> = sorted
            .lines()
            .filter_map(|line| line.strip_prefix("name = "))
            .collect();
        assert_eq!(names, ["\"alpha\"", "\"yankee\"", "\"zeta\""]);
        assert!(sorted.starts_with("version = 4\n\n[[package]]\nname = \"alpha\""));
        assert!(sorted.ends_with("source = \"registry+x\"\n"));
        assert_eq!(sorted.len(), renamed.len());
    }

    #[test]
    fn fails_when_the_root_package_is_missing() {
        assert!(reposition_root_package(LOCK, "missing").is_err());
    }
}
