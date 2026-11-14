fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        panic!("Frosty is a native macOS application and can only be built for macOS");
    }

    println!("cargo:rustc-link-arg=-mmacosx-version-min=13.0");
}
