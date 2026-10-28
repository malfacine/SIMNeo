# funções de plataforma e credenciais Linux

As implementações de diálogos, data local e propriedade da janela ficam em
`crates/sim-ui/src/platform/windows/` e `linux/`. O Windows mantém os diálogos
Win32 e a inicialização pelo Registro. No Linux não há inicialização automática
nem essa opção na interface. Os diálogos de importação ZIP e exportação MP4
usam Zenity; a data do calendário usa o fuso local do sistema.

O banco Linux fica em `$XDG_DATA_HOME/simneo/simneo.db`, ou em
`$HOME/.local/share/simneo/simneo.db` quando XDG_DATA_HOME não contém um caminho
absoluto. `--db` continua permitindo escolher outro banco.

As senhas no banco Linux usam AES-256-GCM, nonce aleatório por gravação e uma
chave aleatória armazenada no Secret Service do usuário, acessado por
`secret-tool`. É necessário um cofre compatível, como GNOME Keyring, disponível
na sessão D-Bus. O arquivo `credentials.lock` contém apenas o lock entre
processos, nunca a chave. A ausência ou falha do cofre retorna erro; não há
gravação de senha em texto puro. A recuperação de uma senha não cria chaves.

O formato do banco continua compatível, mas as credenciais DPAPI do Windows
não podem ser decifradas no Linux. É preciso cadastrar novamente as senhas.
Copiar apenas o banco Linux para outro usuário/máquina também não transfere a
chave do cofre; preserve o cofre ao fazer backup.

Dependências de execução em Debian/Ubuntu:

```sh
sudo apt-get install zenity libsecret-tools gnome-keyring
```

Para executar os testes com um cofre descartável, sem usar o cofre pessoal,
instale também `dbus-x11`, as dependências de compilação Rust/Slint/FFmpeg e rode
na raiz do projeto:

```sh
sh native/linux/test-platform.sh
```

Validação da integração em 28/10/2026: 64 testes de core, P2P, armazenamento e
interface passaram no Debian/WSL2. No Windows, os 29 testes de P2P e interface
passaram após a integração; a fase anterior havia validado também core e
armazenamento. Os testes Linux cobriram recuperação entre processos, indisponibilidade
do D-Bus/cofre, chave ausente, item bloqueado simulado, adulteração do ciphertext
e rollback do banco. Os diálogos Zenity reais foram exercitados em Xvfb para
cancelamento, seleção ZIP e destino MP4. As implementações Windows movidas de
diálogos, inicialização e DPAPI foram comparadas com os arquivos originais.

O carregador P2P Linux e o encaminhamento TCP estão integrados à interface.
Cada câmera prepara seu túnel antes de iniciar o decoder, preservando as outras
conexões quando um canal é trocado ou fechado. Os testes do adaptador cobrem ABI
incompatível, conexões independentes, reconexão, cancelamento e liberação de
handles. Veja [compilação e validação real](../README.md).
O runner completo da interface passou no WSL2 com um DVR real, incluindo vídeo,
mosaico SD/HD, playback até 16× e MP4 decodificado externamente pelo FFmpeg.
