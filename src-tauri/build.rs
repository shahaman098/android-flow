use std::path::PathBuf;
use std::process::Command;

fn main() {
    #[cfg(target_os = "macos")]
    link_swift_runtime();

    tauri_build::build()
}

/// Put the Swift compatibility shims on the linker's search path.
///
/// `screencapturekit` (via `apple-metal`) is a Swift-backed crate, so the final binary
/// references `__swift_FORCE_LOAD_$_swiftCompatibility56` and friends. Its build script
/// only adds `$DEVELOPER_DIR/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`,
/// which does not exist when the machine has Command Line Tools instead of full Xcode —
/// there the archives live in `$DEVELOPER_DIR/usr/lib/swift/macosx`. Without this the
/// link fails outright and the app cannot be built, even though `cargo check` passes
/// because checking never links.
#[cfg(target_os = "macos")]
fn link_swift_runtime() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");

    // Some Swift-backed dependencies use @rpath for the concurrency runtime.
    // Keep both the macOS runtime and bundled Frameworks paths explicit so a
    // release cannot pass linking and then die in dyld at launch.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    let Some(developer_dir) = developer_dir() else {
        // No toolchain located — let the linker try on its own and report the real error.
        return;
    };

    for candidate in [
        developer_dir.join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx"),
        developer_dir.join("usr/lib/swift/macosx"),
    ] {
        if candidate.join("libswiftCompatibility56.a").exists() {
            println!("cargo:rustc-link-search=native={}", candidate.display());
            return;
        }
    }
}

#[cfg(target_os = "macos")]
fn developer_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DEVELOPER_DIR") {
        return Some(PathBuf::from(dir));
    }
    let output = Command::new("xcode-select").arg("-p").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}
