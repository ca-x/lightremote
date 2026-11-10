# Build context is the repository root; standalone copy avoids other workspace crates.
FROM rust:1.88-bookworm AS builder
WORKDIR /build
COPY crates/relay/ ./
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 relay
COPY --from=builder /build/target/release/roc-remote-relay /usr/local/bin/roc-remote-relay
COPY web/dist/ /web/
USER 10001:10001
EXPOSE 8787
ENV ROC_REMOTE_BIND=0.0.0.0:8787 ROC_REMOTE_WEB_DIR=/web
ENTRYPOINT ["roc-remote-relay"]
