fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app.ico")
            .set("ProductName", "爱呵呵 · 听记")
            .set("FileDescription", "爱呵呵 · 听记 — 音视频知识整理")
            .set("CompanyName", "爱呵呵")
            .set("LegalCopyright", "Copyright © 2026 爱呵呵")
            .compile()
            .expect("failed to compile Windows icon resources");
    }
}
