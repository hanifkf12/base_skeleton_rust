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

## Adding a New Domain

Suppose the application needs an `Order` domain.

### 1. Define the business model

Create the domain module first:

```text
src/domain/order/
├── mod.rs
├── entity.rs
├── value_object.rs
└── error.rs
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

Put business invariants in constructors and domain methods. For example, an order should not accept a negative total, and a cancelled order may not be shipped.

Do not add Axum request types or SQLx row derives to the domain entity.

### 2. Add application actions

Create only the use cases currently required by the application:

```text
src/application/order/
├── mod.rs
├── dto.rs
├── error.rs
├── ports.rs
└── use_cases/
    ├── create_order.rs
    ├── get_order.rs
    └── cancel_order.rs
```

Define the persistence contract in the application layer:

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

Avoid a generic `BaseRepository<T>`. Repository methods should represent operations needed by real use cases.

### 3. Implement infrastructure adapters

Add the PostgreSQL implementation:

```text
src/infrastructure/database/postgres/order_repository.rs
```

```rust
pub struct PostgresOrderRepository {
    pool: PgPool,
}

#[async_trait]
impl OrderRepository for PostgresOrderRepository {
    // SQLx implementation.
}
```

Add a database migration for the new tables or columns.

### 4. Expose the use cases through HTTP

Add presentation components:

```text
src/presentation/http/order/
├── mod.rs
├── handlers.rs
├── request.rs
└── response.rs
```

Keep HTTP validation separate from domain validation:

- HTTP validation checks the request shape and required fields.
- Domain validation protects business invariants.

### 5. Wire dependencies

Update bootstrap and presentation composition:

1. Construct `PostgresOrderRepository` in `bootstrap/dependencies.rs`.
2. Construct the order use cases.
3. Add the use cases to `AppState`.
4. Register the order routes in `presentation/http/router.rs`.

### 6. Add tests

Test each layer according to its responsibility:

- Domain tests verify business rules and state transitions.
- Application tests use fake ports and verify orchestration.
- Presentation tests verify routes, status codes, and JSON mapping.
- Infrastructure integration tests verify real PostgreSQL or Redis behavior.

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
