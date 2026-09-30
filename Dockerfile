# One image, one stage. The toolchain stays in it so the engine is built *and*
# run from the same container: edit a crate, hit the command again, and only
# what changed recompiles. A two-stage release image would throw the toolchain
# away and make every engine edit a full rebuild, which is the opposite of what
# you want while working on the engine.
#
# Pinned to the toolchain this is developed against. Do not lower it to a
# guessed MSRV: the graphics stack pulls transitive crates that raise their
# own floor between patch releases (ordered-float wanted 1.90 the first time
# this image was built against 1.88), and finding that out is a container
# rebuild each time.
FROM rust:1.93-bookworm

# Game modules are wasm: se-cli builds the engine's `se` crate and every bundle
# for wasm32-unknown-unknown, so ide, shot and build-examples all need it.
RUN rustup target add wasm32-unknown-unknown

# mesa-vulkan-drivers + libvulkan1 give lavapipe: software Vulkan, so wgpu
# renders with no GPU and no /dev/dri. This is a supported configuration for
# this engine, not a fallback — it never opens a window.
# pkg-config/cmake cover native deps pulled in by the graphics stack.
RUN apt-get update && apt-get install -y --no-install-recommends \
      cmake \
      pkg-config \
      mesa-vulkan-drivers \
      libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

# Both are bind-mounted at run time; cargo builds into /engine/target on the
# mount, so the build cache survives `docker compose down` and an engine edit
# recompiles one crate rather than the world.
ENV CARGO_HOME=/cargo \
    SHINRA_ENGINE=/engine \
    CARGO_TERM_COLOR=always
WORKDIR /engine

# The IDE is a terminal program: it needs a tty, which compose grants.
CMD ["cargo", "run", "-q", "-p", "se-cli", "--", "run", "/examples"]
