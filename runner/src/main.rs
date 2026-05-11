use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};
use gametok_abi::{Drawable, InputFrame};
use glam::Vec3;
use libloading::{Library, Symbol};
use shinra_engine::{
    engine::Engine,
    input::{Key, Keymap},
    mesh::Mesh,
    presenter::{terminal::TerminalPresenter, FrameCtx, Presenter},
    scene::{Camera, Projection, Scene},
};
use std::{
    io::stdout,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Self {
        enable_raw_mode().expect("enable raw mode");
        execute!(stdout(), Hide).expect("hide cursor");
        Self
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), Show);
    }
}

fn make_camera(width: u32, height: u32) -> Camera {
    Camera {
        eye: Vec3::new(0.0, 2.0, 5.0),
        target: Vec3::ZERO,
        up: Vec3::Y,
        projection: Projection::Perspective {
            fov_y_radians: 45f32.to_radians(),
            aspect: width as f32 / height as f32,
            znear: 0.1,
            zfar: 100.0,
        },
    }
}

fn scan_so(dir: &str) -> Result<Vec<String>> {
    let mut out = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("so"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("libgame"))
                .unwrap_or(false)
        })
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    out.sort();
    Ok(out)
}

fn map_key(km: &mut Keymap, k: crossterm::event::KeyEvent) {
    let key = match k.code {
        KeyCode::Char('a') => Key::A,
        KeyCode::Char('d') => Key::D,
        KeyCode::Char('w') => Key::W,
        KeyCode::Char('s') => Key::S,
        KeyCode::Char('j') => Key::J,
        KeyCode::Char('k') => Key::K,
        KeyCode::Char('n') => Key::N,
        KeyCode::Char('q') => Key::Q,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Esc => Key::Esc,
        _ => return,
    };
    // Terminal raw mode delivers presses only — use `tap`, which contributes
    // to one frame and self-clears. The previous press+release pair drained
    // `held` before `frame()` could read it, so axis keys did nothing.
    km.tap(key);
}

fn main() -> Result<()> {
    let (cols, rows) = size().unwrap_or((80, 24));
    let width = (cols as u32 * 2).min(320);
    let height = (rows as u32 * 4).min(180);

    let mut engine = Engine::new(width, height);
    let mut presenter = TerminalPresenter::new(&engine.device, width, height);
    let mut keymap = Keymap::new();

    let games = scan_so("target/debug")?;
    if games.is_empty() {
        eprintln!("no libgame*.so in target/debug — build a game first");
        return Ok(());
    }

    let _guard = RawModeGuard::enter();

    let mut idx = 0;
    'outer: loop {
        let lib = unsafe { Library::new(&games[idx])? };
        let meshes_count: Symbol<unsafe extern "C" fn() -> u32> =
            unsafe { lib.get(b"meshes_count")? };
        let meshes_path: Symbol<unsafe extern "C" fn(u32, *mut u8, u32) -> u32> =
            unsafe { lib.get(b"meshes_path")? };
        let tick: Symbol<unsafe extern "C" fn(f32, *const InputFrame)> =
            unsafe { lib.get(b"tick")? };
        let drawables_ptr: Symbol<unsafe extern "C" fn() -> *const Drawable> =
            unsafe { lib.get(b"drawables_ptr")? };
        let drawables_len: Symbol<unsafe extern "C" fn() -> u32> =
            unsafe { lib.get(b"drawables_len")? };

        let mut meshes: Vec<Arc<Mesh>> = Vec::new();
        let n = unsafe { meshes_count() };
        for i in 0..n {
            let mut buf = [0u8; 256];
            let len = unsafe { meshes_path(i, buf.as_mut_ptr(), buf.len() as u32) } as usize;
            let path = std::str::from_utf8(&buf[..len])?;
            meshes.push(Arc::new(Mesh::from_obj_file(path)?));
        }

        let frame_dur = Duration::from_millis(33);
        loop {
            let frame_start = Instant::now();

            while crossterm::event::poll(Duration::from_millis(0))? {
                if let Event::Key(k) = crossterm::event::read()? {
                    map_key(&mut keymap, k);
                }
            }
            if keymap.take_quit() {
                break 'outer;
            }
            if keymap.take_swipe_next() {
                break;
            }

            let input = keymap.frame();
            unsafe { tick(0.033, &input as *const _) };

            let n_d = unsafe { drawables_len() } as usize;
            let p = unsafe { drawables_ptr() };
            let drawables: &[Drawable] = if p.is_null() || n_d == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(p, n_d) }
            };

            let mut scene = Scene::new(make_camera(width, height));
            for d in drawables {
                let mesh = meshes[d.mesh_id as usize].clone();
                let model = glam::Mat4::from_cols_array(&d.model);
                scene.spawn_mesh(mesh, model);
            }

            engine.render(&scene);
            let mut ctx = FrameCtx {
                device: &engine.device,
                queue: &engine.queue,
                texture: &engine.color,
                width: engine.size.0,
                height: engine.size.1,
            };
            presenter.present(&mut ctx);

            if let Some(dt) = frame_dur.checked_sub(frame_start.elapsed()) {
                thread::sleep(dt);
            }
        }

        // 'n' was pressed — drop lib (unload), advance to next game.
        drop(lib);
        idx = (idx + 1) % games.len();
    }

    Ok(())
}
