fn main() {
    let private_source = "src/p2p_service.local.rs";
    println!("cargo:rerun-if-changed={private_source}");
    let embedded =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("p2p_service.rs");
    if std::path::Path::new(private_source).is_file() {
        std::fs::copy(private_source, embedded).expect("Erro ao incorporar o perfil P2P");
    } else {
        std::fs::write(embedded, "const EMBEDDED_P2P_SERVICE: Option<&str> = None;")
            .expect("Erro ao preparar o perfil P2P");
    }
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("../../assets/icon.ico")
            .set("ProductName", "SIMNeo")
            .set("FileDescription", "SIMNeo — cliente de videomonitoramento")
            .set("OriginalFilename", "SIMNeo.exe")
            .compile()
            .expect("Erro ao incorporar o ícone e a versão do SIMNeo");
    }
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("Erro ao compilar a interface Slint");
}
