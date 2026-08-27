FROM rust:1.93-bookworm AS builder

WORKDIR /app
# Stub build warms the dependency layer; real sources are copied after so
# source edits don't recompile every crate.
COPY Cargo.toml Cargo.lock build.rs ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/deps/base_skeleton_rust*
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 app

COPY --from=builder /app/target/release/base_skeleton_rust /usr/local/bin/base_skeleton_rust

USER app
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3000/health/live || exit 1
ENTRYPOINT ["base_skeleton_rust"]
CMD ["http"]
