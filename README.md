# Rust Axum Clean Architecture Skeleton

A working user CRUD API organized around Clean Architecture boundaries. PostgreSQL is the source of truth and Redis is an optional, best-effort cache for single-user reads.

## Run locally

```bash
cp .env.example .env
docker compose up -d
cargo run
```

The application runs migrations during startup and listens on `http://localhost:3000` by default.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Liveness probe |
| `GET` | `/health/live` | Explicit liveness probe |
| `GET` | `/health/ready` | PostgreSQL readiness probe |
| `POST` | `/api/v1/users` | Create a user |
| `GET` | `/api/v1/users?page=1&per_page=20` | List users |
| `GET` | `/api/v1/users/{id}` | Get a user |
| `PUT` | `/api/v1/users/{id}` | Update a user |
| `DELETE` | `/api/v1/users/{id}` | Delete a user |

Create and update bodies use this shape:

```json
{
  "email": "ada@example.com",
  "display_name": "Ada Lovelace"
}
```

## Dependency boundaries

- `domain` contains user rules and value objects.
- `application` contains one concrete use case per action and the external ports it needs.
- `presentation` owns Axum DTOs, handlers, routing, and HTTP error mapping.
- `infrastructure` implements the ports with SQLx/PostgreSQL and Redis.
- `bootstrap` is the composition root and the only module that wires concrete implementations together.

Cache failures do not fail startup or requests; the application falls back to a no-op cache and PostgreSQL remains authoritative. Omit `REDIS_URL` to disable caching intentionally. A generic transaction abstraction is intentionally absent because every current operation is a single atomic statement.

Every response includes an `x-request-id`. Authorization and cookie headers are marked sensitive in traces, request bodies are limited by `MAX_REQUEST_BODY_BYTES`, and requests are bounded by `REQUEST_TIMEOUT_SECONDS`.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The same checks run automatically through `.github/workflows/ci.yml`.
