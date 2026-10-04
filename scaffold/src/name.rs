use anyhow::{Result, bail, ensure};

const MAX_LENGTH: usize = 64;

/// Identifiers that cannot be crate names: Rust keywords and crates every
/// program already links against.
const RESERVED: &[&str] = &[
    "abstract",
    "alloc",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "core",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "priv",
    "proc_macro",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "std",
    "struct",
    "super",
    "test",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
];

/// A validated project name, usable as a directory, crate, binary, image and
/// database name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectName(String);

impl ProjectName {
    pub fn parse(input: &str) -> Result<Self> {
        ensure!(!input.is_empty(), "the project name must not be empty");
        ensure!(
            input.len() <= MAX_LENGTH,
            "the project name must be at most {MAX_LENGTH} characters"
        );
        let mut characters = input.chars();
        let first = characters.next().expect("checked non-empty");
        ensure!(
            first.is_ascii_lowercase(),
            "the project name must start with a lowercase ASCII letter"
        );
        if let Some(invalid) = characters
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')))
        {
            bail!(
                "the project name may only contain lowercase letters, digits, '-' and '_' (found {invalid:?})"
            );
        }
        let name = Self(input.to_owned());
        ensure!(
            !RESERVED.contains(&name.snake().as_str()),
            "`{input}` is a reserved Rust name and cannot be used as a crate name"
        );
        Ok(name)
    }

    /// The name as given; used for the project directory.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Crate, binary and database form: `my_service`.
    pub fn snake(&self) -> String {
        self.0.replace('-', "_")
    }

    /// Image, service and client-id form: `my-service`.
    pub fn kebab(&self) -> String {
        self.0.replace('_', "-")
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectName;

    #[test]
    fn accepts_kebab_snake_and_digits() {
        for valid in ["my-app", "orders_api", "a1", "billing-v2"] {
            assert!(ProjectName::parse(valid).is_ok(), "{valid}");
        }
    }

    #[test]
    fn rejects_names_that_are_not_valid_crate_names() {
        let too_long = "a".repeat(65);
        for invalid in [
            "",
            "My-App",
            "1app",
            "-app",
            "_app",
            "my app",
            "../x",
            "a/b",
            "app.rs",
            "fn",
            "std",
            "test",
            "proc-macro",
            &too_long,
        ] {
            assert!(ProjectName::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn converts_between_separator_styles() {
        let name = ProjectName::parse("my-cool_app").unwrap();
        assert_eq!(name.snake(), "my_cool_app");
        assert_eq!(name.kebab(), "my-cool-app");
        assert_eq!(name.as_str(), "my-cool_app");
    }
}
