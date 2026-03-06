FROM rust:1.85-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig

WORKDIR /build

# Copy dependencies as standalone crates
COPY crates/_lib/lib-tarminal-sync ./lib-tarminal-sync
COPY crates/_lib/lib-plugin-abi ./lib-plugin-abi

# Copy cocoon core source
COPY crates/cocoon/core/Cargo.toml ./Cargo.toml
COPY crates/cocoon/core/src ./src

# Fix path dependencies to use local paths
RUN sed -i 's|path = "../_lib/lib-tarminal-sync"|path = "./lib-tarminal-sync"|g' Cargo.toml && \
    sed -i 's|path = "../_lib/lib-plugin-abi"|path = "./lib-plugin-abi"|g' Cargo.toml

# Fix workspace inheritance in dependencies
RUN cd lib-plugin-abi && \
    sed -i 's|version.workspace = true|version = "0.1.0"|g' Cargo.toml && \
    sed -i 's|edition.workspace = true|edition = "2021"|g' Cargo.toml && \
    sed -i 's|authors.workspace = true|authors = ["ADI Team"]|g' Cargo.toml && \
    sed -i 's|abi_stable.workspace = true|abi_stable = "0.11"|g' Cargo.toml

RUN cargo build --release --features standalone

FROM alpine:latest
RUN apk add --no-cache ca-certificates
COPY --from=builder /build/target/release/cocoon /usr/local/bin/cocoon

ENV SIGNALING_SERVER_URL=ws://signaling:8080/ws
ENV COCOON_ID=""

CMD ["/usr/local/bin/cocoon"]
