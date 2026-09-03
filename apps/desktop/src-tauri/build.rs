use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=FREE_WHISPER_MANIFEST_SCOPE");
    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must provide a package version");
    let inferred_scope = if version.contains("-alpha.") {
        "alpha"
    } else {
        "stable"
    };
    let scope =
        env::var("FREE_WHISPER_MANIFEST_SCOPE").unwrap_or_else(|_| inferred_scope.to_owned());
    if scope != inferred_scope {
        panic!(
            "FREE_WHISPER_MANIFEST_SCOPE={scope} conflicts with package version {version}; Alpha packages require alpha and stable packages require stable"
        );
    }
    println!("cargo:rustc-env=FREE_WHISPER_MANIFEST_SCOPE={scope}");
    tauri_build::build()
}
