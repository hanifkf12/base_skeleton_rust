# Architecture & Sequence Diagrams

Ringkasan visual (Mermaid) dari `base_skeleton_rust`. Diverifikasi terhadap kode di `src/`.

## 1. Arsitektur sistem (deployment)

```mermaid
flowchart LR
    client([Client / Operator])
    kc[Keycloak<br/>OIDC provider]
    subgraph app[base_skeleton_rust binary]
        http[HTTP server<br/>Axum]
        worker[Job worker]
    end
    pg[(PostgreSQL 17<br/>users, background_jobs)]
    redis[(Redis 8<br/>user cache, optional)]
    otel[OTLP collector]
    prom[Prometheus]

    client -- "Bearer JWT" --> http
    client -- "login / token" --> kc
    http -- "discovery + JWKS" --> kc
    http --> pg
    http -. "cache-aside, fail-open" .-> redis
    worker -- "claim / complete / fail<br/>FOR UPDATE SKIP LOCKED" --> pg
    http -- "traces, logs, metrics" --> otel
    worker -- "traces, logs, metrics" --> otel
    prom -- "GET /metrics (bearer)" --> http
```

CLI mode: `http`, `worker`, `all [--migrate]` (dua komponen satu proses, pool DB dibagi dua), `db migrate|info|revert`, `migration:create`.

## 2. Arsitektur layer (Clean Architecture)

Arah dependensi selalu ke dalam. `bootstrap` adalah composition root yang menyambungkan implementasi ke port.

```mermaid
flowchart TB
    subgraph bootstrap[bootstrap - composition root]
        deps[dependencies.rs<br/>build_dependencies]
        bhttp[http.rs]
        bworker[worker.rs]
        shutdown[shutdown.rs]
    end

    subgraph presentation[presentation/http]
        router[router.rs<br/>layers + routes]
        authmw[auth.rs<br/>require_scope]
        ratelimit[rate_limit.rs]
        handlers[user handlers<br/>request / response DTO]
        errmap[error.rs<br/>ApiError]
    end

    subgraph application[application]
        uc[user use cases<br/>Create / Get / List / Update / Delete]
        jw[job::JobWorker]
        ports["Ports (traits)<br/>UserRepository, UserRegistrationRepository,<br/>UserCache, JobQueue, JobHandler,<br/>AccessTokenVerifier, ReadinessCheck"]
    end

    subgraph domain[domain]
        user[User entity<br/>Email, DisplayName, UserId<br/>errors + events]
    end

    subgraph infrastructure[infrastructure]
        pgrepo[postgres::PostgresUserRepository]
        pgq[postgres::PostgresJobQueue]
        pgready[postgres::ReadinessCheck]
        rcache[cache::RedisUserCache / NoOpUserCache]
        oidc[oidc::OidcAccessTokenVerifier]
        jh[job::UserCreatedHandler]
    end

    telemetry[telemetry<br/>OpenTelemetry + Prometheus]

    bootstrap --> presentation
    bootstrap --> infrastructure
    presentation --> application
    application --> domain
    infrastructure -. implements .-> ports
    ports --- application
    infrastructure --> domain
    telemetry -.-> presentation
    telemetry -.-> application
    telemetry -.-> infrastructure
```

## 3. Middleware & routing HTTP

```mermaid
flowchart LR
    req([Request]) --> timeout[TimeoutLayer]
    timeout --> reqid[PropagateRequestId]
    reqid --> trace[TraceLayer<br/>http_span]
    trace --> sens[SensitiveHeaders<br/>Authorization, Cookie]
    sens --> setid[SetRequestId uuid]
    setid --> metrics[record_http_metrics]
    metrics --> body[DefaultBodyLimit]
    body --> split{path}
    split -- "/health, /health/live, /health/ready" --> health[health handlers]
    split -- "/metrics" --> mtr[metrics<br/>optional bearer]
    split -- "/api/v1/users*" --> gov[GovernorLayer<br/>per-IP rate limit<br/>trusted proxy CIDRs]
    gov --> scope{method}
    scope -- "GET" --> r[require_scope users:read]
    scope -- "POST / PUT / DELETE" --> w[require_scope users:write]
    r --> h[user handler]
    w --> h
```

## 4. Sequence: request terautentikasi (GET user, cache-aside)

```mermaid
sequenceDiagram
    autonumber
    actor C as Client
    participant R as Router + middleware
    participant A as require_scope
    participant V as OidcAccessTokenVerifier
    participant H as get_user handler
    participant U as GetUserUseCase
    participant K as UserCache (Redis / NoOp)
    participant P as UserRepository (Postgres)

    C->>R: GET /api/v1/users/{id}<br/>Authorization: Bearer JWT
    R->>R: request-id, trace span, rate limit (Governor)
    R->>A: users:read required
    A->>V: verify(token)
    V->>V: validate signature (cached JWKS), iss, aud, exp
    alt token invalid
        V-->>A: InvalidToken
        A-->>C: 401 invalid_token
    else JWKS unavailable
        V-->>A: AuthenticationUnavailable
        A-->>C: 503
    else scope missing
        A-->>C: 403 insufficient_scope
    else ok
        V-->>A: AuthenticatedPrincipal
        A->>H: request + principal
        H->>U: execute(UserId)
        U->>K: get(id)
        alt cache hit
            K-->>U: User
        else miss or cache error
            U->>P: find_by_id(id)
            P-->>U: User or None
            opt found
                U->>K: set(user, ttl) - warn on failure
            end
        end
        U-->>H: User or NotFound
        H-->>C: 200 UserResponse / 404
    end
```

## 5. Sequence: buat user + enqueue job (transactional outbox)

```mermaid
sequenceDiagram
    autonumber
    actor C as Client
    participant M as require_scope (users:write)
    participant H as create_user handler
    participant U as CreateUserUseCase
    participant D as Domain (User::new)
    participant P as PostgresUserRepository
    participant DB as PostgreSQL
    participant K as UserCache

    C->>M: POST /api/v1/users {email, display_name}
    M->>H: verified principal
    H->>U: execute(CreateUserInput)
    U->>D: Email::parse, DisplayName::parse, User::new
    alt validation fails
        D-->>H: DomainError
        H-->>C: 400 / 422
    end
    U->>P: create_with_job(user, UserCreationJob "user.created")
    P->>DB: BEGIN
    P->>DB: INSERT INTO users
    P->>DB: INSERT INTO background_jobs<br/>(payload, trace_context = current W3C trace)
    P->>DB: COMMIT
    alt duplicate email (unique constraint)
        P-->>H: Conflict
        H-->>C: 409
    else ok
        P-->>U: User
        U->>K: set(user, ttl) - warn on failure
        U-->>H: User
        H-->>C: 201 Created
    end
```

## 6. Sequence: job worker (claim, lease heartbeat, retry)

```mermaid
sequenceDiagram
    autonumber
    participant L as worker::run loop
    participant W as JobWorker.run_once
    participant Q as PostgresJobQueue
    participant DB as PostgreSQL
    participant Hd as JobHandler (user.created)

    loop until shutdown signal
        opt cleanup interval elapsed
            L->>W: run_maintenance()
            W->>Q: purge_terminal(completed/dead retention)<br/>batch 1000, max 16 batches
            Q->>DB: DELETE ... FOR UPDATE SKIP LOCKED
        end
        L->>W: run_once()
        W->>Q: claim(worker_id, lease_timeout)
        Q->>DB: SELECT ... FOR UPDATE SKIP LOCKED<br/>(pending, or expired lease) then UPDATE lease, attempts+1
        alt no job
            Q-->>W: None
            W-->>L: Idle, sleep poll_interval
        else claimed
            Q-->>W: ClaimedJob (+ trace_context)
            W->>W: open span linked to producer trace
            par handler runs
                W->>Hd: handle(job)
            and heartbeat every lease/3
                loop
                    W->>Q: renew(job, worker, attempt)
                    Q->>DB: UPDATE lease WHERE owner + attempt
                    Note over W,Q: transient error: retry until deadline.<br/>LeaseLost: abort, no terminal write
                end
            end
            alt handler Ok
                W->>Q: complete(job, worker, attempt)
                Q->>DB: status = completed
            else handler Err
                W->>Q: fail(..., retry_delay = base * 2^(attempt-1), capped)
                Q->>DB: attempts < max ? status = pending + run_at : status = dead
            end
            W-->>L: Completed / RetryScheduled / DeadLettered
        end
    end
```

## 7. Sequence: startup OIDC & JWKS refresh

```mermaid
sequenceDiagram
    autonumber
    participant B as bootstrap::http::run
    participant V as OidcAccessTokenVerifier
    participant KC as Keycloak

    B->>V: new(OidcConfig)
    V->>KC: GET /.well-known/openid-configuration
    KC-->>V: issuer, jwks_uri
    V->>V: check issuer == OIDC_ISSUER_URL, validate jwks_uri
    V->>KC: GET jwks_uri
    KC-->>V: JWKS
    V-->>B: verifier ready (fail fast on error)

    Note over V: Per request: use cached keys while age < jwks_max_age.<br/>If stale or kid unknown: refresh, throttled by refresh_interval (single-flight Mutex).<br/>Refresh failure with stale keys: AuthenticationUnavailable (503).
```
