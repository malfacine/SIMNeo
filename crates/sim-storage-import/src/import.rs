use crate::{Error, NewDevice};
use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use base64::{Engine, engine::general_purpose::STANDARD};
use quick_xml::{Reader, events::Event};
use serde::Deserialize;
use sim_core::{Channel, DeviceDraft, Manufacturer};
use zeroize::Zeroizing;

/// Formato legado sem autenticação: sucesso do padding não autentica o arquivo.
pub fn decrypt_xml_password(encoded: &str, password: &str) -> Result<Zeroizing<String>, Error> {
    let payload = Zeroizing::new(
        STANDARD
            .decode(encoded)
            .map_err(|_| Error::Import("Senha criptografada inválida."))?,
    );
    if payload.len() < 32 || payload.len() % 16 != 0 {
        return Err(Error::Import("Senha criptografada incompleta."));
    }
    // String.PadRight conta unidades UTF-16. Buffer.BlockCopy usa os primeiros
    // 32 bytes UTF-8, mesmo quando isso corta uma sequência multibyte.
    let mut padded = Zeroizing::new(password.to_owned());
    let units = password.encode_utf16().count();
    for _ in units..32 {
        padded.push(' ');
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&padded.as_bytes()[..32]);
    let plain = Zeroizing::new(
        cbc::Decryptor::<aes::Aes256>::new_from_slices(&*key, &payload[..16])
            .map_err(|_| Error::Import("Chave de importação inválida."))?
            .decrypt_padded_vec::<Pkcs7>(&payload[16..])
            .map_err(|_| Error::Import("Chave incorreta ou senha criptografada corrompida."))?,
    );
    let text = std::str::from_utf8(&plain)
        .map_err(|_| Error::Import("Chave incorreta ou texto da senha inválido."))?;
    Ok(Zeroizing::new(
        text.trim_start_matches('\u{feff}').to_owned(),
    ))
}

#[derive(Deserialize)]
struct Export {
    #[serde(rename = "Devices")]
    devices: Devices,
}
#[derive(Deserialize)]
struct Devices {
    #[serde(rename = "Device", default)]
    items: Vec<XmlDevice>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XmlDevice {
    name: String,
    #[serde(rename = "MainIPAddress", default)]
    main_ip: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    serial_number: String,
    port: u16,
    user_name: String,
    manufacturer: String,
    #[serde(default, alias = "ExportEncryptedPassword")]
    encrypted_password: String,
    #[serde(default)]
    channels: Channels,
    #[serde(default)]
    is_intelbras_cloud_enabled: bool,
    #[serde(default)]
    is_domain_enabled: bool,
    #[serde(rename = "IsAlternativeIPEnabled", default)]
    alternative: bool,
    #[serde(default)]
    is_register_enabled: bool,
}
#[derive(Default, Deserialize)]
struct Channels {
    #[serde(rename = "Channel", default)]
    items: Vec<XmlChannel>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XmlChannel {
    number: u16,
    name: String,
}

pub fn parse_simnext_xml(xml: &str, key: &str) -> Result<Vec<NewDevice>, Error> {
    if xml.len() > 20 * 1024 * 1024 {
        return Err(Error::Import("O XML excede o limite de 20 MB."));
    }
    validate_document(xml)?;
    let export: Export = quick_xml::de::from_str(xml)
        .map_err(|_| Error::Import("O XML não contém um cadastro de dispositivos compatível."))?;
    if export.devices.items.is_empty() || export.devices.items.len() > 5000 {
        return Err(Error::Import(
            "O arquivo precisa conter entre 1 e 5000 dispositivos.",
        ));
    }
    let mut result = Vec::new();
    for item in export.devices.items {
        if item.is_domain_enabled || item.alternative || item.is_register_enabled {
            return Err(Error::Import(
                "Conexões alternativas, domínio DDNS e auto-registro ainda não são suportados na importação.",
            ));
        }
        if item.channels.items.len() > 1024 {
            return Err(Error::Import("Quantidade de canais excedida."));
        }
        let manufacturer = match item.manufacturer.as_str() {
            "Dahua" => Manufacturer::Dahua,
            "Jufeng" => Manufacturer::Jufeng,
            _ => return Err(Error::Import("Fabricante não suportado no XML.")),
        };
        if item.is_intelbras_cloud_enabled && item.serial_number.trim().is_empty() {
            return Err(Error::Import(
                "O dispositivo Cloud não informa um serial válido.",
            ));
        }
        let primary = if item.main_ip.trim().is_empty() {
            item.address.trim()
        } else {
            item.main_ip.trim()
        };
        let primary = if item.is_intelbras_cloud_enabled
            && primary.eq_ignore_ascii_case(item.serial_number.trim())
        {
            ""
        } else {
            primary
        };
        let serial_only = primary.is_empty() && !item.serial_number.trim().is_empty();
        let mut config = DeviceDraft {
            serial: if !serial_only && !item.serial_number.trim().is_empty() {
                Some(item.serial_number.trim().into())
            } else {
                None
            },
            connection: if serial_only {
                sim_core::ConnectionMode::P2p
            } else {
                sim_core::ConnectionMode::Address
            },
            name: item.name,
            address: if serial_only {
                item.serial_number.trim().into()
            } else {
                primary.into()
            },
            port: item.port,
            username: item.user_name,
            manufacturer,
            channels: item
                .channels
                .items
                .into_iter()
                .map(|c| Channel {
                    number: c.number,
                    name: c.name,
                })
                .collect(),
        };
        config.normalize_serials();
        config.validate()?;
        let password = if item.encrypted_password.is_empty() {
            Zeroizing::new(String::new())
        } else {
            decrypt_xml_password(&item.encrypted_password, key)?
        };
        result.push(NewDevice { config, password });
    }
    Ok(result)
}

fn validate_document(xml: &str) -> Result<(), Error> {
    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut root_seen = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| Error::Import("XML malformado."))?
        {
            Event::DocType(_) => return Err(Error::Import("XML com DTD não é aceito.")),
            Event::Start(e) if depth == 0 => {
                if root_seen || e.local_name().as_ref() != "ExportableSystemData" {
                    return Err(Error::Import("Selecione um XML exportado pelo SIM Next."));
                }
                root_seen = true;
                depth += 1;
            }
            Event::Empty(e) if depth == 0 => {
                if root_seen || e.local_name().as_ref() != "ExportableSystemData" {
                    return Err(Error::Import("Selecione um XML exportado pelo SIM Next."));
                }
                root_seen = true;
            }
            Event::Start(_) => {
                depth += 1;
                if depth > 64 {
                    return Err(Error::Import("XML excede 64 níveis."));
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(Error::Import("XML malformado."))?;
            }
            Event::Text(t) if depth == 0 && !t.as_ref().trim().is_empty() => {
                return Err(Error::Import("Conteúdo fora da raiz XML."));
            }
            Event::CData(_) if depth == 0 => return Err(Error::Import("CDATA fora da raiz XML.")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || depth != 0 {
        return Err(Error::Import("XML incompleto."));
    }
    Ok(())
}
