use std::{io, path::Path};
use windows_sys::Win32::{Foundation::*, System::Registry::*};
const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn command(exe: &Path, db: &Path) -> String {
    format!("\"{}\" --db \"{}\"", exe.display(), db.display())
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn write_at(key_path: &str, enabled: bool, exe: &Path, db: &Path) -> io::Result<()> {
    let mut key = std::ptr::null_mut();
    let path = wide(key_path);
    let name = wide("SimNEO");
    // Somente HKCU e a entrada SimNEO; não altera StartupApproved.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if code != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let key = Key(key);
    let code = if enabled {
        let value = wide(&command(exe, db));
        unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key.0, name.as_ptr()) }
    };
    if code == ERROR_SUCCESS || (!enabled && code == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
pub fn set(enabled: bool, exe: &Path, db: &Path) -> io::Result<()> {
    write_at(RUN, enabled, exe, db)
}
pub fn enabled(exe: &Path, db: &Path) -> io::Result<bool> {
    read_at(RUN).map(|value| value.as_deref() == Some(command(exe, db).as_str()))
}
fn read_at(key_path: &str) -> io::Result<Option<String>> {
    let path = wide(key_path);
    let name = wide("SimNEO");
    let mut buffer = vec![0u16; 32768];
    let mut bytes = (buffer.len() * 2) as u32;
    let code = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Ok(Some(String::from_utf16_lossy(&buffer[..len])))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_paths_and_toggles_only_isolated_registry_value() {
        let exe = Path::new(r"C:\Program Files\SimNEO\SimNEO.exe");
        let db = Path::new(r"C:\User Data\simneo.db");
        assert_eq!(
            command(exe, db),
            r#""C:\Program Files\SimNEO\SimNEO.exe" --db "C:\User Data\simneo.db""#
        );
        let path = format!("Software\\SimNEO\\Tests\\Startup{}", std::process::id());
        write_at(&path, true, exe, db).unwrap();
        assert_eq!(read_at(&path).unwrap(), Some(command(exe, db)));
        write_at(&path, false, exe, db).unwrap();
        assert!(read_at(&path).unwrap().is_none());
        unsafe {
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&path).as_ptr());
        }
    }
}
