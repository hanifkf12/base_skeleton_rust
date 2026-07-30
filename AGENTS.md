# Repository Guidelines

## Project Structure & Module Organization

This is a Rust/Axum clean-architecture service. Source code lives in `src/` and is organized by responsibility:

- `domain/`: entities, value objects, and domain errors; keep it independent of frameworks.
- `application/`: use cases, DTOs, ports, and background-job contracts.
- `infrastructure/`: PostgreSQL repositories/job queue, Redis cache, and job handlers.
- `presentation/http/`: Axum routes, handlers, requests, responses, and HTTP error mapping.
- `bootstrap/`, `config/`, and `telemetry/`: runtime wiring, configuration, and OpenTelemetry setup.

SQL migrations are in `migrations/`; integration tests are in `tests/`. Consult `ARCHITECTURE.md` before adding a feature or domain.

## Build, Test, and Development Commands

Prefix shell commands with `rtk`.

- `rtk cargo run -- http` starts the HTTP server.
- `rtk cargo run -- worker` starts the PostgreSQL-backed job worker.
- `rtk cargo run -- all --migrate` runs migrations, HTTP, and worker together.
- `rtk cargo run -- db migrate` applies pending migrations; `rtk cargo run -- db revert` reverts the latest.
- `rtk cargo fmt --check` verifies formatting.
- `rtk cargo clippy --all-targets --all-features -- -D warnings` runs lints as errors.
- `rtk cargo test` runs unit and integration tests. Use `docker compose up -d` for PostgreSQL or Redis.

## Coding Style & Naming Conventions

Use Rust 2024 idioms and `rustfmt`; use four-space indentation. Name modules and functions in `snake_case`, types and traits in `PascalCase`, and constants in `SCREAMING_SNAKE_CASE`. Keep HTTP types at the presentation boundary and persistence details in infrastructure. Define dependencies as application ports, then implement adapters. Add structured `tracing` spans to meaningful operations; never log secrets or credentials.

## Testing Guidelines

Write unit tests for domain rules and use cases, and integration tests for HTTP and PostgreSQL queue behavior. Name tests after observable behavior, such as `create_user_rejects_duplicate_email`. Run formatting, Clippy, and the full suite before submitting. Add an integration test when changing durable database behavior.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style messages, such as `feat: add PostgreSQL background job queue` or `fix: configure SigNoz OTLP trace export`. Keep commits scoped and include feature migrations. Pull requests should explain behavior, configuration or migration steps, test results, and linked issues. Include examples for API changes and note observability impact.

## Configuration & Operations

Start from `.env.example`; do not commit real credentials. `DATABASE_URL` configures the service and worker; `MIGRATION_DATABASE_URL` may override it for migrations. HTTP and `all` also require `OIDC_ISSUER_URL` and `OIDC_AUDIENCE`; `OIDC_ALLOWED_ALGORITHMS`, `OIDC_ALLOW_INSECURE_HTTP`, `OIDC_HTTP_TIMEOUT_SECONDS`, `OIDC_CLOCK_SKEW_SECONDS`, and `OIDC_JWKS_REFRESH_INTERVAL_SECONDS` tune token verification. Keep `OIDC_ALLOW_INSECURE_HTTP=false` outside local development. Set `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_SERVICE_NAME` to export logs and traces to SigNoz.

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

When the user types `/graphify`, use the installed graphify skill or instructions before doing anything else.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- Dirty graphify-out/ files are expected after hooks or incremental updates; dirty graph files are not a reason to skip graphify. Only skip graphify if the task is about stale or incorrect graph output, or the user explicitly says not to use it.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).
