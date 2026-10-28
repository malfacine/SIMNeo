# AppImage Linux x86_64

O pacote inclui o binário Rust em modo release, transporte P2P Linux, bibliotecas
FFmpeg, bibliotecas da interface, fontes, Zenity e `secret-tool`. Não inclui banco
de usuário, perfis DPAPI, SDK proprietário ou DLL Windows. Não configura
inicialização automática.

Requer uma sessão gráfica Linux, glibc compatível com a versão indicada no
arquivo `*-build-info.json` e um Secret Service desbloqueado na sessão D-Bus,
como GNOME Keyring. O serviço do cofre pertence ao sistema; somente seu cliente
é incluído no pacote. As senhas DPAPI existentes no Windows precisam ser
cadastradas novamente no Linux.

A compilação 0.26.1 exige glibc 2.39 ou mais recente. A inicialização gráfica foi
validada no Debian 13/WSL2 tanto com montagem FUSE quanto com extração temporária.
O arquivo distribuído passou por descoberta de 18 canais, vídeo ao vivo,
mosaico SD/HD, troca e fechamento independente de câmeras. Playback com pausa,
seek, velocidades até 16× e exportação MP4 passou na repetição isolada; na
primeira execução completa, houve timeout de comunicação com o DVR após 4×.
O MP4 H.264 1280×720, com 10 segundos, foi decodificado integralmente sem erro.

Para executar:

```sh
chmod +x SIMNeo-0.26.1-linux-x86_64.AppImage
./SIMNeo-0.26.1-linux-x86_64.AppImage
```

O runtime usa FUSE. Em sistemas sem FUSE, use a extração temporária integrada:

```sh
./SIMNeo-0.26.1-linux-x86_64.AppImage --appimage-extract-and-run
```

O banco segue `$XDG_DATA_HOME/simneo/simneo.db`, com fallback para
`$HOME/.local/share/simneo/simneo.db`. A renderização da interface usa software
por padrão; `SLINT_BACKEND` continua permitindo selecionar outro backend.

## gerar o pacote

Use Linux x86_64 com o toolchain Rust e dependências de compilação do projeto.
No Debian, o empacotador usa também `python3`, `file`, `binutils`, `patchelf`,
`desktop-file-utils`, `squashfs-tools`, `zenity`, `libsecret-tools`, fontes
DejaVu e temas de ícones Adwaita/hicolor. O script resolve as bibliotecas do
sistema de compilação; ele não fornece compatibilidade com glibc mais antiga.

```sh
sh native/linux/build-appimage.sh
```

`CARGO_TARGET_DIR` permite reutilizar um diretório de compilação. O padrão é
`$HOME/.cache/simneo-target`; `CARGO_BUILD_JOBS` é 1 para limitar o consumo de
memória. O perfil privado do serviço P2P segue o mecanismo existente de build,
sem copiar o arquivo de origem para o pacote.

`appimagetool` e o runtime são obtidos dos repositórios oficiais e verificados
por SHA-256 fixado em `package.py`. Uma atualização dessas ferramentas exige
revisar os hashes; o script não aceita silenciosamente outra versão. As saídas
ficam em `dist/`: AppImage, checksum e manifesto de versões das dependências.
As licenças dos componentes também ficam dentro do AppImage.

## testar o arquivo distribuído

Com os binários auxiliares de teste previamente compilados e o perfil DPAPI
local de teste, no PowerShell:

```powershell
./native/linux/test-ui-wsl.ps1 -Mode all -AppImage ./dist/SIMNeo-0.26.1-linux-x86_64.AppImage
```

Esse runner abre o AppImage, não o executável do checkout. Ele usa banco e cofre
descartáveis, testa conexão, vídeo, mosaico, playback e exportação, e valida o
MP4 com ffprobe e decodificação completa pelo FFmpeg. Os auxiliares e o DVR de
teste não são incluídos no pacote distribuído.

O runner aceita Windows PowerShell 5.1 e PowerShell 7. Para verificar o envio de
argumentos e UTF-8 ao WSL sem acessar o DVR ou perfis de credenciais:

```powershell
./native/linux/test-wsl-runner.ps1
```

Referências do formato: [appimagetool](https://github.com/AppImage/appimagetool)
e [execução sem FUSE](https://docs.appimage.org/user-guide/troubleshooting/fuse.html).
