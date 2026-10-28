use std::{io, path::PathBuf};

/// Diretório persistente do usuário, independente do diretório de execução.
pub fn data_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("SimNEO"))
        .ok_or_else(|| io::Error::other("LOCALAPPDATA não está definido."));
    #[cfg(target_os = "linux")]
    return linux_directory(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"));
}

#[cfg(target_os = "linux")]
fn linux_directory(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> io::Result<PathBuf> {
    if let Some(path) = xdg.map(PathBuf::from).filter(|path| path.is_absolute()) {
        return Ok(path.join("simneo"));
    }
    home.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".local/share/simneo"))
        .ok_or_else(|| io::Error::other("HOME não está definido com um caminho absoluto."))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn follows_xdg_and_ignores_relative_values() {
        assert_eq!(
            linux_directory(Some("/data".into()), None).unwrap(),
            PathBuf::from("/data/simneo")
        );
        assert_eq!(
            linux_directory(Some("relative".into()), Some("/home/test".into())).unwrap(),
            PathBuf::from("/home/test/.local/share/simneo")
        );
        assert!(linux_directory(None, None).is_err());
    }
}
