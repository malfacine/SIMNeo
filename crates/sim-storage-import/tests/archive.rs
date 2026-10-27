use sim_storage_import::import_simnext_zip;
use std::{fs::File, io::Write, path::Path};
use zip::unstable::write::FileOptionsExt;
use zip::{ZipWriter, write::SimpleFileOptions};

fn write_zip(path: &Path, password: Option<&str>, names: &[&str], xml: &str) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default();
    let options = if let Some(password) = password {
        options.with_aes_encryption(zip::AesMode::Aes256, password)
    } else {
        options
    };
    for name in names {
        writer.start_file(*name, options).unwrap();
        writer.write_all(xml.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn imports_plain_and_encrypted_zip_and_rejects_wrong_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.zip");
    let xml = include_str!("../../../fixtures/simnext-example.xml");
    for password in [None, Some("teste")] {
        write_zip(&path, password, &["folder/devices.XML"], xml);
        let devices = import_simnext_zip(&path, "teste").unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(&*devices[0].password, "camera-sintetica");
        assert!(import_simnext_zip(&path, "errada").is_err());
    }
}

#[test]
fn imports_legacy_zipcrypto_export() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.zip");
    let mut writer = ZipWriter::new(File::create(&path).unwrap());
    let options = SimpleFileOptions::default()
        .with_deprecated_encryption(b"teste")
        .unwrap();
    writer.start_file("devices.xml", options).unwrap();
    writer
        .write_all(include_bytes!("../../../fixtures/simnext-example.xml"))
        .unwrap();
    writer.finish().unwrap();
    assert_eq!(import_simnext_zip(&path, "teste").unwrap().len(), 1);
    assert!(import_simnext_zip(&path, "incorrect").is_err());
}

#[test]
fn rejects_missing_or_multiple_xml_and_oversized_xml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.zip");
    for names in [&["other.txt"][..], &["a.xml", "b.xml"][..]] {
        write_zip(&path, None, names, "test");
        assert!(import_simnext_zip(&path, "").is_err());
    }
    write_zip(
        &path,
        None,
        &["large.xml"],
        &"x".repeat(20 * 1024 * 1024 + 1),
    );
    assert!(import_simnext_zip(&path, "").is_err());
    std::fs::write(&path, b"invalid zip").unwrap();
    assert!(import_simnext_zip(&path, "").is_err());
}
