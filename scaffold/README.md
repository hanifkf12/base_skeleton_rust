# skeleton-new

Generates a new service from the skeleton. On every run it fetches the skeleton from git,
so a new project always starts from the latest **committed and pushed** version; there is
no template baked into the binary and nothing to reinstall when the skeleton changes.

This crate lives in the skeleton repository but outside its Cargo package (`[workspace]` in
its own manifest). Its `scaffold/` directory is never copied into generated projects.

## Install

```bash
cargo install --path scaffold --locked
```

Needs `git` and `cargo` (with `rustfmt`) on `PATH` when it runs. Reinstall only when this
tool's own code changes.

## Use

```bash
skeleton-new orders-api                       # creates ./orders-api
skeleton-new orders-api --path ~/work         # creates ~/work/orders-api
skeleton-new orders-api --branch v2           # a branch or tag instead of the default branch
skeleton-new orders-api --repo git@github.com:me/skeleton.git
skeleton-new help                             # usage and examples (same as --help); also shown with no arguments
```

| Option | Env | Default |
| --- | --- | --- |
| `--repo <url or path>` | `SKELETON_REPO` | `https://github.com/hanifkf12/base_skeleton_rust.git` |
| `--branch <name>` | `SKELETON_BRANCH` | the repository's default branch |
| `--path <dir>` | | `.` |

The clone uses your normal git configuration, so SSH keys and credential helpers work for a
private repository. The name may contain lowercase letters, digits, `-` and `_`, must start
with a letter, and must not be a Rust keyword or a name such as `std`/`core`/`test`.

## What it does

1. Checks the target first: it must not exist or must be empty, and nothing is overwritten.
2. Makes a shallow clone in a temporary directory (removed afterwards, also on errors).
3. Copies the tracked files, keeping script permissions. Because only committed files are
   cloned, `.env`, `target/`, `.zed/` and other ignored local state cannot leak. `scaffold/`
   is skipped.
4. Rewrites the skeleton's names in one pass: `base_skeleton_rust` becomes the snake-case
   name (crate, binary, `RUST_LOG`), `base-skeleton-rust` the kebab-case name (Docker image,
   `OTEL_SERVICE_NAME`), and `base_skeleton` / `base-skeleton` the snake-case / kebab-case name
   (database names, OIDC audience, Keycloak client ids).
5. Moves the renamed package to where Cargo sorts it in `Cargo.lock`, so
   `cargo build --locked` keeps working with the same dependency versions.
6. Runs `cargo fmt`, because a different name length changes line wrapping and import order.

It prints the fetched commit (`<repo>@<short sha>`). It does not run `git init`, create a
`.env`, or start any service; those are the developer's first steps.

## After generating

The generated project's `README.md` has a **First-time setup** section that walks through
everything the developer does next: install the tools, `git init` and the first commit, review
the derived names, create `.env`, start Docker Compose, apply migrations, run and check the
service, create a Keycloak user and call the API, run the checks, and replace the demo parts.
In short:

```bash
cd orders-api
git init -b main && git add -A && git commit -m "chore: initial commit from skeleton"
cp .env.example .env
docker compose up -d
cargo run -- db migrate
cargo run -- all
```

## Develop

```bash
cd scaffold
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The tests build a small local git repository shaped like the skeleton and clone it over
`file://`, so they need `git` but no network.
