use std::path::PathBuf;

pub fn save_mp4(owner: isize, suggested: &str) -> Result<Option<PathBuf>, &'static str> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut path = vec![0u16; 32768];
    let name: Vec<u16> = suggested.encode_utf16().collect();
    if name.len() >= path.len() || name.contains(&0) {
        return Err("Nome do arquivo inválido.");
    }
    path[..name.len()].copy_from_slice(&name);
    let filter: Vec<u16> = "Vídeo MP4 (*.mp4)\0*.mp4\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Exportar Video\0".encode_utf16().collect();
    let extension: Vec<u16> = "mp4\0".encode_utf16().collect();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner as _;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrDefExt = extension.as_ptr();
    dialog.lpstrFile = path.as_mut_ptr();
    dialog.nMaxFile = path.len() as u32;
    dialog.Flags = OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetSaveFileNameW(&mut dialog) } != 0 {
        let len = path.iter().position(|c| *c == 0).unwrap_or(path.len());
        Ok(Some(std::ffi::OsString::from_wide(&path[..len]).into()))
    } else if unsafe { CommDlgExtendedError() } == 0 {
        Ok(None)
    } else {
        Err("Não foi possível abrir o seletor de destino.")
    }
}

pub fn pick_zip(owner: isize) -> Result<Option<PathBuf>, &'static str> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut path = vec![0u16; 32768];
    let filter: Vec<u16> = "Exportação SIM Next (*.zip)\0*.zip\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Importar dispositivos\0".encode_utf16().collect();
    let mut dialog: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    dialog.hwndOwner = owner as _;
    dialog.lpstrFilter = filter.as_ptr();
    dialog.lpstrTitle = title.as_ptr();
    dialog.lpstrFile = path.as_mut_ptr();
    dialog.nMaxFile = path.len() as u32;
    dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // Todos os buffers permanecem vivos durante o diálogo síncrono nesta thread.
    if unsafe { GetOpenFileNameW(&mut dialog) } != 0 {
        let len = path.iter().position(|c| *c == 0).unwrap_or(path.len());
        Ok(Some(std::ffi::OsString::from_wide(&path[..len]).into()))
    } else if unsafe { CommDlgExtendedError() } == 0 {
        Ok(None)
    } else {
        Err("Não foi possível abrir o seletor de arquivos.")
    }
}
