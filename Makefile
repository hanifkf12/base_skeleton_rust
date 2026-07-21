SHELL := /bin/sh
.DEFAULT_GOAL := help

CARGO ?= cargo
DOCKER ?= docker
COMPOSE ?= $(DOCKER) compose
CURL ?= curl

IMAGE_NAME ?= base-skeleton-rust
IMAGE_TAG ?= local
ENV_FILE ?= .env
APP_PORT ?= 3000
LOCAL_TEST_DATABASE_URL ?= postgres://postgres:postgres@localhost:5432/base_skeleton

.PHONY: help env deps-up deps-down deps-logs \
	http worker all all-migrate \
	db-migrate db-info db-revert \
	fmt fmt-check clippy test test-postgres check build release audit ci \
	docker-build docker-migrate docker-http docker-worker health

help: ## Show available targets.
	@awk 'BEGIN {FS = ":.*## "; printf "Usage: make <target>\n\nTargets:\n"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

env: ## Create .env from .env.example when it does not exist.
	@test -f "$(ENV_FILE)" || cp .env.example "$(ENV_FILE)"

deps-up: ## Start PostgreSQL and Redis with Docker Compose.
	$(COMPOSE) up --detach

deps-down: ## Stop Docker Compose dependencies without deleting data.
	$(COMPOSE) down

deps-logs: ## Follow Docker Compose dependency logs.
	$(COMPOSE) logs --follow

http: ## Start only the HTTP API.
	$(CARGO) run -- http

worker: ## Start only the background-job worker.
	$(CARGO) run -- worker

all: ## Start the HTTP API and worker in one process.
	$(CARGO) run -- all

all-migrate: ## Apply migrations, then start HTTP and worker.
	$(CARGO) run -- all --migrate

db-migrate: ## Apply all pending database migrations.
	$(CARGO) run -- db migrate

db-info: ## Show database migration status.
	$(CARGO) run -- db info

db-revert: ## Revert the latest reversible migration.
	$(CARGO) run -- db revert --yes

fmt: ## Format all Rust source files.
	$(CARGO) fmt --all

fmt-check: ## Check Rust formatting without changing files.
	$(CARGO) fmt --all -- --check

clippy: ## Run Clippy for all targets and features with warnings denied.
	$(CARGO) clippy --all-targets --all-features -- -D warnings

test: ## Run all tests; PostgreSQL tests use TEST_DATABASE_URL when set.
	$(CARGO) test --all-targets --all-features

test-postgres: ## Run the PostgreSQL tests against the local Compose database.
	TEST_DATABASE_URL="$(LOCAL_TEST_DATABASE_URL)" $(CARGO) test --test postgres_job_queue

check: fmt-check clippy test ## Run formatting, lint, and test checks.

build: ## Build the debug binary using the lockfile.
	$(CARGO) build --locked

release: ## Build the optimized production binary using the lockfile.
	$(CARGO) build --release --locked

audit: ## Audit Rust dependencies; requires cargo-audit.
	$(CARGO) audit

ci: fmt-check clippy test release audit docker-build ## Run the same quality gates as CI.

docker-build: ## Build the production container image.
	$(DOCKER) build --tag "$(IMAGE_NAME):$(IMAGE_TAG)" .

docker-migrate: ## Run migrations using the production image.
	$(DOCKER) run --rm --env-file "$(ENV_FILE)" "$(IMAGE_NAME):$(IMAGE_TAG)" db migrate

docker-http: ## Run the HTTP API using the production image.
	$(DOCKER) run --rm --env-file "$(ENV_FILE)" --publish "$(APP_PORT):$(APP_PORT)" "$(IMAGE_NAME):$(IMAGE_TAG)" http

docker-worker: ## Run the worker using the production image.
	$(DOCKER) run --rm --env-file "$(ENV_FILE)" "$(IMAGE_NAME):$(IMAGE_TAG)" worker

health: ## Check the local liveness endpoint.
	$(CURL) --fail --silent --show-error "http://127.0.0.1:$(APP_PORT)/health/live"
