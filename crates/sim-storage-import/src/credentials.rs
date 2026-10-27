use crate::Error;

#[cfg(windows)]
fn transform(data: &[u8], encrypt: bool) -> Result<Vec<u8>, Error> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    use zeroize::Zeroize;

    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len().try_into().map_err(|_| Error::Credential)?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // SAFETY: Os buffers de entrada vivem até o retorno da API síncrona.
    // O Windows aloca output; copiamos e liberamos com LocalFree em ambos os casos.
    // Sem LOCAL_MACHINE: apenas o usuário atual pode recuperar as credenciais.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(Error::Credential);
        }
        if output.cbData == 0 {
            if !output.pbData.is_null() {
                LocalFree(output.pbData.cast());
            }
            return Ok(Vec::new());
        }
        if output.pbData.is_null() {
            return Err(Error::Credential);
        }
        let buffer = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result = buffer.to_vec();
        buffer.zeroize();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}

#[cfg(not(windows))]
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
