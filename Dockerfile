# ---- build stage ----
FROM rust:1.78-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    nasm \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build -p editor-server --release

# ---- runtime stage ----
FROM debian:bookworm-slim AS runtime

# mesa-vulkan-drivers provides llvmpipe (software Vulkan) so wgpu can render
# headlessly without a physical GPU.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libstdc++6 \
    mesa-vulkan-drivers \
    libvulkan1 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/editor-server /usr/local/bin/editor-server

EXPOSE 5812
EXPOSE 5813

CMD ["editor-server"]
