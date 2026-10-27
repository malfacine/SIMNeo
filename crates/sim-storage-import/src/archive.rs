use crate::{Error, NewDevice, parse_simnext_xml};
use std::{fs::File, io::Read, path::Path};
use zeroize::Zeroizing;

const MAX_XML: u64 = 20 * 1024 * 1024;

/// Lê em memória: nenhum caminho interno do ZIP é extraído no disco.
pub fn import_simnext_zip(path: &Path, password: &str) -> Result<Vec<NewDevice>, Error> {
    let file = File::open(path)?;
    if file.metadata()?.len() > 100 * 1024 * 1024 {
        return Err(Error::Import("O ZIP excede o limite de 100 MB."));
    }
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| Error::Import("Arquivo ZIP inválido ou não suportado."))?;
    if archive.len() > 5000 {
        return Err(Error::Import("O ZIP contém arquivos demais."));
    }
    let xmls: Vec<usize> = archive
        .file_names()
        .enumerate()
        .filter(|(_, name)| name.to_ascii_lowercase().ends_with(".xml"))
        .map(|(index, _)| index)
        .collect();
    let [index] = xmls.as_slice() else {
        return Err(Error::Import(
            "Selecione um ZIP contendo exatamente um arquivo XML de dispositivos.",
        ));
    };
    let entry = archive
        .by_index_decrypt(*index, password.as_bytes())
        .map_err(|_| Error::Import("Não foi possível abrir o XML. Confira a senha do ZIP."))?;
    if entry.size() > MAX_XML {
        return Err(Error::Import("O XML excede o limite de 20 MB."));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    entry
        .take(MAX_XML + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Import("Senha do ZIP incorreta ou arquivo corrompido."))?;
    if bytes.len() as u64 > MAX_XML {
        return Err(Error::Import("O XML excede o limite de 20 MB."));
    }
    let xml =
        std::str::from_utf8(&bytes).map_err(|_| Error::Import("O XML precisa estar em UTF-8."))?;
    parse_simnext_xml(xml.trim_start_matches('\u{feff}'), password)
}
