use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use glam::{Mat4, Quat, Vec3};
use openh264::{
    encoder::{Encoder, EncoderConfig, FrameType},
    formats::YUVBuffer,
    OpenH264API,
};
use scene::{Camera as SceneCamera, Projection as SceneProjection, Scene as SceneDoc};
use serde::Serialize;
use shinra_engine::{
    engine::Engine,
    mesh::Mesh,
    scene::{Camera as EngineCamera, Projection as EngineProjection, Scene as EngineScene},
};
use tokio::sync::watch;

const WIDTH: u32 = 512;
const HEIGHT: u32 = 384;

/// (is_keyframe, h264_annexb_bytes)
type Frame = Arc<(bool, Vec<u8>)>;

#[derive(Clone)]
struct GameSlot {
    name: String,
    dir: PathBuf,
}

struct GameState {
    games: Vec<GameSlot>,
    index: usize,
    scene: SceneDoc,
}

#[derive(Serialize)]
struct GameInfo {
    name: String,
    index: usize,
    total: usize,
}

#[derive(Clone)]
struct AppState {
    game_frame_rx: watch::Receiver<Frame>,
    editor_frame_rx: watch::Receiver<Frame>,
    game: Arc<RwLock<GameState>>,
}

fn load_game(slot: &GameSlot) -> Result<SceneDoc> {
    let path = slot.dir.join("scene.ron");
    let s = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let scene: SceneDoc =
        ron::from_str(&s).with_context(|| format!("parse {}", path.display()))?;
    Ok(scene)
}

fn scan_games(dir: &str) -> Result<Vec<GameSlot>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("scan {dir}"))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("scene.ron").exists() {
            continue;
        }
        out.push(GameSlot {
            name: entry.file_name().to_string_lossy().into_owned(),
            dir: path,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    let games_dir = std::env::var("GAMES_DIR").unwrap_or_else(|_| "assets/games".into());
    let games = scan_games(&games_dir)?;
    if games.is_empty() {
        anyhow::bail!("no games found under {games_dir} (expected gameN/scene.ron)");
    }
    println!(
        "editor-server: found {} game(s): {}",
        games.len(),
        games
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let scene = load_game(&games[0])?;
    println!("editor-server: starting on game '{}'", games[0].name);

    let blank: Frame = Arc::new((true, Vec::new()));
    let (game_frame_tx, game_frame_rx) = watch::channel(blank.clone());
    let (editor_frame_tx, editor_frame_rx) = watch::channel(blank);

    let game = Arc::new(RwLock::new(GameState {
        games,
        index: 0,
        scene,
    }));

    // Game render thread — uses each scene's own camera (or the fallback).
    {
        let g = Arc::clone(&game);
        std::thread::spawn(move || {
            if let Err(e) = render_loop(game_frame_tx, g, CameraSource::FromScene) {
                eprintln!("game render_loop: {e}");
            }
        });
    }

    // Editor render thread — uses a fixed perspective overview camera.
    {
        let g = Arc::clone(&game);
        std::thread::spawn(move || {
            if let Err(e) = render_loop(editor_frame_tx, g, CameraSource::Editor) {
                eprintln!("editor render_loop: {e}");
            }
        });
    }

    let state = AppState {
        game_frame_rx,
        editor_frame_rx,
        game,
    };

    let http_app = Router::new()
        .route("/", get(index_html))
        .route("/scene", get(get_scene))
        .route("/game", get(get_game))
        .with_state(state.clone());

    let game_ws_app = Router::new()
        .route("/ws", get(ws_handler_game))
        .with_state(state.clone());

    let editor_ws_app = Router::new()
        .route("/ws", get(ws_handler_editor))
        .with_state(state.clone());

    let http_listener = tokio::net::TcpListener::bind("0.0.0.0:5812").await?;
    let game_listener = tokio::net::TcpListener::bind("0.0.0.0:5813").await?;
    let editor_listener = tokio::net::TcpListener::bind("0.0.0.0:5814").await?;

    println!("editor-server: HTTP :5812  GAME WS :5813  EDITOR WS :5814");

    tokio::select! {
        r = axum::serve(http_listener,   http_app)        => r?,
        r = axum::serve(game_listener,   game_ws_app)     => r?,
        r = axum::serve(editor_listener, editor_ws_app)   => r?,
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum CameraSource {
    FromScene,
    Editor,
}

/// Default camera used when the scene has no camera set.
fn fallback_scene_camera() -> SceneCamera {
    SceneCamera {
        eye: [3.0, 3.0, 3.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        projection: SceneProjection::Perspective {
            fov_y_degrees: 60.0,
            aspect: WIDTH as f32 / HEIGHT as f32,
            znear: 0.1,
            zfar: 100.0,
        },
    }
}

/// Fixed editor overview camera. Independent of the game's camera so the
/// editor viewport always shows a consistent vantage no matter what the
/// scene declares.
fn editor_camera() -> SceneCamera {
    SceneCamera {
        eye: [3.0, 3.0, 3.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        projection: SceneProjection::Perspective {
            fov_y_degrees: 50.0,
            aspect: WIDTH as f32 / HEIGHT as f32,
            znear: 0.05,
            zfar: 100.0,
        },
    }
}

fn to_engine_camera(cam: &SceneCamera) -> EngineCamera {
    let projection = match &cam.projection {
        SceneProjection::Perspective {
            fov_y_degrees,
            aspect,
            znear,
            zfar,
        } => EngineProjection::Perspective {
            fov_y_radians: fov_y_degrees.to_radians(),
            aspect: *aspect,
            znear: *znear,
            zfar: *zfar,
        },
        SceneProjection::Orthographic {
            half_height,
            aspect,
            znear,
            zfar,
        } => EngineProjection::Orthographic {
            half_height: *half_height,
            aspect: *aspect,
            znear: *znear,
            zfar: *zfar,
        },
    };
    EngineCamera {
        eye: Vec3::from(cam.eye),
        target: Vec3::from(cam.target),
        up: Vec3::from(cam.up),
        projection,
    }
}

fn build_engine_scene(
    doc: &SceneDoc,
    camera: &SceneCamera,
    mesh_cache: &mut HashMap<String, Arc<Mesh>>,
    quad_mesh: &mut Option<Arc<Mesh>>,
) -> EngineScene {
    let mut sc = EngineScene::new(to_engine_camera(camera));

    for node in &doc.nodes {
        if let Some(tilemap) = &node.tilemap {
            if quad_mesh.is_none() {
                match Mesh::from_obj_file("assets/quad.obj") {
                    Ok(m) => *quad_mesh = Some(Arc::new(m)),
                    Err(e) => eprintln!(
                        "editor-server: assets/quad.obj missing — tilemaps not rendered: {e}"
                    ),
                }
            }
            if let Some(quad) = quad_mesh.as_ref() {
                for cell in &tilemap.cells {
                    let model = Mat4::from_translation(Vec3::new(
                        cell.x as f32 * tilemap.tile_size[0],
                        0.0,
                        cell.y as f32 * tilemap.tile_size[1],
                    ));
                    sc.spawn_mesh(Arc::clone(quad), model);
                }
            }
        }

        if let Some(mesh_ref) = &node.mesh {
            if !mesh_cache.contains_key(&mesh_ref.path) {
                match Mesh::from_obj_file(&mesh_ref.path) {
                    Ok(m) => {
                        mesh_cache.insert(mesh_ref.path.clone(), Arc::new(m));
                    }
                    Err(e) => eprintln!("editor-server: cannot load {}: {e}", mesh_ref.path),
                }
            }
            if let Some(mesh) = mesh_cache.get(&mesh_ref.path) {
                let t = &node.transform;
                let model = Mat4::from_scale_rotation_translation(
                    Vec3::from(t.scale),
                    Quat::from_array(t.rotation),
                    Vec3::from(t.translation),
                );
                sc.spawn_mesh(Arc::clone(mesh), model);
            }
        }
    }

    sc
}

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

fn render_loop(
    tx: watch::Sender<Frame>,
    game: Arc<RwLock<GameState>>,
    cam_src: CameraSource,
) -> Result<()> {
    let mut engine = Engine::new(WIDTH, HEIGHT);

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

    let mut mesh_cache: HashMap<String, Arc<Mesh>> = HashMap::new();
    let mut quad_mesh: Option<Arc<Mesh>> = None;
    let mut frame_idx: u32 = 0;

    let editor_cam = editor_camera();
    let fallback_cam = fallback_scene_camera();

    loop {
        if frame_idx % 30 == 0 {
            encoder.force_intra_frame();
        }

        let scene = {
            let g = game.read().unwrap();
            let cam = match cam_src {
                CameraSource::Editor => &editor_cam,
                CameraSource::FromScene => g.scene.camera.as_ref().unwrap_or(&fallback_cam),
            };
            build_engine_scene(&g.scene, cam, &mut mesh_cache, &mut quad_mesh)
        };
        engine.render(&scene);

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

        let yuv = rgba_to_yuv(&pixels, WIDTH, HEIGHT);
        let bitstream = encoder.encode(&yuv)?;
        let is_key = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        let h264_bytes = bitstream.to_vec();

        let _ = tx.send(Arc::new((is_key, h264_bytes)));
        frame_idx = frame_idx.wrapping_add(1);

        std::thread::sleep(std::time::Duration::from_millis(33));
    }
}

async fn get_scene(State(state): State<AppState>) -> Json<SceneDoc> {
    Json(state.game.read().unwrap().scene.clone())
}

async fn get_game(State(state): State<AppState>) -> Json<GameInfo> {
    let g = state.game.read().unwrap();
    Json(GameInfo {
        name: g.games[g.index].name.clone(),
        index: g.index,
        total: g.games.len(),
    })
}

async fn index_html() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html>
<head><title>shinra editor</title></head>
<body style="margin:0;background:#111;color:#888;font-family:monospace">
  <div style="display:flex;gap:8px;padding:8px">
    <div>
      <p>game viewport (5813)</p>
      <canvas id="g" width="512" height="384" style="display:block"></canvas>
    </div>
    <div>
      <p>editor viewport (5814)</p>
      <canvas id="e" width="512" height="384" style="display:block"></canvas>
    </div>
  </div>
  <p style="padding:6px">arrow keys: move object &middot; n: next game</p>
  <script>
    function attach(canvasId, port) {
      const ctx = document.getElementById(canvasId).getContext('2d');
      const decoder = new VideoDecoder({
        output(frame) { ctx.drawImage(frame, 0, 0); frame.close(); },
        error(e) { console.warn('decoder', port, e); },
      });
      decoder.configure({ codec: 'avc1.42E01E', codedWidth: 512, codedHeight: 384 });
      let synced = false;
      const ws = new WebSocket(`ws://localhost:${port}/ws`);
      ws.binaryType = 'arraybuffer';
      ws.onmessage = ({ data }) => {
        if (typeof data === 'string') return;
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
      return ws;
    }
    const wsGame = attach('g', 5813);
    attach('e', 5814);
    document.addEventListener('keydown', e => {
      if (wsGame.readyState === WebSocket.OPEN) {
        wsGame.send(JSON.stringify({ type: 'keydown', key: e.key }));
      }
    });
  </script>
</body>
</html>"#,
    )
}

async fn ws_handler_game(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    let rx = state.game_frame_rx.clone();
    ws.on_upgrade(|socket| handle_ws(socket, state, rx))
}

async fn ws_handler_editor(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rx = state.editor_frame_rx.clone();
    ws.on_upgrade(|socket| handle_ws(socket, state, rx))
}

async fn handle_ws(mut socket: WebSocket, state: AppState, mut frame_rx: watch::Receiver<Frame>) {
    loop {
        tokio::select! {
            r = frame_rx.changed() => {
                if r.is_err() { break; }
                let frame = frame_rx.borrow_and_update().clone();
                let (is_key, h264) = frame.as_ref();
                if h264.is_empty() { continue; }
                let mut msg = Vec::with_capacity(1 + h264.len());
                msg.push(*is_key as u8);
                msg.extend_from_slice(h264);
                if socket.send(Message::Binary(msg)).await.is_err() { break; }
            }
            inbound = socket.recv() => {
                match inbound {
                    Some(Ok(Message::Text(t))) => handle_input(&t, &state.game),
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
        }
    }
}

fn handle_input(text: &str, game: &Arc<RwLock<GameState>>) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if v.get("type").and_then(|t| t.as_str()) != Some("keydown") {
        return;
    }
    let key = v.get("key").and_then(|k| k.as_str()).unwrap_or("");

    let mut g = game.write().unwrap();

    match key {
        "n" | "N" => {
            if g.games.len() < 2 {
                return;
            }
            let next = (g.index + 1) % g.games.len();
            let slot = g.games[next].clone();
            match load_game(&slot) {
                Ok(scene) => {
                    g.index = next;
                    g.scene = scene;
                    println!("editor-server: switched to game '{}'", slot.name);
                }
                Err(e) => eprintln!("editor-server: failed to load game '{}': {e}", slot.name),
            }
        }
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
            // Step is 5% of camera-target distance for the SCENE camera (or
            // the fallback if absent), so movement feels right at the scale
            // the game itself defines.
            let fallback = fallback_scene_camera();
            let cam = g.scene.camera.as_ref().unwrap_or(&fallback);
            let dist = ((cam.eye[0] - cam.target[0]).powi(2)
                + (cam.eye[1] - cam.target[1]).powi(2)
                + (cam.eye[2] - cam.target[2]).powi(2))
            .sqrt();
            let d = dist * 0.05;
            let (dx, dy) = match key {
                "ArrowLeft" => (-d, 0.0),
                "ArrowRight" => (d, 0.0),
                "ArrowUp" => (0.0, d),
                "ArrowDown" => (0.0, -d),
                _ => (0.0, 0.0),
            };
            if let Some(node) = g.scene.nodes.first_mut() {
                node.transform.translation[0] += dx;
                node.transform.translation[1] += dy;
            }
        }
        _ => {}
    }
}
