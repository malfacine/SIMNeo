# Componentes de terceiros

A licença AGPL-3.0 do SIMNeo se aplica ao código desenvolvido neste projeto.
Ela não altera a licença, a autoria nem os direitos dos componentes de terceiros.

## DLL original do SIM Next

`assets/libt2u.dll` é a DLL original utilizada pelo SIM Next para a comunicação
P2P e a criação de túneis. O SIMNeo a carrega dinamicamente; o arquivo não é
modificado. Essa DLL é proprietária e permanece sujeita aos termos de seu titular.
Ela não faz parte do código licenciado sob AGPL-3.0.

## FFmpeg

O pacote Windows usa FFmpeg 8.1, carregado por DLLs separadas, para vídeo e exportação.
Ele utiliza a distribuição LGPL compartilhada da BtbN. Os textos de licença
acompanham a pasta `licenses` e as DLLs podem ser substituídas por versões compatíveis.

- Código-fonte: https://ffmpeg.org/download.html
- Distribuição e instruções de compilação: https://github.com/BtbN/FFmpeg-Builds
- Build: `ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip`, obtido em 25/10/2026.
- SHA256 do arquivo original: `edc7450333aae4042e1f4670aa7bcafe4265324ba9f75abf8de49e8b52812389`.

O AppImage Linux usa as bibliotecas compartilhadas FFmpeg 7.1.5 do Debian 13,
incluindo suas dependências e os codecs habilitados nessa distribuição. As
versões dos pacotes estão em `usr/share/doc/SIMNeo/build-info.json` dentro do
AppImage e no manifesto que acompanha o download. Os avisos e textos de licença
dos pacotes Linux ficam em `usr/share/doc/SIMNeo/licenses`.

## Microsoft Visual C++ Runtime

O pacote portátil inclui `vcruntime140.dll` x64, sem modificações, obtida da
pasta de arquivos redistribuíveis do Visual Studio. O componente é proprietário
da Microsoft e não faz parte do código licenciado sob AGPL-3.0.

- Redistribuição e termos aplicáveis: https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files

## Slint e demais dependências

A interface utiliza Slint 1.18.1, sob a opção GPL-3.0 oferecida pelo fornecedor.
Seus textos de licença acompanham o pacote Windows em `licenses` e o AppImage
em `usr/share/doc/SIMNeo/licenses/rust`.

- Slint: https://github.com/slint-ui/slint
- zip-rs 8.6.0: licença MIT; https://github.com/zip-rs/zip2
- winresource 0.1.34: ferramenta de compilação sob licença MIT;
  https://github.com/caxteo/winresource

As demais dependências e suas versões estão registradas em `Cargo.lock`.
O AppImage inclui também Zenity, libsecret, GTK, fontes DejaVu e temas de ícones
Adwaita/hicolor, com os avisos dos respectivos pacotes. As ferramentas de
empacotamento são documentadas em [AppImage](native/linux/appimage/README.md).
O NetSDK proprietário usado nos testes Linux não é redistribuído no AppImage.
