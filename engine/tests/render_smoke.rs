use shinra_engine::{
    engine::Engine,
    mesh::{Mesh, Vertex},
    scene::{Camera, Projection, Scene},
};
use std::sync::Arc;

fn cube_mesh() -> Mesh {
    let vertices = vec![
        // +X face
        Vertex {
            position: [0.5, -0.5, -0.5],
            normal: [1.0, 0.0, 0.0],
        },
        Vertex {
            position: [0.5, 0.5, -0.5],
            normal: [1.0, 0.0, 0.0],
        },
        Vertex {
            position: [0.5, 0.5, 0.5],
            normal: [1.0, 0.0, 0.0],
        },
        Vertex {
            position: [0.5, -0.5, 0.5],
            normal: [1.0, 0.0, 0.0],
        },
        // -X face
        Vertex {
            position: [-0.5, -0.5, 0.5],
            normal: [-1.0, 0.0, 0.0],
        },
        Vertex {
            position: [-0.5, 0.5, 0.5],
            normal: [-1.0, 0.0, 0.0],
        },
        Vertex {
            position: [-0.5, 0.5, -0.5],
            normal: [-1.0, 0.0, 0.0],
        },
        Vertex {
            position: [-0.5, -0.5, -0.5],
            normal: [-1.0, 0.0, 0.0],
        },
        // +Y face
        Vertex {
            position: [-0.5, 0.5, -0.5],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            position: [-0.5, 0.5, 0.5],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            position: [0.5, 0.5, 0.5],
            normal: [0.0, 1.0, 0.0],
        },
        Vertex {
            position: [0.5, 0.5, -0.5],
            normal: [0.0, 1.0, 0.0],
        },
        // -Y face
        Vertex {
            position: [-0.5, -0.5, 0.5],
            normal: [0.0, -1.0, 0.0],
        },
        Vertex {
            position: [-0.5, -0.5, -0.5],
            normal: [0.0, -1.0, 0.0],
        },
        Vertex {
            position: [0.5, -0.5, -0.5],
            normal: [0.0, -1.0, 0.0],
        },
        Vertex {
            position: [0.5, -0.5, 0.5],
            normal: [0.0, -1.0, 0.0],
        },
        // +Z face
        Vertex {
            position: [-0.5, -0.5, 0.5],
            normal: [0.0, 0.0, 1.0],
        },
        Vertex {
            position: [0.5, -0.5, 0.5],
            normal: [0.0, 0.0, 1.0],
        },
        Vertex {
            position: [0.5, 0.5, 0.5],
            normal: [0.0, 0.0, 1.0],
        },
        Vertex {
            position: [-0.5, 0.5, 0.5],
            normal: [0.0, 0.0, 1.0],
        },
        // -Z face
        Vertex {
            position: [0.5, -0.5, -0.5],
            normal: [0.0, 0.0, -1.0],
        },
        Vertex {
            position: [-0.5, -0.5, -0.5],
            normal: [0.0, 0.0, -1.0],
        },
        Vertex {
            position: [-0.5, 0.5, -0.5],
            normal: [0.0, 0.0, -1.0],
        },
        Vertex {
            position: [0.5, 0.5, -0.5],
            normal: [0.0, 0.0, -1.0],
        },
    ];
    let indices: Vec<u32> = (0..6u32)
        .flat_map(|f| {
            let b = f * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect();
    Mesh { vertices, indices }
}

fn perspective(eye: [f32; 3], target: [f32; 3], znear: f32, zfar: f32) -> Camera {
    Camera {
        eye: glam::Vec3::from(eye),
        target: glam::Vec3::from(target),
        up: glam::Vec3::Y,
        projection: Projection::Perspective {
            fov_y_radians: std::f32::consts::PI / 3.0,
            aspect: 256.0 / 144.0,
            znear,
            zfar,
        },
    }
}

fn readback(engine: &Engine) -> Vec<u8> {
    let (w, h) = engine.size;
    let bytes_per_row =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf_size = (bytes_per_row * h) as u64;

    let staging = engine.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: buf_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = engine
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &engine.color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    engine.queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = engine.device.poll(wgpu::PollType::wait_indefinitely());

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let start = row * bytes_per_row as usize;
        pixels.extend_from_slice(&mapped[start..start + (w * 4) as usize]);
    }
    drop(mapped);
    staging.unmap();
    pixels
}

fn save_png(pixels: &[u8], width: u32, height: u32, name: &str) {
    let dir = std::path::Path::new("target/debug/smoke");
    std::fs::create_dir_all(dir).expect("create smoke dir");
    let img = image::RgbaImage::from_raw(width, height, pixels.to_vec())
        .expect("failed to create RgbaImage");
    img.save(dir.join(format!("{name}.png")))
        .expect("failed to save PNG");
}

fn has_rasterized_pixel(pixels: &[u8]) -> bool {
    // Rendered Lambert shading (base 0.85 × min factor 0.15 in linear → sRGB)
    // produces R values well above 80; clear color (0.05 linear) maps to ~45/255.
    pixels.chunks(4).any(|p| p[0] > 80)
}

#[test]
fn render_smoke() {
    let mut engine = Engine::new(256, 144);

    // --- cube ---
    {
        let mesh = Arc::new(cube_mesh());
        let mut scene = Scene::new(perspective([2.0, 2.0, 2.0], [0.0, 0.0, 0.0], 0.1, 100.0));
        scene.spawn_mesh(mesh, glam::Mat4::IDENTITY);
        engine.render(&scene);
        let pixels = readback(&engine);
        save_png(&pixels, 256, 144, "cube");
        assert!(
            has_rasterized_pixel(&pixels),
            "cube: nothing was rasterized"
        );
    }

    // --- teapot ---
    {
        let mesh = Arc::new(Mesh::from_obj_file("../assets/teapot.obj").expect("load teapot"));
        let mut scene = Scene::new(perspective([0.0, 1.5, 4.0], [0.0, 1.0, 0.0], 0.1, 100.0));
        scene.spawn_mesh(mesh, glam::Mat4::IDENTITY);
        engine.render(&scene);
        let pixels = readback(&engine);
        save_png(&pixels, 256, 144, "teapot");
        assert!(
            has_rasterized_pixel(&pixels),
            "teapot: nothing was rasterized"
        );
    }

    // --- bunny ---
    {
        let mesh = Arc::new(Mesh::from_obj_file("../assets/bunny.obj").expect("load bunny"));
        let mut scene = Scene::new(perspective(
            [-0.05, 0.12, 0.25],
            [-0.05, 0.10, 0.0],
            0.01,
            10.0,
        ));
        scene.spawn_mesh(mesh, glam::Mat4::IDENTITY);
        engine.render(&scene);
        let pixels = readback(&engine);
        save_png(&pixels, 256, 144, "bunny");
        assert!(
            has_rasterized_pixel(&pixels),
            "bunny: nothing was rasterized"
        );
    }
}
