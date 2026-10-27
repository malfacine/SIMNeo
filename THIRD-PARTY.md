# Componentes de terceiros

A licença AGPL-3.0 do SIMNeo se aplica ao código desenvolvido neste projeto.
Ela não altera a licença, a autoria nem os direitos dos componentes de terceiros.

## DLL original do SIM Next

`assets/libt2u.dll` é a DLL original utilizada pelo SIM Next para a comunicação
P2P e a criação de túneis. O SIMNeo a carrega dinamicamente; o arquivo não é
modificado. Essa DLL é proprietária e permanece sujeita aos termos de seu titular.
Ela não faz parte do código licenciado sob AGPL-3.0.

## FFmpeg

O aplicativo usa FFmpeg 8.1, carregado por DLLs separadas, para vídeo e exportação.
O pacote utiliza a distribuição LGPL compartilhada da BtbN. Os textos de licença
acompanham a pasta `licenses` e as DLLs podem ser substituídas por versões compatíveis.

- Código-fonte: https://ffmpeg.org/download.html
- Distribuição e instruções de compilação: https://github.com/BtbN/FFmpeg-Builds
- Build: `ffmpeg-n8.1-latest-win64-lgpl-shared-8.1.zip`, obtido em 25/10/2026.
- SHA256 do arquivo original: `edc7450333aae4042e1f4670aa7bcafe4265324ba9f75abf8de49e8b52812389`.

## Slint e demais dependências

A interface utiliza Slint 1.18.1, sob a opção GPL-3.0 oferecida pelo fornecedor.
Seus textos de licença acompanham o pacote em `licenses`.

- Slint: https://github.com/slint-ui/slint
- zip-rs 8.6.0: licença MIT; https://github.com/zip-rs/zip2
- winresource 0.1.34: ferramenta de compilação sob licença MIT;
  https://github.com/caxteo/winresource

As demais dependências e suas versões estão registradas em `Cargo.lock`.
