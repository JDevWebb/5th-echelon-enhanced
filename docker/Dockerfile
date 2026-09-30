# The 5th Echelon server in a container.
#
#   docker compose -f docker/compose.yaml up -d      (from the repository root)
#
# Build context: the repository root. Base images are pinned by digest;
# update them together (docker buildx imagetools inspect <image>).

FROM rust:1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS build
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

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
# The server runs as this user; the entrypoint starts as root only to hand
# it the volume, then drops to it keeping nothing but binding port 80.
RUN useradd --system --uid 10001 --user-group --home-dir /srv/5th-echelon --no-create-home --shell /usr/sbin/nologin echelon \
 && install -d -o echelon -g echelon -m 750 /srv/5th-echelon
COPY --from=build /dedicated_server /app/dedicated_server
COPY data /app/data
COPY docker/entrypoint.sh /app/entrypoint.sh
# Everything the server writes (database, keys, service.toml, logs) lives
# in the volume.
WORKDIR /srv/5th-echelon
VOLUME /srv/5th-echelon
# 80 config + community API, 8000 content, 21126/21127 game (UDP),
# 21128/21129 internet play (UDP), 50051 API.
EXPOSE 80/tcp 8000/tcp 21126/udp 21127/udp 21128/udp 21129/udp 50051/tcp
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s \
  CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/50051' || exit 1
ENTRYPOINT ["/app/entrypoint.sh"]
