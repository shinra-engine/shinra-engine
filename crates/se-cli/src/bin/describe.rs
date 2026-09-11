//! Print what a `.so` module registered, in the same shape the wasm route's
//! `describeModule` prints it. For comparing the two routes.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let m = se_host::Module::open(std::path::Path::new(&p)).unwrap();
    let has = |s: &[u8]| m.has(s);
    if has(se_abi::sym::LAYOUTS) {
        for l in unsafe { se_host::sink::layouts(&m) }.unwrap() {
            println!("layout {} size={} align={} hash={:#x}", l.name, l.size, l.align, l.hash);
            for f in &l.fields {
                println!("    {}: {:?}[{}] @{}", f.name, f.ty, f.count, f.offset);
            }
        }
    }
    if has(se_abi::sym::BUFFERS) {
        for b in unsafe { se_host::sink::buffers(&m) }.unwrap() {
            println!("buffer {} {:?} {}x{} {:?} n={} sampled={}",
                b.name, b.extent, b.width, b.height, b.format, b.count, b.sampled);
        }
    }
    if has(se_abi::sym::GRAPH) {
        for g in unsafe { se_host::sink::graphs(&m) }.unwrap() {
            println!("graph {} present={} passes={}", g.name, g.present, g.passes.len());
            for p in &g.passes {
                println!("    pass {} -> {:?} depth={:?} reads={:?} draw={:?} shader={}B",
                    p.name, p.color, p.depth, p.reads, p.draw.kind, p.shader.len());
            }
            for (a, b) in &g.edges { println!("    edge {a} -> {b}"); }
        }
    }
    if has(se_abi::sym::ASSETS) {
        for a in unsafe { se_host::sink::assets(&m) }.unwrap() {
            println!("asset {} {} bytes", a.name, a.bytes.len());
        }
    }
}
