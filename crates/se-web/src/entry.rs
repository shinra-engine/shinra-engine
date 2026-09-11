//! What JS calls to start the engine.

use wasm_bindgen::prelude::*;

/// Install a panic hook so a Rust panic reaches the browser console as
/// something readable rather than `unreachable executed`.
#[wasm_bindgen(start)]
pub fn boot() {
    console_error_panic_hook::set_once();
}

/// The engine's memory and function table, for JS to hand to every module it
/// instantiates.
///
/// This is the linking step: a module built with `--import-memory` asks for
/// `env.memory`, and giving it this one is what puts the module and the engine
/// in a single address space.
///
/// A control module additionally imports `se_ctl_spawn`, `se_ctl_set`, … by
/// name, so those are bound here too. A module that imports one and is handed
/// nothing is a link failure, which is why this set is the whole set and not a
/// convenient subset.
#[wasm_bindgen(js_name = engineImports)]
pub fn engine_imports() -> js_sys::Object {
    thread_local! {
        /// Built once. The `se_ctl_*` closures are leaked into it, so building
        /// it per instantiation would leak one set per module.
        static ENV: js_sys::Object = {
            let env = js_sys::Object::new();
            let memory = wasm_bindgen::memory();
            let _ = js_sys::Reflect::set(&env, &JsValue::from_str("memory"), &memory);
            let table = wasm_bindgen::function_table();
            let _ =
                js_sys::Reflect::set(&env, &JsValue::from_str("__indirect_function_table"), &table);
            crate::ctl::imports(&env);
            env
        };
    }
    let imports = js_sys::Object::new();
    ENV.with(|env| {
        let _ = js_sys::Reflect::set(&imports, &JsValue::from_str("env"), env);
    });
    imports
}

/// Read one instantiated module's registrations and describe them.
///
/// JS owns instantiation — it holds the fetch and the imports object — and
/// hands the exports over. From here on the code is the engine's own: the same
/// `LayoutDef`s and `BufferDef`s the `.so` route builds, from the same source.
#[wasm_bindgen(js_name = describeModule)]
pub fn describe_module(name: String, exports: js_sys::Object) -> Result<String, JsValue> {
    let m = crate::module::WebModule::new(name, exports);
    let r = m.read().map_err(|e| JsValue::from_str(&format!("{e:#}")))?;

    let mut out = String::new();
    for l in &r.layouts {
        out.push_str(&format!("layout {} size={} align={} hash={:#x}\n", l.name, l.size, l.align, l.hash));
        for f in &l.fields {
            out.push_str(&format!("    {}: {:?}[{}] @{}\n", f.name, f.ty, f.count, f.offset));
        }
    }
    for b in &r.buffers {
        out.push_str(&format!(
            "buffer {} {:?} {}x{} {:?} n={} sampled={}\n",
            b.name, b.extent, b.width, b.height, b.format, b.count, b.sampled
        ));
    }
    for g in &r.graphs {
        out.push_str(&format!("graph {} present={} passes={}\n", g.name, g.present, g.passes.len()));
        for p in &g.passes {
            out.push_str(&format!(
                "    pass {} -> {:?} depth={:?} reads={:?} draw={:?} shader={}B\n",
                p.name, p.color, p.depth, p.reads, p.draw.kind, p.shader.len()
            ));
        }
        for (a, b) in &g.edges {
            out.push_str(&format!("    edge {a} -> {b}\n"));
        }
    }
    for a in &r.assets {
        out.push_str(&format!("asset {} {} bytes\n", a.name, a.bytes.len()));
    }
    if out.is_empty() {
        out.push_str("(registered nothing)\n");
    }
    Ok(out)
}

/// Turn the two parallel arrays JS hands over into what `render::collect`
/// takes. Names and exports are separate because they arrive separately:
/// the names come out of `modules.json`, the exports out of instantiation.
pub(crate) fn pair(
    names: &js_sys::Array,
    exports: &js_sys::Array,
) -> Result<(Vec<String>, Vec<js_sys::Object>), JsValue> {
    if names.length() != exports.length() {
        return Err(JsValue::from_str("names and exports are different lengths"));
    }
    let n: Vec<String> = names.iter().map(|v| v.as_string().unwrap_or_default()).collect();
    let e: Vec<js_sys::Object> = exports.iter().map(js_sys::Object::from).collect();
    Ok((n, e))
}

/// Everything a frame is except the device.
///
/// The point of the split is that this runs where there is no GPU — under
/// `wasm-bindgen --target nodejs`, say — so the part of the frame that is
/// description rather than execution can be checked outside a browser.
#[wasm_bindgen(js_name = planGame)]
pub fn plan_game(
    names: js_sys::Array,
    exports: js_sys::Array,
    width: u32,
    height: u32,
) -> Result<String, JsValue> {
    let (names, exports) = pair(&names, &exports)?;
    let loaded = crate::render::collect(&names, &exports).map_err(err)?;
    loaded.plan((width.max(1), height.max(1))).map_err(err)
}

pub(crate) fn err(e: anyhow::Error) -> JsValue {
    JsValue::from_str(&format!("{e:#}"))
}
