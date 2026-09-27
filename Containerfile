# The environment every callgrind count is taken in.
#
# Pinned by digest (multi-arch index: amd64 and arm64), with Debian packages
# from a dated snapshot, so the image rebuilds to the same valgrind and the
# same toolchain on any host. The Rust toolchain is the image's own 1.97.1,
# the version rust-toolchain.toml names.
FROM docker.io/library/rust:1.97.1-slim-trixie@sha256:8e8cf8f7fd54a2d23d5a743b3a03f56e26b6c774276c33fa0595111704ebb15c

RUN rm -f /etc/apt/sources.list.d/debian.sources \
 && echo 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260925T000000Z trixie main' \
      > /etc/apt/sources.list.d/snapshot.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends valgrind=1:3.24.0-3 \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy rustfmt llvm-tools-preview

# Builds happen inside the image (its glibc, not the host's) into a target
# directory of their own, so host and container artifacts never mix.
ENV CARGO_TARGET_DIR=/work/target/container \
    CARGO_TERM_COLOR=never
WORKDIR /work
