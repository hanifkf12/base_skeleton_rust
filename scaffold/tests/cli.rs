use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Runs the real binary in an empty working directory and returns what it left behind.
fn run(args: &[&str]) -> (Output, Vec<String>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let directory: PathBuf = std::env::temp_dir().join(format!(
        "skeleton-new-cli-{}-{sequence}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    // A repository that cannot be fetched: any code path that tries to generate fails
    // loudly instead of reaching the network.
    let output = Command::new(env!("CARGO_BIN_EXE_skeleton-new"))
        .args(args)
        .env("SKELETON_REPO", "file:///definitely/not/a/repository")
        .current_dir(&directory)
        .output()
        .unwrap();
    let left: Vec<String> = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fs::remove_dir_all(&directory).unwrap();
    (output, left)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn help_command_prints_usage_and_examples_without_creating_a_project() {
    let (output, left) = run(&["help"]);
    assert!(output.status.success());
    let text = stdout(&output);
    for expected in [
        "Usage: skeleton-new",
        "--branch",
        "--repo",
        "Examples:",
        "skeleton-new help",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(left.is_empty(), "help must not create anything: {left:?}");
}

#[test]
fn help_flags_and_no_arguments_show_the_same_help() {
    let (flag, _) = run(&["--help"]);
    assert!(flag.status.success());
    assert!(stdout(&flag).contains("Examples:"));

    // No arguments shows help instead of a terse "missing argument" error.
    let (bare, left) = run(&[]);
    assert!(
        stdout(&bare).contains("Usage: skeleton-new") || {
            // clap prints `arg_required_else_help` output to stderr with a non-zero status.
            String::from_utf8_lossy(&bare.stderr).contains("Usage: skeleton-new")
        }
    );
    assert!(left.is_empty());
}
