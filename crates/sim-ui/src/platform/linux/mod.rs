pub mod file_dialog;

pub fn playback_smoke_path() -> String {
    std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME definido no Linux"))
        .join(format!("playback-ui-{}.mp4", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

pub fn p2p_library_override() -> Option<std::path::PathBuf> {
    std::env::var_os("SIMNEO_P2P_LIBRARY").map(std::path::PathBuf::from)
}

pub fn find_p2p_library() -> std::io::Result<Option<std::path::PathBuf>> {
    let bundled = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("assets/libsim_t2u_linux.so");
    let source = std::env::current_dir()?.join("native/linux/t2u/build/libsim_t2u_linux.so");
    Ok([bundled, source].into_iter().find(|path| path.is_file()))
}

pub fn foreground_owner() -> isize {
    0
}

pub fn local_date() -> (i32, i32, i32) {
    use chrono::Datelike;
    let now = chrono::Local::now();
    (now.year(), now.month() as i32, now.day() as i32)
}
