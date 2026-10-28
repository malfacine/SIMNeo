pub mod file_dialog;
pub mod startup;

pub fn playback_smoke_path() -> String {
    format!(
        "D:/Server/apps/SimNEO/data/playback-ui-{}.mp4",
        std::process::id()
    )
}

pub fn p2p_library_override() -> Option<std::path::PathBuf> {
    std::env::var_os("SIMNEO_P2P_DLL").map(std::path::PathBuf::from)
}

pub fn find_p2p_library() -> std::io::Result<Option<std::path::PathBuf>> {
    let bundled = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("assets/libt2u.dll");
    let source = std::env::current_dir()?.join("assets/libt2u.dll");
    Ok([bundled, source].into_iter().find(|path| path.is_file()))
}

pub fn foreground_owner() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

pub fn local_date() -> (i32, i32, i32) {
    let mut now = unsafe { std::mem::zeroed() };
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut now);
    }
    (now.wYear as i32, now.wMonth as i32, now.wDay as i32)
}
