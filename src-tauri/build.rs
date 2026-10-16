fn main() {
    println!("cargo:rerun-if-env-changed=HPBAR_RELEASE_VERSION");
    tauri_build::build()
}
