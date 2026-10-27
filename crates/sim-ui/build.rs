fn main() {
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
