# Rust Axum Backend Skeleton Prompt — Clean Architecture & SOLID

## Clean Architecture and SOLID Requirements

The project must use Clean Architecture and apply SOLID principles in an idiomatic Rust style.

The architecture should separate the system into the following layers:

### 1. Domain Layer

The domain layer contains core business concepts and rules.

It may contain:

- Entities
- Value objects
- Domain errors
- Domain services when genuinely necessary
- Repository contracts required by the domain or application layer

The domain layer must:

- Contain no Axum-specific code
- Contain no SQLx-specific code
- Contain no Redis-specific code
- Contain no HTTP request or response types
- Contain no infrastructure configuration
- Be independently testable
- Depend only on Rust standard library and minimal domain-safe crates

Domain entities must not derive database-specific or HTTP-specific behavior unless there is a strong and documented reason.

### 2. Application Layer

The application layer contains application use cases and orchestration.

It may contain:

- Use cases
- Commands and queries
- Input and output DTOs
- Ports for external dependencies
- Transaction boundaries
- Application-level error mapping

Each use case should represent one application action, such as:

- Create user
- Get user
- List users
- Update user
- Delete user

The application layer may depend on the domain layer, but it must not depend directly on:

- Axum
- SQLx
- Redis
- Reqwest implementation details
- Concrete infrastructure implementations

External dependencies must be accessed through small and focused ports.

Example:

```rust
#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: NewUser) -> Result<User, RepositoryError>;
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError>;
    async fn update(&self, user: User) -> Result<User, RepositoryError>;
    async fn delete(&self, id: UserId) -> Result<(), RepositoryError>;
}
```

Adjust the signatures to match idiomatic Rust and the selected crate versions.

### 3. Presentation Layer

The presentation layer handles communication with external clients.

For the HTTP implementation, it contains:

- Axum routes
- Handlers
- HTTP request DTOs
- HTTP response DTOs
- Request validation
- Authentication extraction
- HTTP error mapping
- Middleware

Handlers must remain thin.

A handler should generally:

1. Extract and validate input.
2. Convert HTTP input into an application input.
3. Call one application use case.
4. Convert the result into an HTTP response.

Handlers must not:

- Execute SQL queries
- Access Redis directly
- Contain business rules
- Construct infrastructure clients
- Implement transaction logic

### 4. Infrastructure Layer

The infrastructure layer contains concrete implementations of external dependencies.

It may contain:

- SQLx PostgreSQL repositories
- Redis cache implementation
- Reqwest external API client
- Configuration loaders
- Database migrations
- Telemetry exporters

Infrastructure implementations must satisfy ports defined by an inner layer.

Examples:

- `PostgresUserRepository` implements `UserRepository`
- `RedisCache` implements `CachePort`
- `ReqwestExternalApiClient` implements `ExternalApiPort`

The infrastructure layer may depend on domain and application contracts.

The domain and application layers must never depend on infrastructure implementations.

### 5. Bootstrap and Dependency Wiring

Dependency construction must happen in the outermost layer.

The bootstrap module is responsible for:

- Loading configuration
- Creating the PostgreSQL pool
- Creating the Redis pool
- Creating the HTTP client
- Creating repository implementations
- Creating use cases
- Building application state
- Building the Axum router
- Starting the HTTP server
- Handling graceful shutdown

Do not use a service locator or global mutable dependency container.

Use explicit constructor injection.

## Dependency Rule

Dependencies must point inward:

```text
Presentation ────────┐
                     ▼
Infrastructure → Application → Domain
```

A more precise interpretation is:

- Domain depends on no outer layer.
- Application depends on Domain.
- Presentation depends on Application and HTTP-facing shared types.
- Infrastructure depends on Application and Domain contracts.
- Bootstrap knows all concrete implementations and wires them together.

Infrastructure and Presentation must not depend directly on each other except through bootstrap-level composition.

## SOLID Principles

Apply SOLID principles pragmatically.

### Single Responsibility Principle

Each module, struct, and use case should have one clear responsibility.

Examples:

- A handler handles HTTP concerns.
- A use case coordinates one business action.
- A repository handles persistence.
- A cache implementation handles cache storage.
- An API client handles communication with an external service.

Do not place validation, SQL access, caching, and response formatting in one function.

### Open/Closed Principle

Core application behavior should be extensible by implementing ports rather than modifying domain logic.

For example, replacing Redis with another cache should require a new cache adapter, not changes to the use case.

Do not introduce generic extension points for hypothetical future requirements.

### Liskov Substitution Principle

Every implementation of a port must follow the same behavioral contract.

For example:

- Repository implementations must distinguish “not found” from infrastructure failure consistently.
- Cache implementations must apply the same TTL semantics.
- External API implementations must map timeout and transport errors consistently.

Document non-obvious contracts and verify them with shared tests where useful.

### Interface Segregation Principle

Prefer small, focused traits.

Avoid large interfaces such as:

```rust
trait ApplicationDependencies {
    // database methods
    // cache methods
    // email methods
    // external API methods
    // logging methods
}
```

Instead, define ports based on what a use case actually requires.

A read-only use case should not be forced to depend on write operations unless combining them has a clear practical benefit.

### Dependency Inversion Principle

Application and domain logic must depend on abstractions at external boundaries.

Concrete adapters must be injected from the bootstrap layer.

Define traits only when at least one of these conditions applies:

- The dependency crosses an architectural boundary.
- Multiple implementations are expected.
- A fake implementation is needed for meaningful tests.
- The abstraction represents a stable business capability.

Do not create traits for:

- Plain data structures
- Utility functions
- Configuration structs
- Every service by default
- Types that will only ever have one simple implementation and do not cross a boundary

## Recommended Project Structure

Use the following structure as the initial direction:

```text
src/
├── main.rs
├── bootstrap/
│   ├── mod.rs
│   ├── app.rs
│   └── dependencies.rs
├── config/
├── domain/
│   └── user/
│       ├── mod.rs
│       ├── entity.rs
│       ├── value_object.rs
│       ├── error.rs
│       └── repository.rs
├── application/
│   └── user/
│       ├── mod.rs
│       ├── dto.rs
│       ├── ports.rs
│       └── use_cases/
├── presentation/
│   └── http/
│       ├── router.rs
│       ├── state.rs
│       ├── middleware/
│       └── user/
├── infrastructure/
│   ├── database/
│   │   └── postgres/
│   ├── cache/
│   │   └── redis/
│   └── external_api/
├── shared/
│   ├── error/
│   ├── response/
│   ├── datetime/
│   ├── pagination/
│   ├── validation/
│   └── id/
└── telemetry/
```

The structure may be adjusted when the implementation provides a concrete reason.

Do not create empty directories, empty traits, placeholder layers, or speculative abstractions merely to match the diagram.

## Use-Case Design

Use cases should be represented by concrete structs with explicit dependencies.

Example:

```rust
pub struct GetUserUseCase<R, C> {
    repository: R,
    cache: C,
}
```

Alternatively, trait objects may be used when they substantially simplify application state and dependency wiring:

```rust
pub struct GetUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn CachePort>,
}
```

Choose either static dispatch or dynamic dispatch consistently based on practical application needs.

Explain the choice in the architecture plan.

Do not mix generic and dynamic dependency injection without a clear reason.

## Caching Boundary

Caching is an infrastructure concern, but cache-aside orchestration may be performed by the application use case because it defines application behavior.

The application layer must depend on a cache port, not directly on Redis.

For example:

```rust
#[async_trait]
pub trait CachePort: Send + Sync {
    async fn get_json<T>(&self, key: &str) -> Result<Option<T>, CacheError>
    where
        T: DeserializeOwned + Send;

    async fn set_json<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<(), CacheError>
    where
        T: Serialize + Sync;

    async fn delete(&self, key: &str) -> Result<(), CacheError>;
}
```

If generic async methods make trait objects impractical, redesign the boundary using:

- Domain-specific cache methods
- Serialized byte or string values
- A concrete generic cache dependency
- Separate typed cache adapters

Explain the selected tradeoff. Do not force an object-unsafe trait design.

## Transaction Boundary

Transactions should be controlled at the application use-case boundary when one business operation requires multiple database writes.

Do not expose SQLx transaction types to the domain layer.

Use one of these approaches:

- A unit-of-work port
- A transaction closure owned by the infrastructure adapter
- A repository method representing one atomic persistence operation

Choose the simplest design that satisfies actual use cases.

Do not add a generic unit-of-work abstraction unless it is used by at least one real application flow.

## Error Boundaries

Each layer should own errors relevant to its responsibility:

- Domain errors represent violated business rules.
- Application errors represent failed use cases.
- Infrastructure errors represent database, Redis, or HTTP client failures.
- Presentation errors map application outcomes into HTTP responses.

Do not leak:

- `sqlx::Error`
- Redis driver errors
- `reqwest::Error`
- Internal stack traces

outside the infrastructure boundary.

Preserve the original error as a source for logging and debugging.

## Anti-Overengineering Rules

Clean Architecture and SOLID must not be used as justification for unnecessary complexity.

Do not:

- Create one trait for every struct
- Create factories for simple constructors
- Create empty domain services
- Wrap every external type without a concrete benefit
- Add command bus, query bus, mediator, or event bus
- Add CQRS unless explicitly requested
- Add domain events unless an actual workflow uses them
- Add generic repository abstractions
- Create `BaseRepository<T>`
- Create one crate per layer at the beginning
- Create deeply nested modules for trivial functionality
- Add abstractions only because they may be useful later

Start as a single Rust crate with internal modules.

A multi-crate workspace may only be introduced when there is a demonstrated need for independent compilation, reuse, ownership, or deployment.

## Architecture Review Requirement

Before implementation, provide:

1. Layer responsibilities.
2. Dependency direction.
3. Ports and adapters that will be created.
4. Which components use traits and why.
5. Static dispatch versus dynamic dispatch decision.
6. Transaction boundary strategy.
7. Cache boundary strategy.
8. Error ownership for each layer.
9. Testing strategy per layer.
10. Abstractions intentionally excluded to avoid overengineering.

Do not write implementation code until this architecture review is complete.
