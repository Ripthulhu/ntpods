use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ff = manifest.join("vendor/ffmpeg");

    cc::Build::new()
        .file("src/eld_shim.c")
        .include(ff.join("include"))
        .compile("eld_shim");

    println!(
        "cargo:rustc-link-search=native={}",
        ff.join("lib").display()
    );
    println!("cargo:rustc-link-lib=avcodec");
    println!("cargo:rustc-link-lib=avutil");
    println!("cargo:rustc-link-lib=swresample");
    println!("cargo:rerun-if-changed=src/eld_shim.c");
}
