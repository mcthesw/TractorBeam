fn main() {
    if std::env::var("TARGET").as_deref() != Ok("i686-pc-windows-gnullvm") {
        return;
    }

    let def = std::path::Path::new("winmm.def")
        .canonicalize()
        .expect("winmm.def");
    println!("cargo:rustc-cdylib-link-arg={}", def.display());
    println!("cargo:rerun-if-changed=winmm.def");
}
