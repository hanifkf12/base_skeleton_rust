# Rust Axum Clean Architecture Skeleton

A working user CRUD API organized around Clean Architecture boundaries. PostgreSQL is the source of truth and also provides a durable background-job queue. Redis is an optional, best-effort cache for single-user reads.

## Run locally

```bash
cp .env.example .env
docker compose up -d
cargo run
```

In a second terminal, start the job worker:

```bash
cargo run --bin worker
```

Both processes run migrations during startup. The API listens on `http://localhost:3000` by default. Redis is not required for jobs; omit `REDIS_URL` if you do not want the optional user cache.

## PostgreSQL job queue

Creating a user writes both the user and a `user.created` job in one PostgreSQL transaction. The worker claims ready jobs with `FOR UPDATE SKIP LOCKED`, so multiple worker processes can safely consume the same queue.

The queue provides:

- At-least-once delivery.
- Exponential retry delays.
- A configurable attempt limit and `dead` status.
- Worker leases and recovery of jobs abandoned by a crashed worker.
- Ownership checks before a worker can complete or fail a claimed job.
- A handler registry keyed by `job_type`.

The included `user.created` handler validates the payload and logs it to demonstrate the full path. Replace or extend that handler with a real idempotent side effect.

Useful inspection queries:

```sql
SELECT id, job_type, status, attempts, max_attempts, available_at, last_error
FROM background_jobs
ORDER BY created_at DESC;
```

To retry one dead job after fixing its cause:

```sql
UPDATE background_jobs
SET status = 'pending',
    attempts = 0,
    available_at = NOW(),
    last_error = NULL,
    updated_at = NOW()
WHERE id = '<job-uuid>' AND status = 'dead';
```

Run more worker processes to increase throughput:

```bash
JOB_WORKER_ID=worker-1 cargo run --bin worker
JOB_WORKER_ID=worker-2 cargo run --bin worker
```

Keep each worker ID unique. `JOB_LEASE_TIMEOUT_SECONDS` must be longer than the normal maximum runtime of a handler; processing is at-least-once, so every real handler must also be idempotent.

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
- `application` contains use cases, job orchestration, and the external ports they need.
- `presentation` owns Axum DTOs, handlers, routing, and HTTP error mapping.
- `infrastructure` implements the ports with SQLx/PostgreSQL and Redis, including job claiming and handlers.
- `bootstrap` is the composition root and the only module that wires concrete implementations together.

Cache failures do not fail startup or requests; the application falls back to a no-op cache and PostgreSQL remains authoritative. Omit `REDIS_URL` to disable caching intentionally. User creation uses the focused `UserRegistrationRepository` port because it must save the user and its first job atomically. The project still avoids a broad generic unit-of-work abstraction.

See `ARCHITECTURE.md` for the job lifecycle and the step-by-step guide for adding a new job type.

Every response includes an `x-request-id`. Authorization and cookie headers are marked sensitive in traces, request bodies are limited by `MAX_REQUEST_BODY_BYTES`, and requests are bounded by `REQUEST_TIMEOUT_SECONDS`.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Set `TEST_DATABASE_URL` to include the real PostgreSQL queue lifecycle test locally. Without it, that one test returns early; all dependency-free tests still run. CI provisions PostgreSQL and always executes the database path.

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/base_skeleton \
  cargo test --test postgres_job_queue
```

The same checks run automatically through `.github/workflows/ci.yml`.
