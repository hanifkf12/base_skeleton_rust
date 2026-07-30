# Rust Axum Clean Architecture Skeleton

A working user CRUD API organized around Clean Architecture boundaries. PostgreSQL is the source of truth and also provides a durable background-job queue. Redis is an optional, best-effort cache for single-user reads.

For the full system-level view—including Mermaid diagrams, runtime flows, security boundaries, data consistency, failure modes, deployment topology, and extension guidance—see [Complete System Architecture](docs/system-architecture.md).

## Run locally

```bash
cp .env.example .env
docker compose up -d
cargo run -- db migrate
cargo run -- all
```

Before running `http` or `all`, replace the example OIDC values in `.env` by following [Set up authorization](#set-up-authorization). The values in `.env.example` are placeholders and cannot authenticate requests.

`all` runs HTTP and the worker in one process for local or simple deployments. To run them as independently scalable production processes:

```bash
cargo run -- http
cargo run -- worker
```

Runtime commands do not change the database schema. Run `db migrate` as a deployment step before starting HTTP or workers. The API listens on `http://localhost:3000` by default. Redis is not required for jobs; omit `REDIS_URL` if you do not want the optional user cache.

Build one production image and use it for both the privileged migration step and the unprivileged runtime:

```bash
docker build -t base-skeleton-rust .
docker run --rm --env-file .env base-skeleton-rust db migrate
docker run --rm --env-file .env -p 3000:3000 base-skeleton-rust http
```

The Debian slim runtime includes CA certificates, runs as a non-root user, defaults to `http`, and health-checks `/health/live`. Override the command with `worker`, `all`, or `db migrate`.

The HTTP server is an OIDC resource server. Before starting `http` or `all`, set `OIDC_ISSUER_URL` to an issuer with discovery/JWKS support and set `OIDC_AUDIENCE` to this API's dedicated audience. These variables are intentionally not read by `worker` or `db` commands.

## Commands

The repository includes a `Makefile` for common workflows. Run `make help` to list targets; for example, `make deps-up`, `make db-migrate`, `make all`, and `make check`. The underlying Cargo and Docker commands remain available directly.

| Command | Purpose |
| --- | --- |
| `cargo run -- http` | Start only the REST API |
| `cargo run -- worker` | Start only the PostgreSQL worker |
| `cargo run -- all` | Start HTTP and worker together |
| `cargo run -- all --migrate` | Migrate, then start HTTP and worker together |
| `cargo run -- migration:create <name>` | Create a timestamped forward-only SQL migration file |
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
- Periodic cleanup independent of job success. Completed and dead retention are separately configurable; each pass deletes at most 1,000 rows.

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
| `GET` | `/metrics` | Bearer-protected Prometheus metrics; absent unless configured |
| `POST` | `/api/v1/users` | Create a user |
| `GET` | `/api/v1/users?page=1&per_page=20` | List users |
| `GET` | `/api/v1/users/{id}` | Get a user |
| `PUT` | `/api/v1/users/{id}` | Update a user |
| `DELETE` | `/api/v1/users/{id}` | Delete a user |

Health endpoints are public. Every users endpoint requires a Bearer JWT access token issued by `OIDC_ISSUER_URL` for `OIDC_AUDIENCE`: `GET` and `HEAD` require `users:read`, while `POST`, `PUT`, and `DELETE` require `users:write`.

All `/api/*` routes also use a per-client-IP token bucket, defaulting to 120 requests/minute with burst 30. Health and metrics are excluded. Rejections use the normal JSON error envelope, status `429`, and `Retry-After`.

The TCP peer is authoritative by default. `X-Forwarded-For` is considered only when the immediate peer belongs to `TRUSTED_PROXY_CIDRS`; malformed headers fall back to the peer. Configure only proxy networks you operate, and ensure the proxy overwrites client-supplied forwarding headers.

## OIDC access tokens

This service validates externally issued JWT access tokens; it does not store passwords, redirect browsers, issue tokens, or refresh tokens. Configure:

- `OIDC_ISSUER_URL` (required for `http` and `all`): exact expected issuer and discovery base URL. Use HTTPS in production.
- `OIDC_AUDIENCE` (required for `http` and `all`): dedicated API audience.
- `OIDC_ALLOWED_ALGORITHMS` (optional, default `RS256`): comma-delimited asymmetric signing algorithms.
- `OIDC_ALLOW_INSECURE_HTTP` (optional, default `false`): allows HTTP issuer and JWKS URLs for local development only.
- `OIDC_HTTP_TIMEOUT_SECONDS` (optional, default `5`): discovery and JWKS request timeout.
- `OIDC_CLOCK_SKEW_SECONDS` (optional, default `30`): allowed token timestamp skew.
- `OIDC_JWKS_REFRESH_INTERVAL_SECONDS` (optional, default `60`): minimum interval between unknown-key JWKS refreshes.
- `OIDC_JWKS_MAX_AGE_SECONDS` (optional, default `300`): maximum key-cache age before refresh is mandatory.
- `OIDC_MAX_TOKEN_LIFETIME_SECONDS` (optional, default `3600`): maximum interval between required `iat` and `exp` claims.

For a complete local Keycloak and Postman walkthrough, see [Keycloak Setup Guide](docs/keycloak-setup.md).

At HTTP startup, the service loads discovery metadata and the initial JWKS. Startup fails if either is unavailable or invalid. Unless insecure HTTP is explicitly allowed, the configured issuer, discovered issuer, and JWKS URI must use HTTPS. Signing keys are cached; an unknown `kid` triggers at most one refresh per configured interval. Once the cache reaches `OIDC_JWKS_MAX_AGE_SECONDS`, refresh is mandatory even for an unchanged `kid`; a failure returns `503 authentication_unavailable` and stale key material is not used. The refresh interval throttles retries to protect the provider.

Tokens must have a signature from a discovered signing key, an allowed algorithm, matching `iss` and `aud`, valid `exp`, required `iat`, and optional `nbf` timestamps, a non-empty `sub`, and a standard space-delimited `scope` claim. Future `iat` outside clock skew and excessive token lifetime are rejected. Authentication failures use the existing JSON error envelope plus a `WWW-Authenticate: Bearer` challenge.

### Set up authorization

This API is an OAuth 2.0/OIDC resource server. Use an identity provider such as Keycloak, Auth0, Microsoft Entra ID, Okta, or another provider that issues signed JWT access tokens and publishes OIDC discovery/JWKS metadata.

1. Create an API resource in the provider with a dedicated audience, for example `base-skeleton-api`.
2. Create the OAuth scopes `users:read` and `users:write`. Assign them to the users, groups, roles, or service account that should administer this API. If the provider uses roles internally, configure its token claim mapping so the issued access token has a space-delimited `scope` claim containing these values.
3. Configure the provider to issue JWT **access tokens** for this audience, signed with an asymmetric algorithm such as `RS256`, and expose a JWKS through OIDC discovery. Do not use opaque access tokens or ID tokens for this API.
4. Create a client to obtain tokens. For server-to-server use, create a confidential client with the client-credentials grant and assign the required scopes. For user-driven applications, use your provider's normal authorization-code-with-PKCE flow; this API only receives the resulting access token.
5. Find the issuer URL in the provider's discovery document. The configured value must exactly match the document's `issuer` field. For an issuer such as `https://id.example.com/realms/demo`, its discovery URL is normally:

   ```text
   https://id.example.com/realms/demo/.well-known/openid-configuration
   ```

6. Set the matching values in `.env`:

   ```dotenv
   OIDC_ISSUER_URL=https://id.example.com/realms/demo
   OIDC_AUDIENCE=base-skeleton-api
   OIDC_ALLOWED_ALGORITHMS=RS256
   ```

   If the provider signs tokens with several asymmetric algorithms, list them comma-separated, for example `RS256,ES256`. HMAC algorithms are intentionally not supported.

7. Start the dependencies and API:

   ```bash
   docker compose up -d
   cargo run -- db migrate
   cargo run -- http
   ```

   Startup confirms the discovery document and initial JWKS are available. A failure here usually means the issuer URL is wrong, the provider is unreachable, or no compatible signing keys are published.

### Obtain a token and call the API

For a client-credentials client, set the client credentials in your shell (use your secret manager or CI secret store outside local development), then obtain a token from the `token_endpoint` in the discovery document:

```bash
export OIDC_CLIENT_ID=your-client-id
export OIDC_CLIENT_SECRET=your-client-secret
```

The exact token endpoint and optional provider-specific fields differ by provider, but the standard request is:

```bash
ACCESS_TOKEN="$(curl --fail --silent --show-error \
  --request POST "https://id.example.com/realms/demo/protocol/openid-connect/token" \
  --header 'content-type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=client_credentials' \
  --data-urlencode "client_id=$OIDC_CLIENT_ID" \
  --data-urlencode "client_secret=$OIDC_CLIENT_SECRET" \
  --data-urlencode 'scope=users:read users:write' \
  | jq -r '.access_token')"
```

Keep client secrets and access tokens out of source control, shell history where possible, and logs. Some providers require an additional `audience` or `resource` parameter; set it to the same value as `OIDC_AUDIENCE` when required by that provider.

Send the access token in the standard Authorization header:

```bash
# Public endpoint: no token needed.
curl -i http://localhost:3000/health/ready

# Requires users:read.
curl -i http://localhost:3000/api/v1/users \
  --header "authorization: Bearer $ACCESS_TOKEN"

# Requires users:write.
curl -i --request POST http://localhost:3000/api/v1/users \
  --header "authorization: Bearer $ACCESS_TOKEN" \
  --header 'content-type: application/json' \
  --data '{"email":"ada@example.com","display_name":"Ada Lovelace"}'
```

| Response | Meaning | Usual resolution |
| --- | --- | --- |
| `401 unauthorized` | Missing, malformed, expired, or invalid token | Obtain a new access token and check issuer, audience, signing algorithm, and clock. |
| `403 insufficient_scope` | Valid token lacks the endpoint's scope | Assign `users:read` or `users:write` in the provider, then obtain a new token. |
| `503 authentication_unavailable` | An unknown key or stale JWKS required refresh while the provider was unavailable | Restore provider connectivity and retry; only keys still within the configured cache age remain usable. |

### Protecting future API routes

Keep authorization at the HTTP boundary. Add a new route to the read or write router in `src/presentation/http/router.rs`, then attach the appropriate `ScopeRequirement` middleware. Use `users:read` for safe read routes and `users:write` for state-changing routes. Add an HTTP test for missing credentials, allowed scope, and insufficient scope. Do not authorize by trusting client-supplied user IDs or by parsing JWTs inside handlers; protected handlers can extract the verified `AuthenticatedPrincipal` from the request.

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

Logs are structured JSON on stdout and are also exported directly to SigNoz through OTLP. The log bridge includes the active trace and span context, so SigNoz can correlate each log record with its trace. Export is opt-in so local development works without a collector:

```bash
# Self-hosted SigNoz OTLP/HTTP
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
OTEL_SERVICE_NAME=base-skeleton-rust

# SigNoz Cloud also needs its ingestion header
OTEL_EXPORTER_OTLP_HEADERS=signoz-ingestion-key=your-ingestion-key
```

The service accepts W3C `traceparent` and `tracestate` headers, exports traces to `/v1/traces` and logs to `/v1/logs`, and stores the resulting trace context with every durable job. The worker restores that context before handling the job, so an HTTP request and its asynchronous work appear in one distributed trace. Job payloads, authorization headers, cookies, and request bodies are never added to spans or logs.

Metrics share the OpenTelemetry meter provider and export through OTLP `/v1/metrics` whenever the OTLP endpoint is set. They cover HTTP count/duration/active requests, rate-limit rejections, job outcome/duration, cleanup deletions, and worker errors. Labels use normalized route templates and bounded outcomes—never IPs, subjects, UUIDs, or raw paths.

Set `METRICS_PROMETHEUS_BEARER_TOKEN` from secret management to mount `GET /metrics`, and configure the scraper with the same Bearer credential. Without it the route does not exist. Comparison is constant-time and the token is never logged.

For self-hosted deployments, OTLP/HTTP is normally exposed on port `4318`; use the regional ingest endpoint supplied by SigNoz Cloud for cloud deployments. See the [SigNoz Rust instrumentation guide](https://signoz.io/docs/instrumentation/opentelemetry-rust/) and [self-hosted ingestion overview](https://signoz.io/docs/ingestion/self-hosted/overview/).

### Run with tracing and logging

1. Apply the queue migration, then start the service with the OTLP endpoint configured:

   ```bash
   cargo run -- db migrate
   OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
   OTEL_SERVICE_NAME=base-skeleton-rust \
   cargo run -- all
   ```

2. Send a request. The application writes JSON logs to stdout and exports both logs and traces asynchronously:

   ```bash
   curl -i -X POST http://localhost:3000/api/v1/users \
     -H 'authorization: Bearer <access-token-with-users:write>' \
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

Clients or upstream services should forward W3C `traceparent` and optional `tracestate` headers. This service extracts them for incoming requests and uses them as the parent trace context. The OTLP log bridge sends `tracing` events directly to SigNoz and includes their active trace context; stdout JSON remains available for local development or a second log pipeline.

Only safe operational fields are recorded: method, path, status, error code, user ID, page values, job ID/type, cache TTL, and database/cache system. HTTP 5xx responses are marked as error spans, while expected 4xx responses produce warning logs without turning the server span into an error. The application intentionally excludes request bodies, emails, display names, job payloads, authorization headers, cookies, SQL parameters, and connection strings.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Set `TEST_DATABASE_URL` to include the real PostgreSQL queue lifecycle test locally. Without it, that one test returns early; all dependency-free tests still run. CI provisions PostgreSQL and always executes the database path.

CI treats a missing `TEST_DATABASE_URL` as an error, performs locked release and Docker builds, and runs `cargo audit`. A weekly schedule catches advisories published after dependencies were last changed.

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/base_skeleton \
  cargo test --test postgres_job_queue
```

The same checks run automatically through `.github/workflows/ci.yml`.
