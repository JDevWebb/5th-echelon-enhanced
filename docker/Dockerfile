# The 5th Echelon server in a container.
#
#   docker compose -f docker/compose.yaml up -d      (from the repository root)
#
# Build context: the repository root.

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The toolchain pinned in rust-toolchain.toml.
COPY rust-toolchain.toml .
RUN rustup show active-toolchain || rustup toolchain install
COPY . .
RUN cargo build --release -p dedicated_server \
 && cp target/release/dedicated_server /dedicated_server

FROM debian:bookworm-slim
COPY --from=build /dedicated_server /app/dedicated_server
COPY data /app/data
COPY docker/entrypoint.sh /app/entrypoint.sh
# Everything the server writes (database, keys, service.toml, logs) lives
# in the volume.
WORKDIR /srv/5th-echelon
VOLUME /srv/5th-echelon
# 80 config + community API, 8000 content, 21126/21127 game (UDP), 50051 API.
EXPOSE 80/tcp 8000/tcp 21126/udp 21127/udp 50051/tcp
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s \
  CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' || exit 1
ENTRYPOINT ["/app/entrypoint.sh"]
