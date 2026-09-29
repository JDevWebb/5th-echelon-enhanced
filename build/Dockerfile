# Local build and test environment for 5th Echelon Enhanced (no CI).
#   Linux server build and workspace tests: cargo
#   Windows launcher and hooks DLL: cargo-xwin (MSVC targets, from Linux)
FROM rust:1-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler clang lld llvm \
    gcc-x86-64-linux-gnu libc6-dev-amd64-cross \
 && rm -rf /var/lib/apt/lists/*
COPY rust-toolchain.toml /tmp/tc/rust-toolchain.toml
RUN cd /tmp/tc && rustup show active-toolchain || rustup toolchain install \
 && rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc x86_64-unknown-linux-gnu \
 && rustup component add rust-src rustfmt
RUN cargo install --locked cargo-xwin
# winres expects rc.exe; see rc-wrapper.sh.
COPY build/rc-wrapper.sh /rc.exe
# Linux x86_64 server from any host (the node image is Debian bookworm too).
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
WORKDIR /src
