use crate::Error;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
use linux::transform;
#[cfg(windows)]
use windows::transform;

#[cfg(not(any(windows, target_os = "linux")))]
fn transform(_: &[u8], _: bool) -> Result<Vec<u8>, Error> {
    Err(Error::Credential)
}

pub fn protect(password: &str) -> Result<Vec<u8>, Error> {
    transform(password.as_bytes(), true)
}
pub fn unprotect(data: &[u8]) -> Result<zeroize::Zeroizing<String>, Error> {
    let bytes = zeroize::Zeroizing::new(transform(data, false)?);
    let password = std::str::from_utf8(&bytes).map_err(|_| Error::Credential)?;
    Ok(zeroize::Zeroizing::new(password.to_owned()))
}
