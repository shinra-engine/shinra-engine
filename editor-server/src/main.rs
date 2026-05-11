use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use glam::{Mat4, Vec3};
use openh264::{
    encoder::{Encoder, EncoderConfig, FrameType},
    formats::YUVBuffer,
    OpenH264API,
};
use scene::Scene as SceneDoc;
use serde::Deserialize;
use shinra_engine::{
    engine::Engine,
    mesh::Mesh,
    scene::{orbit_eye, Camera, Projection, Scene as EngineScene},
};
use tokio::sync::{watch, RwLock};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 384;

/// (is_keyframe, h264_annexb_bytes)
type Frame = Arc<(bool, Vec<u8>)>;

#[derive(Clone)]
struct AppState {
    frame_rx: watch::Receiver<Frame>,
    scene: Arc<RwLock<SceneDoc>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let initial: Frame = Arc::new((true, Vec::new()));
    let (frame_tx, frame_rx) = watch::channel(initial);

    // wgpu is not Send across all backends — render on a dedicated OS thread.
    std::thread::spawn(move || {
        if let Err(e) = render_loop(frame_tx) {
            eprintln!("render_loop: {e}");
        }
    });

    let scene = Arc::new(RwLock::new(SceneDoc::default()));
    let state = AppState { frame_rx, scene };

    let http_app = Router::new()
        .route("/", get(index_html))
        .route("/scene", get(get_scene).post(post_scene))
        .route("/scene/save", post(save_scene))
        .route("/scene/load", post(load_scene))
        .with_state(state.clone());

    let ws_app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    let http_listener = tokio::net::TcpListener::bind("0.0.0.0:5812").await?;
    let ws_listener = tokio::net::TcpListener::bind("0.0.0.0:5813").await?;

    println!("editor-server: HTTP :5812  WS :5813");

    tokio::select! {
        r = axum::serve(http_listener, http_app) => r?,
        r = axum::serve(ws_listener, ws_app) => r?,
    }

    Ok(())
}

/// Convert RGBA pixels to a YUV I420 `YUVBuffer`.
///
/// Packs planes as [Y…][U…][V…] matching openh264's `from_vec` layout.
fn rgba_to_yuv(pixels: &[u8], width: u32, height: u32) -> YUVBuffer {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_size = y_size / 4;
    let mut yuv = vec![0u8; y_size + 2 * uv_size];

    for row in 0..h {
        for col in 0..w {
            let i = (row * w + col) * 4;
            let r = pixels[i] as f32;
            let g = pixels[i + 1] as f32;
            let b = pixels[i + 2] as f32;

            yuv[row * w + col] = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;

            if row % 2 == 0 && col % 2 == 0 {
                let uv_idx = (row / 2) * (w / 2) + col / 2;
                yuv[y_size + uv_idx] =
                    (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0) as u8;
                yuv[y_size + uv_size + uv_idx] =
                    (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0) as u8;
            }
        }
    }

    YUVBuffer::from_vec(yuv, w, h)
}

fn render_loop(tx: watch::Sender<Frame>) -> Result<()> {
    let mut engine = Engine::new(WIDTH, HEIGHT);

    let bunny: Arc<Mesh> = Arc::new(Mesh::from_obj_file("assets/bunny.obj")?);

    // Persistent readback buffer (CPU-visible, aligned rows).
    let unpadded_bpr = WIDTH * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let pad_bpr = unpadded_bpr.div_ceil(align) * align;
    let readback = engine.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("h264_readback"),
        size: (pad_bpr * HEIGHT) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let config = EncoderConfig::new()
        .set_bitrate_bps(2_000_000)
        .max_frame_rate(30.0);
    let mut encoder = Encoder::with_api_config(OpenH264API::from_source(), config)?;

    let start = Instant::now();
    let mut frame_idx: u32 = 0;

    loop {
        let t = start.elapsed().as_secs_f32();

        // Force a keyframe every ~1s so newly-connected WS clients can sync
        // (watch::channel retains only the latest frame).
        if frame_idx % 30 == 0 {
            encoder.force_intra_frame();
        }

        let camera = Camera {
            eye: orbit_eye(t * 0.5, 3.0, 1.5),
            target: Vec3::ZERO,
            up: Vec3::Y,
            projection: Projection::Perspective {
                fov_y_radians: std::f32::consts::PI / 3.0,
                aspect: WIDTH as f32 / HEIGHT as f32,
                znear: 0.1,
                zfar: 100.0,
            },
        };

        let mut scene = EngineScene::new(camera);
        scene.spawn_mesh(bunny.clone(), Mat4::IDENTITY);
        engine.render(&scene);

        // GPU → CPU readback.
        let mut enc_cmd = engine
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("h264_readback"),
            });
        enc_cmd.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &engine.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pad_bpr),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        engine.queue.submit([enc_cmd.finish()]);

        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = engine.device.poll(wgpu::PollType::wait_indefinitely());

        let pixels = {
            let data = slice.get_mapped_range();
            let mut buf = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
            for row in 0..HEIGHT as usize {
                let s = row * pad_bpr as usize;
                buf.extend_from_slice(&data[s..s + WIDTH as usize * 4]);
            }
            buf
        };
        readback.unmap();

        // RGBA → YUV I420 → H.264 annexb.
        let yuv = rgba_to_yuv(&pixels, WIDTH, HEIGHT);
        let bitstream = encoder.encode(&yuv)?;
        let is_key = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        let h264_bytes = bitstream.to_vec();

        let _ = tx.send(Arc::new((is_key, h264_bytes)));
        frame_idx = frame_idx.wrapping_add(1);

        // ~30 fps cap.
        std::thread::sleep(std::time::Duration::from_millis(33));
    }
}

#[derive(Deserialize)]
struct PathRequest {
    path: String,
}

async fn save_scene(
    State(state): State<AppState>,
    Json(req): Json<PathRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let scene = state.scene.read().await.clone();
    let ron_str = ron::ser::to_string_pretty(&scene, Default::default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Some(parent) = std::path::Path::new(&req.path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    std::fs::write(&req.path, ron_str)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn load_scene(
    State(state): State<AppState>,
    Json(req): Json<PathRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let ron_str =
        std::fs::read_to_string(&req.path).map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    let scene: SceneDoc =
        ron::from_str(&ron_str).map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    *state.scene.write().await = scene;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_scene(State(state): State<AppState>) -> Json<SceneDoc> {
    Json(state.scene.read().await.clone())
}

async fn post_scene(State(state): State<AppState>, Json(new_scene): Json<SceneDoc>) -> StatusCode {
    *state.scene.write().await = new_scene;
    StatusCode::NO_CONTENT
}

async fn index_html() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html>
<head><title>shinra editor</title></head>
<body style="margin:0;background:#111">
  <canvas id="c" width="512" height="384" style="display:block;width:100%;height:auto"></canvas>
  <script>
    const canvas = document.getElementById('c');
    const ctx = canvas.getContext('2d');
    const decoder = new VideoDecoder({
      output(frame) { ctx.drawImage(frame, 0, 0); frame.close(); },
      error(e) { console.warn('decoder:', e); },
    });
    decoder.configure({ codec: 'avc1.42E01E', codedWidth: 512, codedHeight: 384 });
    let synced = false;
    const ws = new WebSocket('ws://localhost:5813/ws');
    ws.binaryType = 'arraybuffer';
    ws.onmessage = ({ data }) => {
      const buf = new Uint8Array(data);
      const isKey = buf[0] === 1;
      if (!synced && !isKey) return;
      synced = true;
      decoder.decode(new EncodedVideoChunk({
        type: isKey ? 'key' : 'delta',
        timestamp: performance.now() * 1000,
        data: buf.subarray(1),
      }));
    };
  </script>
</body>
</html>"#,
    )
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws(socket, state.frame_rx))
}

async fn handle_ws(mut socket: WebSocket, mut frame_rx: watch::Receiver<Frame>) {
    loop {
        if frame_rx.changed().await.is_err() {
            break;
        }
        let frame = frame_rx.borrow_and_update().clone();
        let (is_key, h264) = frame.as_ref();

        if h264.is_empty() {
            continue;
        }

        let mut msg = Vec::with_capacity(1 + h264.len());
        msg.push(*is_key as u8);
        msg.extend_from_slice(h264);

        if socket.send(Message::Binary(msg)).await.is_err() {
            break;
        }
    }
}
