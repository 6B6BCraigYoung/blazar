fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        std::fs::create_dir_all(&dist).expect("建不了 crates/web/dist");
        std::fs::write(
            &index,
            "<!doctype html><meta charset=utf-8><title>Blazar</title>\
             <body style=\"background:#0c0c0d;color:#a1a1aa;font:14px sans-serif;padding:40px\">\
             界面还没构建：在 crates/web 下运行 <code>trunk build --release</code> 后重新编译 hub。",
        )
        .expect("写不了占位页");
    }
    println!("cargo:rerun-if-changed=../web/dist");
}
