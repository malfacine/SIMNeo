//! Cadastra um DVR de teste em banco separado; lê o perfil exclusivamente de stdin.
//! Execute dentro do Secret Service descartável usado pelo runner Linux.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sim_core::{Channel, ConnectionMode, DeviceDraft, Manufacturer};
    use sim_storage_import::{NewDevice, Store};
    use std::{io::Read, path::PathBuf};
    use zeroize::{Zeroize, Zeroizing};
    #[derive(serde::Deserialize)]
    struct Profile {
        serial: String,
        username: String,
        password: String,
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            self.serial.zeroize();
            self.username.zeroize();
            self.password.zeroize();
        }
    }
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Informe o banco de teste.")?,
    );
    if path.exists() {
        return Err("O banco de teste já existe.".into());
    }
    let mut json = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut json)?;
    let mut profile: Profile =
        serde_json::from_str(&json).map_err(|_| "Perfil de teste inválido.")?;
    let mut store = Store::open(&path)?;
    store.insert_batch(&[NewDevice::new(
        DeviceDraft {
            name: "Teste Linux".into(),
            serial: None,
            address: std::mem::take(&mut profile.serial),
            connection: ConnectionMode::P2p,
            port: 37777,
            username: std::mem::take(&mut profile.username),
            manufacturer: Manufacturer::Dahua,
            channels: (0..18)
                .map(|number| Channel {
                    number,
                    name: format!("Canal {}", number + 1),
                })
                .collect(),
        },
        std::mem::take(&mut profile.password),
    )])?;
    println!("Banco de teste criado; senha protegida pelo cofre Linux.");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Exemplo exclusivo para Linux.");
}
