//! Build script: bakes the app version from the single source of truth —
//! the VERSION file at the repo root — into ASTRID_VERSION, which lib.rs
//! reads with env!(). Cargo package versions must be three-segment semver,
//! so the four-segment phase version (0.1.3.3) can only travel through
//! this channel. Gradle parses the same file for versionName/versionCode,
//! keeping the Chat card, the About screen, and the APK identical.
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let version_file = manifest.join("../../VERSION");
    let version = fs::read_to_string(&version_file)
        .unwrap_or_else(|e| panic!("VERSION file missing at repo root ({}): {e}", version_file.display()))
        .trim()
        .to_string();
    assert!(
        !version.is_empty(),
        "VERSION file at repo root must contain major.minor.phase[.fix], got empty"
    );
    println!("cargo:rustc-env=ASTRID_VERSION={version}");
    println!("cargo:rerun-if-changed={}", version_file.display());
}
