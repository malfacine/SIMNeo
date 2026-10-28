use crate::Error;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    fs::{DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    process::{Command, Stdio},
};
use zeroize::Zeroizing;

const HEADER: &[u8] = b"SIMNEO:LNX:1\0";
const ATTRIBUTES: &[&str] = &["application", "SimNEO", "purpose", "database-key-v1"];

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| Error::Credential)?;
    Ok(bytes)
}

fn master_key(create: bool) -> Result<Zeroizing<Vec<u8>>, Error> {
    let directory = sim_core::platform::data_directory().map_err(|_| Error::Credential)?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|_| Error::Credential)?;
    // O arquivo contém apenas o lock. A chave permanece no Secret Service.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("credentials.lock"))
        .map_err(|_| Error::Credential)?;
    lock.lock().map_err(|_| Error::Credential)?;
    let output = Command::new("secret-tool")
        .arg("lookup")
        .args(ATTRIBUTES)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| Error::Credential)?;
    let encoded = Zeroizing::new(output.stdout);
    if output.status.success() {
        let encoded = encoded.strip_suffix(b"\n").unwrap_or(&encoded);
        let key = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| Error::Credential)?);
        if key.len() != 32 {
            return Err(Error::Credential);
        }
        return Ok(key);
    }
    // secret-tool retorna 1 sem mensagem quando não existe item. Erros do serviço
    // também retornam 1, mas com diagnóstico; nunca substituir a chave nesse caso.
    if !create
        || output.status.code() != Some(1)
        || !output.stderr.is_empty()
        || !encoded.is_empty()
    {
        return Err(Error::Credential);
    }
    // A busca inclui itens bloqueados sem tentar desbloqueá-los. Um lookup
    // cancelado não pode ser confundido com ausência e substituir a chave.
    let search = Command::new("secret-tool")
        .args(["search", "--all"])
        .args(ATTRIBUTES)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| Error::Credential)?;
    let found = Zeroizing::new(search.stdout);
    if !search.status.success() || !search.stderr.is_empty() || !found.is_empty() {
        return Err(Error::Credential);
    }
    let random_key = Zeroizing::new(random::<32>()?);
    let key = Zeroizing::new(random_key.to_vec());
    let encoded = Zeroizing::new(STANDARD.encode(&key));
    let mut child = Command::new("secret-tool")
        .args(["store", "--label=SimNEO - chave do banco local"])
        .args(ATTRIBUTES)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Error::Credential)?;
    let written = child
        .stdin
        .take()
        .ok_or(Error::Credential)?
        .write_all(encoded.as_bytes());
    let result = child.wait_with_output().map_err(|_| Error::Credential)?;
    if written.is_err() || !result.status.success() {
        return Err(Error::Credential);
    }
    Ok(key)
}

pub(super) fn transform(data: &[u8], encrypt: bool) -> Result<Vec<u8>, Error> {
    if !encrypt && (!data.starts_with(HEADER) || data.len() < HEADER.len() + 12 + 16) {
        return Err(Error::Credential);
    }
    let key = master_key(encrypt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| Error::Credential)?;
    if encrypt {
        let nonce = random::<12>()?;
        let encrypted = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: data,
                    aad: HEADER,
                },
            )
            .map_err(|_| Error::Credential)?;
        let mut result = HEADER.to_vec();
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&encrypted);
        Ok(result)
    } else {
        let nonce: [u8; 12] = data[HEADER.len()..HEADER.len() + 12]
            .try_into()
            .map_err(|_| Error::Credential)?;
        cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &data[HEADER.len() + 12..],
                    aad: HEADER,
                },
            )
            .map_err(|_| Error::Credential)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_random_nonce_and_tamper_detection() {
        // Executar dentro de uma sessão Secret Service de teste desbloqueada.
        let password = "Senha ç漢字\ncom espaços ";
        let first = transform(password.as_bytes(), true).unwrap();
        let mut second = transform(password.as_bytes(), true).unwrap();
        assert_ne!(first, second);
        assert_eq!(transform(&first, false).unwrap(), password.as_bytes());
        let last = second.len() - 1;
        second[last] ^= 1;
        assert!(transform(&second, false).is_err());
        let empty = transform(b"", true).unwrap();
        assert!(transform(&empty, false).unwrap().is_empty());
    }

    #[test]
    fn rejects_other_platform_and_truncated_blobs() {
        assert!(transform(b"Windows DPAPI", false).is_err());
        assert!(transform(HEADER, false).is_err());
    }
}
