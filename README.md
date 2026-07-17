# Rust Axum Clean Architecture Skeleton

A working user CRUD API organized around Clean Architecture boundaries. PostgreSQL is the source of truth and also provides a durable background-job queue. Redis is an optional, best-effort cache for single-user reads.

## Run locally

```bash
cp .env.example .env
docker compose up -d
cargo run -- db migrate
cargo run -- all
```

`all` runs HTTP and the worker in one process for local or simple deployments. To run them as independently scalable production processes:

```bash
cargo run -- http
cargo run -- worker
```

Runtime commands do not change the database schema. Run `db migrate` as a deployment step before starting HTTP or workers. The API listens on `http://localhost:3000` by default. Redis is not required for jobs; omit `REDIS_URL` if you do not want the optional user cache.

## Commands

| Command | Purpose |
| --- | --- |
| `cargo run -- http` | Start only the REST API |
| `cargo run -- worker` | Start only the PostgreSQL worker |
| `cargo run -- all` | Start HTTP and worker together |
| `cargo run -- all --migrate` | Migrate, then start HTTP and worker together |
| `cargo run -- db migrate` | Apply pending migrations and exit |
| `cargo run -- db info` | Show applied, pending, failed, or checksum-mismatched migrations |
| `cargo run -- db revert --yes` | Revert the latest migration only when a matching down migration exists |

`db undo --yes` is an alias for `db revert --yes`. Existing migrations are forward-only, so the revert command intentionally refuses to undo them. Prefer a new corrective migration for production rollbacks.

Migration commands use `MIGRATION_DATABASE_URL` when configured and otherwise fall back to `DATABASE_URL`. This allows production to give schema privileges only to the migration process.

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
JOB_WORKER_ID=worker-1 cargo run -- worker
JOB_WORKER_ID=worker-2 cargo run -- worker
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

## Observability with SigNoz

Logs are structured JSON on stdout. Within HTTP and job spans they include `trace_id` and `span_id` fields for SigNoz log/trace correlation; configure your SigNoz OpenTelemetry Collector (or its Kubernetes/Docker agent) to collect container stdout as logs. Trace export is opt-in so local development works without a collector:

```bash
# Self-hosted SigNoz OTLP/HTTP
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
OTEL_SERVICE_NAME=base-skeleton-rust

# SigNoz Cloud also needs its ingestion header
OTEL_EXPORTER_OTLP_HEADERS=signoz-ingestion-key=your-ingestion-key
```

The service accepts W3C `traceparent` and `tracestate` headers, exports HTTP spans through OTLP, and stores the resulting trace context with every durable job. The worker restores that context before handling the job, so an HTTP request and its asynchronous work appear in one distributed trace. Job payloads, authorization headers, cookies, and request bodies are never added to spans.

For self-hosted deployments, OTLP/HTTP is normally exposed on port `4318`; use the regional ingest endpoint supplied by SigNoz Cloud for cloud deployments. See the [SigNoz Rust instrumentation guide](https://signoz.io/docs/instrumentation/opentelemetry-rust/) and [self-hosted ingestion overview](https://signoz.io/docs/ingestion/self-hosted/overview/).

### Run with tracing and logging

1. Apply the queue migration, then start the service with the OTLP endpoint configured:

   ```bash
   cargo run -- db migrate
   OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
   OTEL_SERVICE_NAME=base-skeleton-rust \
   cargo run -- all
   ```

2. Send a request. The application writes JSON logs to stdout and exports its trace asynchronously:

   ```bash
   curl -i -X POST http://localhost:3000/api/v1/users \
     -H 'content-type: application/json' \
     -d '{"email":"ada@example.com","display_name":"Ada Lovelace"}'
   ```

3. In SigNoz, filter traces by `service.name = base-skeleton-rust`. Open the HTTP trace to see its nested spans; use `trace_id` and `span_id` from a JSON log to find the related trace.

The standard `RUST_LOG` filter controls log verbosity. For normal production operation, start with `RUST_LOG=base_skeleton_rust=info,tower_http=info`; use `debug` temporarily when diagnosing a request. Never put secrets in `RUST_LOG` or span fields.

### Trace layout

An HTTP request has one root span and nested layer spans:

```text
http.request                         method, path, final status
└─ presentation.http.user.create     HTTP adapter
   └─ application.user.create        use case
      ├─ infrastructure.postgres.user.create_with_job
      └─ infrastructure.redis.user_cache.set
```

Read operations use the same structure, with `infrastructure.redis.user_cache.get` and/or a PostgreSQL repository span. Health readiness requests include a PostgreSQL readiness span. The root HTTP span is closed after Axum produces the response and records `http.response.status_code`.

Creating a user also saves W3C trace context with `user.created`. When the worker consumes that row, it creates a `job.process` consumer span under the originating trace. The producer request and asynchronous job therefore remain correlated even when different processes run the API and worker.

### Propagation, collection, and safety

Clients or upstream services should forward W3C `traceparent` and optional `tracestate` headers. This service extracts them for incoming requests and uses them as the parent trace context. Configure the SigNoz Collector to read the service/container stdout as JSON logs; SigNoz can then correlate the `trace_id` and `span_id` fields with OTLP traces.

Only safe operational fields are recorded: method, path, status, user ID, page values, job ID/type, cache TTL, and database/cache system. The application intentionally excludes request bodies, emails, display names, job payloads, authorization headers, cookies, SQL parameters, and connection strings.

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
