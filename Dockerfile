FROM rust:1.83-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build
COPY Cargo.toml .
COPY src ./src

RUN cargo build --release

FROM scratch
COPY --from=builder /build/target/release/adi-worker /adi-worker
