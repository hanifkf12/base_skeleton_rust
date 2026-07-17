# Architecture Guide

This project follows Clean Architecture and applies SOLID principles pragmatically. Business rules stay in the center of the application, while Axum, PostgreSQL, Redis, and other external systems remain replaceable implementation details.

## Request Flow

```text
HTTP request
    ↓
Presentation / Axum
    ↓
Application use case
    ↓
Domain rules
    ↓
Application port (trait)
    ↓
Infrastructure adapter
    ↓
PostgreSQL / Redis
```

The dependency direction is:

```text
Presentation ────────┐
                     ▼
Infrastructure → Application → Domain
```

- The domain does not depend on any outer layer.
- The application depends on the domain.
- Presentation depends on application use cases.
- Infrastructure implements ports owned by the application.
- Bootstrap is the only layer that knows all concrete implementations.

## Layers

### Domain

Location: `src/domain`

The domain contains pure business concepts and rules:

- `User`
- `UserId`
- `Email`
- `DisplayName`
- `UserError`

Email and display-name validation belongs here because those rules remain true regardless of whether the caller is an HTTP handler, CLI command, test, or background worker.

The domain must not know about:

- Axum
- JSON or HTTP responses
- SQLx or database rows
- PostgreSQL
- Redis
- Infrastructure configuration

### Application

Location: `src/application`

The application layer defines what the system can do. The current user actions are:

- Create user
- Get user
- List users
- Update user
- Delete user

Each action is represented by a concrete use-case struct with explicit dependencies:

```rust
pub struct CreateUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
    cache_ttl_seconds: u64,
}
```

The application owns external-boundary contracts:

```rust
pub trait UserRepository {
    // Persistence operations required by user use cases.
}

pub trait UserCache {
    // User-specific cache operations.
}
```

The application knows that users must be persisted and cached, but it does not know that PostgreSQL and Redis provide those capabilities.

Dynamic dispatch with `Arc<dyn Port>` is used at external boundaries because dependencies are shared by Axum state and can easily be replaced by fakes in tests. Use cases themselves remain concrete structs.

### Infrastructure

Location: `src/infrastructure`

Infrastructure contains concrete implementations of application ports:

```text
UserRepository → PostgresUserRepository
UserCache      → RedisUserCache / NoOpUserCache
```

Infrastructure is responsible for:

- Executing SQL queries.
- Serializing cached values.
- Calling external systems.
- Logging adapter failures.
- Converting driver errors into application-level errors.

SQLx and Redis errors must not escape this boundary.

### Presentation

Location: `src/presentation`

Presentation owns HTTP-specific concerns:

- Axum routes and extractors.
- Request and response DTOs.
- JSON serialization.
- HTTP status codes.
- HTTP error mapping.

A handler should only:

1. Extract HTTP input.
2. Convert it into an application input.
3. Execute one use case.
4. Convert the result into an HTTP response.

Example:

```rust
let user = state.create_user.execute(request.into()).await?;
Ok((StatusCode::CREATED, Json(...)))
```

Handlers must not contain business rules, SQL queries, Redis access, or dependency construction.

### Bootstrap

Location: `src/bootstrap`

Bootstrap is the composition root. It is responsible for:

- Loading configuration.
- Creating the PostgreSQL connection pool.
- Running database migrations.
- Creating the Redis connection manager when Redis is configured and reachable.
- Falling back to `NoOpUserCache` when the optional cache is unavailable.
- Constructing repository and cache adapters.
- Injecting adapters into application use cases.
- Creating Axum application state.
- Building and starting the HTTP server.
- Handling graceful shutdown.

Conceptually:

```text
PostgresUserRepository ─┐
                       ├─→ CreateUserUseCase ─→ AppState ─→ Axum Router
RedisUserCache ─────────┘
```

PostgreSQL is a required dependency and powers `/health/ready`. Redis is deliberately excluded from readiness because it is an optional optimization rather than a source of truth. `/health/live` only reports that the process is running.

No service locator or global mutable dependency container is used.

## Adding a Feature to an Existing Domain

A feature is one new application action inside an existing business area. Examples include suspending a user, changing a user's email, or searching users. Do not create a new domain module when the behavior naturally belongs to `User`.

The recommended implementation order follows the dependency direction from the inside out.

### Step 1: Write the behavior in plain language

Before creating files, define:

- The action and actor.
- Required input.
- Business rules and state changes.
- Successful output.
- Expected failures.
- Required external dependencies.

Example:

```text
Action: Suspend user
Input: UserId and suspension reason
Rules: An already suspended user cannot be suspended again
Output: Updated user
Failures: User not found, already suspended, repository unavailable
```

This prevents HTTP or database concerns from accidentally becoming business rules.

### Step 2: Update the domain only when business behavior changes

If the feature introduces a business state or invariant, update `src/domain/user` first:

```text
src/domain/user/
├── entity.rs         # Add suspend() or another domain operation
├── value_object.rs   # Add SuspensionReason when it protects an invariant
├── error.rs          # Add AlreadySuspended or another domain failure
└── mod.rs            # Export new public domain types
```

Example:

```rust
impl User {
    pub fn suspend(&mut self, reason: SuspensionReason) -> Result<(), UserError> {
        if self.is_suspended() {
            return Err(UserError::AlreadySuspended);
        }

        self.status = UserStatus::Suspended;
        self.suspension_reason = Some(reason);
        self.updated_at = Utc::now();
        Ok(())
    }
}
```

Do not put SQL, status codes, JSON field names, or cache keys in this method.

If the feature only changes orchestration and introduces no business rule, the domain may require no change.

### Step 3: Add one application use case

Create one concrete use case for the action:

```text
src/application/user/use_cases/suspend_user.rs
```

Add its input DTO to `src/application/user/dto.rs`, export the use case from `use_cases/mod.rs`, and re-export it from `application/user/mod.rs`.

```rust
pub struct SuspendUserInput {
    pub reason: String,
}

pub struct SuspendUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
}
```

Its `execute` method should:

1. Load the user through an application port.
2. Convert raw input into domain value objects.
3. Call the domain behavior.
4. Persist the result.
5. Update or invalidate related cache entries.
6. Return an application result or `ApplicationError`.

Add a new port or port method only when the use case crosses an architectural boundary. Do not create a trait for the use case itself.

### Step 4: Implement persistence changes

If persistence changes are required:

1. Create a migration using the migration guide below.
2. Extend the relevant application port in `src/application/user/ports.rs`.
3. Implement that contract in `src/infrastructure/database/postgres/user_repository.rs`.
4. Update infrastructure row-to-domain mapping.
5. Map driver errors into repository errors without leaking `sqlx::Error`.

Prefer one repository method representing one atomic persistence operation. Introduce a transaction boundary only when the use case performs multiple writes that must succeed or fail together.

### Step 5: Add the HTTP boundary

Update the user presentation module:

```text
src/presentation/http/user/
├── handlers.rs   # Add a thin suspend_user handler
├── request.rs    # Add SuspendUserRequest
├── response.rs   # Reuse UserResponse when its shape is correct
└── mod.rs         # Export the handler
```

Register the endpoint in `src/presentation/http/router.rs`:

```rust
.route("/api/v1/users/{id}/suspension", post(suspend_user))
```

The handler should extract input, call exactly one use case, and map the output. Business decisions still belong to the domain or application layer.

### Step 6: Wire the use case

Update both composition points:

1. Add `Arc<SuspendUserUseCase>` to `src/presentation/http/state.rs`.
2. Construct it in `src/bootstrap/dependencies.rs` with explicit dependencies.

Do not construct repositories, caches, or clients inside handlers.

### Step 7: Test the vertical slice

Add tests at the boundaries changed by the feature:

- Domain test for each new invariant and valid state transition.
- Application test using fake ports for success and each expected failure.
- HTTP test in `tests/http_api.rs` for route, request, response, and error mapping.
- PostgreSQL integration test when a query or constraint is nontrivial.

Then run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

### Existing-domain feature checklist

- [ ] Business behavior is described before implementation.
- [ ] Domain invariants are enforced by domain types or methods.
- [ ] One concrete use case represents the action.
- [ ] New external access is represented by a focused port.
- [ ] SQL and driver types remain in infrastructure.
- [ ] The HTTP handler remains thin.
- [ ] Bootstrap wires all concrete dependencies.
- [ ] Cache entries are updated or invalidated.
- [ ] Migrations are additive and were tested locally.
- [ ] Domain, application, and HTTP tests cover the feature.
- [ ] Formatting, clippy, and tests pass.

## Adding a New Domain

A new domain is a distinct business area with its own language, rules, and lifecycle. Examples include `Order`, `Payment`, or `Inventory`. A different table or endpoint alone does not necessarily justify a new domain.

The example below adds an `Order` vertical slice.

### Step 1: Create the domain model

```text
src/domain/order/
├── mod.rs
├── entity.rs
├── value_object.rs
└── error.rs
```

Register it in `src/domain/mod.rs`:

```rust
pub mod order;
```

Example entity:

```rust
pub struct Order {
    id: OrderId,
    customer_id: UserId,
    status: OrderStatus,
    total: Money,
}
```

Put constructors, state transitions, and invariants here. An order might reject a negative total or prevent cancellation after shipment.

Do not derive SQLx database rows or use Axum request types in domain entities.

### Step 2: Create the application module

```text
src/application/order/
├── mod.rs
├── dto.rs
├── error.rs
├── ports.rs
└── use_cases/
    ├── mod.rs
    ├── create_order.rs
    ├── get_order.rs
    └── cancel_order.rs
```

Register it in `src/application/mod.rs`:

```rust
pub mod order;
```

Define only ports required by real use cases:

```rust
#[async_trait]
pub trait OrderRepository: Send + Sync {
    async fn create(&self, order: &Order) -> Result<Order, RepositoryError>;
    async fn find_by_id(
        &self,
        id: OrderId,
    ) -> Result<Option<Order>, RepositoryError>;
}
```

Avoid `BaseRepository<T>`, command buses, factories, or unused extension points.

### Step 3: Create the database migration

Create the `orders` table and its constraints before implementing the adapter. Follow the full migration workflow in the next section.

Name important constraints explicitly so infrastructure errors can be mapped precisely:

```sql
CREATE TABLE orders (
    id UUID PRIMARY KEY,
    customer_id UUID NOT NULL REFERENCES users(id),
    status TEXT NOT NULL,
    total_cents BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT orders_total_non_negative CHECK (total_cents >= 0)
);

CREATE INDEX orders_customer_created_idx
    ON orders (customer_id, created_at DESC, id DESC);
```

### Step 4: Implement infrastructure

```text
src/infrastructure/database/postgres/
├── order_repository.rs
└── mod.rs
```

Export `PostgresOrderRepository` from `postgres/mod.rs`. Keep `OrderRow`, SQL queries, and SQLx error inspection inside infrastructure.

Create cache or external API adapters only if an actual order use case needs them.

### Step 5: Add presentation

```text
src/presentation/http/order/
├── mod.rs
├── handlers.rs
├── request.rs
└── response.rs
```

Register `mod order` in `src/presentation/http/mod.rs`, export the handlers, and add routes to `src/presentation/http/router.rs`.

Keep validation responsibilities separate:

- HTTP validation verifies request shape and transport-level constraints.
- Domain validation protects business invariants.
- Application validation handles use-case-specific requirements.

### Step 6: Wire the domain's use cases

1. Add concrete order use cases to `src/presentation/http/state.rs`.
2. Create `PostgresOrderRepository` in `src/bootstrap/dependencies.rs`.
3. Inject the repository into each order use case.
4. Add each use case to `AppState`.
5. Pass the completed state to the router.

Bootstrap is the only layer allowed to know that `OrderRepository` is implemented by PostgreSQL.

### Step 7: Test and document

- Unit-test order value objects, invariants, and state transitions.
- Test each use case with a fake `OrderRepository`.
- Add HTTP tests for the order routes.
- Add PostgreSQL integration tests for constraints and query behavior.
- Document new endpoints and environment variables in `README.md`.
- Run all quality checks before committing.

### New-domain checklist

- [ ] The business area has distinct language and lifecycle.
- [ ] Domain types contain no Axum, SQLx, or Redis details.
- [ ] Each application action has one concrete use case.
- [ ] Ports describe required capabilities rather than technologies.
- [ ] Migrations use explicit constraints and appropriate indexes.
- [ ] Infrastructure maps database rows back into validated domain types.
- [ ] Presentation DTOs do not leak into the domain.
- [ ] Bootstrap performs all dependency construction.
- [ ] Tests cover each changed architectural boundary.
- [ ] Documentation and API examples are updated.

## Database Migration Guide

SQLx migrations live in `migrations/` and are embedded into the binary by `sqlx::migrate!()` in `src/bootstrap/dependencies.rs`. The application automatically applies pending migrations after connecting to PostgreSQL during startup.

### Install the SQLx CLI

Install a PostgreSQL-only CLI compatible with this project's SQLx version:

```bash
cargo install sqlx-cli --version 0.8.6 \
    --no-default-features \
    --features rustls,postgres
```

Verify the installation:

```bash
sqlx --version
```

### Ensure new migrations are embedded on rebuild

`sqlx::migrate!()` embeds migration files into the application binary. On stable Rust, Cargo may not notice a newly added migration when no Rust source file changed. Generate a build script once:

```bash
sqlx migrate build-script
```

This creates or updates `build.rs` with the equivalent of:

```rust
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
```

Commit `build.rs`. Afterward, adding or changing a migration causes Cargo to rebuild the embedded migrator.

### Start PostgreSQL and configure the connection

```bash
cp .env.example .env
docker compose up -d postgres
```

The CLI reads `DATABASE_URL` from `.env`. The default local value is:

```text
postgres://postgres:postgres@localhost:5432/base_skeleton
```

Confirm PostgreSQL is ready:

```bash
docker compose ps
```

### Create a forward-only migration

Use a short, descriptive, snake-case name:

```bash
sqlx migrate add create_orders
```

SQLx creates a timestamped file similar to:

```text
migrations/20260717120000_create_orders.sql
```

Edit that file with the forward schema change. Prefer explicit constraint and index names:

```sql
ALTER TABLE users
    ADD COLUMN status TEXT NOT NULL DEFAULT 'active';

ALTER TABLE users
    ADD CONSTRAINT users_status_allowed
    CHECK (status IN ('active', 'suspended'));

CREATE INDEX users_status_created_idx
    ON users (status, created_at DESC, id DESC);
```

Do not add `IF NOT EXISTS` merely to hide schema drift. A migration should fail when the database is not in the expected state.

### Create a reversible migration

For local development or schema changes with a genuinely safe rollback, create paired files:

```bash
sqlx migrate add -r add_users_status
```

This creates files similar to:

```text
migrations/20260717120500_add_users_status.up.sql
migrations/20260717120500_add_users_status.down.sql
```

After reversible migrations are introduced, SQLx CLI creates subsequent migrations as reversible pairs as well. Check both generated files before committing.

The `up` file applies the change:

```sql
ALTER TABLE users
    ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
```

The `down` file reverses it:

```sql
ALTER TABLE users DROP COLUMN status;
```

Only write a down migration when rollback is safe. Dropping a column or table destroys data and is usually unsuitable for production rollback.

### Inspect pending and applied migrations

```bash
sqlx migrate info
```

SQLx records applied migrations and checksums in the `_sqlx_migrations` table.

### Apply migrations manually

Apply all pending migrations:

```bash
sqlx migrate run
```

Apply migrations against an explicit database URL when necessary:

```bash
sqlx migrate run \
    --database-url postgres://postgres:postgres@localhost:5432/base_skeleton
```

Starting the application also applies pending migrations:

```bash
cargo run
```

For production, prefer a dedicated deployment migration job before starting new application replicas. This makes schema failures visible before traffic reaches the new version.

### Revert the latest reversible migration

```bash
sqlx migrate revert
```

Inspect migration state afterward:

```bash
sqlx migrate info
```

`migrate revert` requires a reversible migration with a matching `.down.sql` file. For production incidents, prefer a new forward migration that corrects the schema; reverting application code and schema independently can cause compatibility failures.

### Never edit an applied migration

After a migration has been shared or applied to any persistent environment:

- Do not rename it.
- Do not change its SQL.
- Do not reorder it.
- Do not delete it.

SQLx verifies stored checksums and will reject a changed migration. Create a new migration to correct or extend the schema. Migration `0002_name_users_email_constraint.sql` demonstrates this pattern: it corrects the existing schema without modifying `0001_create_users.sql`.

### Production-safe schema changes

For large or live tables, use an expand-and-contract workflow.

#### Adding a required column

Avoid adding a non-null column that requires a long table rewrite in one deployment. Use stages:

1. Add the column as nullable or with a safe default.
2. Deploy code that writes both the old and new representation when necessary.
3. Backfill existing rows in bounded batches.
4. Verify no null or invalid values remain.
5. Add the `NOT NULL` or validation constraint in a later migration.
6. Remove obsolete columns only after all deployed code stops using them.

#### Renaming or removing a column

Do not immediately rename or drop a column used by running application instances:

1. Add the replacement column.
2. Deploy code compatible with both columns.
3. Backfill and verify data.
4. Switch reads to the replacement.
5. Stop writing the old column.
6. Drop it in a later deployment.

#### Creating large indexes

PostgreSQL may lock writes while building a normal index. For a large production table, consider `CREATE INDEX CONCURRENTLY`. Confirm the SQLx/PostgreSQL transaction requirements before using it and place it in a dedicated migration.

### Migration verification checklist

- [ ] Migration filename and description clearly state the change.
- [ ] Existing applied migration files remain unchanged.
- [ ] Constraint and index names are explicit and stable.
- [ ] Forward migration succeeds on a fresh database.
- [ ] Forward migration succeeds on a database with all previous migrations.
- [ ] Application starts successfully after migration.
- [ ] Repository and HTTP tests pass against the new schema.
- [ ] Locking and table size were considered for production changes.
- [ ] Destructive changes have a backup and recovery plan.
- [ ] Rollback uses a tested down migration or a forward corrective migration.

## Adding Domain Events

A domain event describes a meaningful business fact that has already happened:

- `UserRegistered`
- `OrderPlaced`
- `OrderCancelled`
- `PaymentCompleted`

Do not introduce events merely to call another Rust module. Add an event when a real workflow or external consumer reacts to that business fact.

Examples include:

- Sending a welcome email after user registration.
- Reserving inventory after an order is placed.
- Starting fulfillment after payment completes.
- Writing important business activity to an audit stream.

### Define the event in its domain

For the first user event, keep it local to the user domain:

```text
src/domain/user/event.rs
```

```rust
use chrono::{DateTime, Utc};

use super::UserId;

#[derive(Debug, Clone)]
pub struct UserRegistered {
    pub user_id: UserId,
    pub occurred_at: DateTime<Utc>,
}
```

Export it from `src/domain/user/mod.rs`:

```rust
mod event;

pub use event::UserRegistered;
```

Do not create a generic `DomainEvent` trait for a single event. Introduce a shared abstraction only when multiple events have a demonstrated common requirement.

### Define the publishing port

Publishing crosses an architectural boundary, so the application should own the contract:

```rust
#[async_trait]
pub trait UserEventPublisher: Send + Sync {
    async fn publish_registered(
        &self,
        event: UserRegistered,
    ) -> Result<(), EventPublishingError>;
}
```

Inject the publisher into the use case that produces the event:

```rust
pub struct CreateUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
    event_publisher: Arc<dyn UserEventPublisher>,
}
```

The use case can construct the event after creating the user:

```rust
let created = self.repository.create(&user).await?;

let event = UserRegistered {
    user_id: created.id(),
    occurred_at: Utc::now(),
};

self.event_publisher.publish_registered(event).await?;
```

The concrete publisher belongs in infrastructure:

```text
src/infrastructure/events/
├── mod.rs
└── user_event_publisher.rs
```

It may publish to Kafka, RabbitMQ, NATS, AWS SNS/SQS, or another external system.

## Event Delivery Guarantees

Choose the delivery guarantee before implementing event publication.

### Best-effort delivery

```text
Save user → Publish event
```

This approach is simple, but the process can terminate after the user is saved and before the event is published.

Use best-effort delivery only when losing an occasional event is acceptable, such as noncritical analytics or optional notifications.

The application must also decide whether a publishing failure:

- Fails the request.
- Is logged and ignored.
- Is retried in the background.

The choice is business behavior and should be explicit in the use case.

### Reliable delivery with a transactional outbox

Use an outbox when an event must not be lost:

```text
Database transaction
    ├── Save business entity
    └── Save event to outbox

Commit transaction

Background worker
    ├── Read unpublished outbox records
    ├── Publish events
    └── Mark records as published
```

An outbox record normally contains:

```text
event_id
event_type
aggregate_id
payload
occurred_at
published_at
attempt_count
```

For one real workflow, prefer a focused atomic port over a large generic unit-of-work abstraction:

```rust
#[async_trait]
pub trait UserRegistrationStore: Send + Sync {
    async fn save_user_and_event(
        &self,
        user: &User,
        event: &UserRegistered,
    ) -> Result<User, RepositoryError>;
}
```

The PostgreSQL implementation performs the user insert and outbox insert in one SQLx transaction.

Consumers should use `event_id` for idempotency because message brokers may deliver the same event more than once.

## Domain Event Checklist

Before adding a domain event, answer these questions:

1. What meaningful business fact does the event represent?
2. Which real consumer needs the event?
3. Is event loss acceptable?
4. Should publishing failure fail the originating request?
5. Is ordering important?
6. Can consumers process the same event more than once safely?
7. Does the entity write and event write need one transaction?
8. How will failed deliveries be retried and monitored?

If the event has no real consumer or workflow, do not add it yet.

## Avoiding Overengineering

When adding domains or events, do not introduce these components without a demonstrated need:

- A generic repository framework.
- A command or query bus.
- A mediator.
- An event bus with no real consumer.
- A generic unit-of-work abstraction.
- One crate per layer.
- A trait for every struct.
- Factories for simple constructors.

Start with concrete domain types, concrete use cases, and small ports at real architectural boundaries. Add more abstraction only when actual workflows require it.
