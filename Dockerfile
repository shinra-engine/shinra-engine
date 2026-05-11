# Dev-mode image: keeps the full Rust toolchain inside the container so the
# editor-server is built (and re-built) from source mounted at /engine-core.
# Use Dockerfile.release for a slim multi-stage runtime image.
# Rust MSRV is set by Cargo.lock — wgpu 27 needs 1.88, getrandom needs
# edition 2024 (stabilized in 1.85). 1.88-bookworm covers both.
FROM rust:1.88-bookworm

# - cmake / nasm / pkg-config: openh264 source build (transitive dep)
# - mesa-vulkan-drivers / libvulkan1: software Vulkan (lavapipe) so wgpu
#   renders without a physical GPU
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    nasm \
    pkg-config \
    mesa-vulkan-drivers \
    libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# /engine-core (source) and /game (project) are bind-mounted at runtime.
# The editor-server resolves asset paths relative to its current directory.
WORKDIR /game

EXPOSE 5812
EXPOSE 5813

# cargo builds into /engine-core/target/ on the bind mount, so the build
# persists across `docker compose up`. First build is slow; subsequent ones
# only touch crates whose source actually changed.
CMD ["cargo", "run", "--release", "--manifest-path", "/engine-core/Cargo.toml", "-p", "editor-server"]
