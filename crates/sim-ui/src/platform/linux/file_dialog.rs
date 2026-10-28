use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf, process::Command};

fn pick(args: &[&str]) -> Result<Option<PathBuf>, &'static str> {
    let output = Command::new("zenity")
        .arg("--file-selection")
        .args(args)
        .output()
        .map_err(|_| "Não foi possível abrir o seletor de arquivos. Instale o Zenity.")?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err("O seletor de arquivos falhou.");
    }
    let mut path = output.stdout;
    if path.last() == Some(&b'\n') {
        path.pop();
    }
    if path.is_empty() {
        return Err("O seletor não retornou um arquivo.");
    }
    Ok(Some(PathBuf::from(OsString::from_vec(path))))
}

pub fn pick_zip(_: isize) -> Result<Option<PathBuf>, &'static str> {
    pick(&[
        "--title=Importar dispositivos",
        "--file-filter=Arquivos ZIP | *.zip *.ZIP",
    ])
}

pub fn save_mp4(_: isize, suggested: &str) -> Result<Option<PathBuf>, &'static str> {
    if suggested.contains('\0') {
        return Err("Nome do arquivo inválido.");
    }
    pick(&[
        "--save",
        "--confirm-overwrite",
        "--title=Exportar vídeo",
        "--file-filter=Vídeos MP4 | *.mp4",
        &format!("--filename={suggested}"),
    ])
}
