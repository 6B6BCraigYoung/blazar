fn main() {
    std::fs::create_dir_all("engine").expect("create the engine resource directory");
    tauri_build::build();
}
