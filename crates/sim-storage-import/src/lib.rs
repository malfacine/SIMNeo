//! Persistência e importação de dispositivos.

mod archive;
mod credentials;
pub use archive::import_simnext_zip;
mod import;
mod storage;
pub use import::{decrypt_xml_password, parse_simnext_xml};
use sim_core::{DeviceDraft, ValidationError};
pub use storage::Store;
use thiserror::Error;
use zeroize::Zeroizing;

pub type ImportKey = Zeroizing<String>;

/// Arquivo de configuração local protegido pelo cofre do usuário atual.
pub fn read_protected_file(path: &std::path::Path) -> Result<Zeroizing<String>, Error> {
    let protected = std::fs::read(path)?;
    credentials::unprotect(&protected)
}

/// Não implementa Debug para evitar exposição de credenciais em logs.
pub struct NewDevice {
    pub config: DeviceDraft,
    pub password: Zeroizing<String>,
}

impl NewDevice {
    pub fn new(mut config: DeviceDraft, password: String) -> Self {
        config.normalize_serials();
        Self {
            config,
            password: Zeroizing::new(password),
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("Não foi possível acessar o banco local: {0}")]
    Database(#[from] rusqlite::Error),
    #[cfg_attr(windows, error("Não foi possível proteger ou recuperar a credencial do Windows."))]
    #[cfg_attr(not(windows), error("Não foi possível proteger ou recuperar a credencial. Verifique o cofre de senhas do usuário e o secret-tool."))]
    Credential,
    #[error("O banco foi criado por uma versão mais recente do SimNEO.")]
    NewerDatabase,
    #[error("Não foi possível ler o arquivo selecionado.")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Import(&'static str),
}
