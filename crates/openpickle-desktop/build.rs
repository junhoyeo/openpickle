fn main() {
    println!("cargo:rerun-if-changed=macos/asweb.h");
    println!("cargo:rerun-if-changed=macos/asweb.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("macos/asweb.m")
            .include("macos")
            .flag("-fobjc-arc")
            .compile("openpickle_asweb");
        for framework in ["AuthenticationServices", "AppKit", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
