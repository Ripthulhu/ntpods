use std::path::PathBuf;

fn main() {
    // The AAC-ELD decoder (FFmpeg) is only used on Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ff = manifest.join("vendor/ffmpeg");

    // Compile the tiny C shim against the vendored FFmpeg headers.
    cc::Build::new()
        .file("src/eld_shim.c")
        .include(ff.join("include"))
        .compile("eld_shim");

    if std::env::var_os("CARGO_FEATURE_EXPERIMENTAL_PLAYBACK").is_some() {
        cc::Build::new()
            .file("src/playback/aac_shim.c")
            .std("c11")
            .include(ff.join("include"))
            .compile("aac_shim");
    }

    // Link the FFmpeg import libs (avcodec pulls avutil + swresample).
    println!(
        "cargo:rustc-link-search=native={}",
        ff.join("lib").display()
    );
    println!("cargo:rustc-link-lib=avcodec");
    println!("cargo:rustc-link-lib=avutil");
    println!("cargo:rustc-link-lib=swresample");
    println!("cargo:rerun-if-changed=src/eld_shim.c");
    println!("cargo:rerun-if-changed=src/playback/aac_shim.c");
}
