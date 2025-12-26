FROM rust:1.83-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build

# Copy dependency first
COPY crates/lib-tarminal-sync /lib-tarminal-sync

# Copy cocoon source
COPY crates/cocoon/Cargo.toml .
COPY crates/cocoon/src ./src

RUN cargo build --release

FROM alpine:latest
RUN apk add --no-cache ca-certificates
COPY --from=builder /build/target/release/cocoon /usr/local/bin/cocoon

ENV SIGNALING_SERVER_URL=ws://signaling:8080/ws
ENV COCOON_ID=""

CMD ["/usr/local/bin/cocoon"]
