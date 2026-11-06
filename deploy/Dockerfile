# Build context is the repository root; standalone copy avoids other workspace crates.
FROM rust:1.88-bookworm AS builder
WORKDIR /build
COPY crates/relay/ ./
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 relay
COPY --from=builder /build/target/release/lightremote-relay /usr/local/bin/lightremote-relay
COPY web/dist/ /web/
USER 10001:10001
EXPOSE 8787
ENV LIGHTREMOTE_BIND=0.0.0.0:8787 LIGHTREMOTE_WEB_DIR=/web
ENTRYPOINT ["lightremote-relay"]
