# bibliotecas por plataforma

O transporte P2P Linux está integrado ao aplicativo Rust. As implementações
ficam em `crates/sim-p2p-t2u/src/linux.rs` e `windows.rs`; a implementação Windows
foi movida sem alteração de comportamento e mantém `assets/libt2u.dll`.
Veja [funções de plataforma](linux/platform.md).
O empacotamento Linux está em [AppImage](linux/appimage/README.md).

| Pasta | Saída | Estado |
| --- | --- | --- |
| `linux/t2u/` | `build/libsim_t2u_linux.so` | Implementação nativa IPv4: coordenador, rendezvous, relay e fluxo confiável VDT. |
| `linux/netsdk/` | `build/libsim_intelbras_linux.so` | Wrapper do NetSDK oficial, validado com login através do túnel Linux. |

## validação real em 27/10/2026

Ambiente: Debian x86-64 no WSL2. O transporte Linux depende apenas da libc,
sem Wine, DLL Windows ou processo Windows intermediando a comunicação de rede.
O PowerShell é utilizado somente pelo runner para ler os perfis DPAPI existentes
e enviar a configuração ao processo Linux por stdin.

- Coordenador, autenticação, rendezvous pelo serial, registro no relay,
  negociação com o peer, transporte VDT e canal remoto `127.0.0.1:37777` passaram.
- O DVR respondeu ao desafio binário: `device_tunnel_verified = true`.
- O NetSDK oficial Linux autenticou através desse transporte: `logged_in = 1`,
  `channels = 18`, sem erro de login ou transporte. Três execuções consecutivas
  passaram após a correção da renegociação; a verificação final também passou.
- Testes automatizados com UDP local passaram: entradas inválidas,
  autenticação, relay, manutenção da sessão, perda/retransmissão, duplicação,
  dados truncados, escrita em partes e remontagem do fluxo entre datagramas.
- Os mesmos testes passaram com AddressSanitizer e UndefinedBehaviorSanitizer.
  A detecção de vazamentos foi desativada somente no host Python da suíte;
  isso não constitui teste de ausência de vazamentos.
- TEA comparado com a DLL Windows fixada: onze vetores sintéticos idênticos.

O bloqueio inicial era a ausência de relay: a referência Windows também usava
relay (`proxy = 1`). Outra falha era refazer o rendezvous depois de receber os
endpoints, substituindo a sessão enquanto aguardava o peer. Um teste de regressão
reproduziu essa falha antes da correção.

O VDT usado pelo dispositivo tem assinatura `0x1201` e cabeçalho em ordem de rede;
não é o transporte UDP legado descrito no relatório antigo. O caminho moderno
usa o token na negociação, sem exigir o diagnóstico legado `2007/2008`. Por isso
`peer_authenticated`, campo desse diagnóstico, pode ser zero enquanto
`peer_connected`, `transport_ready`, `stream_opened` e o login real passam.

## compilar e testar

Requisitos Linux: compilador C/C++, make e Python 3 para a suíte. O NetSDK deve
permanecer em `sdk/NetSDK 3.050/Linux/`. As bibliotecas proprietárias não foram
copiadas ou redistribuídas com o código deste diretório.

Na raiz do projeto, em PowerShell:

```powershell
wsl -d Debian -- sh -lc 'cd /mnt/d/Server/apps/SimNEO/native/linux/netsdk && make test'
wsl -d Debian -- sh -lc 'cd /mnt/d/Server/apps/SimNEO/native/linux/t2u && make test && make sanitize'
python3 native/linux/t2u/tests/compare_dll.py

# Túnel real, desafio do DVR e login pelo SDK Linux:
./native/linux/t2u/tests/probe-wsl.ps1 -Device .tools/test-device.dpapi

# Somente autenticação com o coordenador:
./native/linux/t2u/tests/probe-wsl.ps1

# Referência Windows em processo separado; não altera o arquivo DLL:
./native/linux/t2u/tests/probe-wsl.ps1 -ReferenceWindows -Device .tools/test-device.dpapi
```

Os runners não passam credenciais na linha de comando, não imprimem senhas ou
seriais e não criam arquivos descriptografados. Perfis e SDKs continuam ignorados
pelo Git. `LD_LIBRARY_PATH` é definido somente no subprocesso que usa o SDK.
Fora do runner, configure-o para o diretório `Linux/bin` desse SDK.

## API Linux

Header público: `linux/t2u/include/sim_t2u.h`, compatível com C e C++.

1. `sim_t2u_open(...)`: recebe servidor, chave, serial em maiúsculas, senha do
   túnel, destino remoto e timeout; retorna um handle opaco e os marcos da conexão.
2. `sim_t2u_write(...)`: envia e aguarda confirmação; retorna bytes escritos ou
   erro negativo. Limite de 64 KiB por chamada, com segmentação interna.
3. `sim_t2u_read(...)`: lê parte do fluxo; retorna bytes, zero no fechamento ou
   erro negativo. O chamador deve remontar os frames do protocolo de aplicação.
4. `sim_t2u_close(...)`: encerra o canal e libera socket e buffers.

Timeout de leitura é `SIM_T2U_TIMEOUT` (`-4`) e permite continuar lendo. Libere
o handle após outros erros; não reutilize um canal após erro de escrita. As
chamadas do mesmo handle precisam ser serializadas. Leitura/escrita processam
keepalive; faça polling a cada três segundos ou menos durante períodos ociosos.

A API é própria: não substitui os símbolos ou a ABI da DLL proprietária.
`sim_t2u_abi_version()` retorna 1. O carregador Rust verifica essa versão antes
de carregar os símbolos. Cada conexão TCP local possui seu próprio handle
nativo; fechar uma câmera não fecha o controle ou as outras câmeras. Chamadas
bloqueantes ficam em threads próprias. O encerramento aguarda os workers e
libera os handles antes de descarregar a biblioteca.

## integração no aplicativo Linux

Compile a biblioteca com `make -C native/linux/t2u` e o aplicativo com
`cargo build --locked -p sim-ui --bin sim-ui`. O ambiente precisa das bibliotecas
de desenvolvimento FFmpeg/Slint usadas pelo projeto. Em execução, use
`assets/libsim_t2u_linux.so` ao lado do binário, ou defina
`SIMNEO_P2P_LIBRARY` com o caminho absoluto da biblioteca. No checkout, o
carregador também procura `native/linux/t2u/build/libsim_t2u_linux.so` a partir
do diretório atual. A configuração JSON aceita `p2pLibrary` como alias de
`p2pDll`. O perfil do serviço P2P continua privado, conforme o build existente.

O adaptador Rust restringe o listener a `127.0.0.1`. A referência do NetSDK em
`linux/netsdk/` é somente um teste adicional; o aplicativo não depende dela.

Testes da interface com DVR real, banco e cofre descartáveis no WSL:

```powershell
./native/linux/test-ui-wsl.ps1 -Mode video
./native/linux/test-ui-wsl.ps1 -Mode mosaic
./native/linux/test-ui-wsl.ps1 -Mode playback
```

Compile também `cargo build --locked -p sim-ui --example linux_seed` antes desses
testes. O runner espera binários em `/home/doc/.cache/simneo-target/debug`;
`-BinaryDirectory` permite outro diretório. Requer Xvfb, D-Bus, GNOME Keyring,
secret-tool, ffprobe e ffmpeg. As credenciais chegam por stdin. O teste de
playback usa gravações de 26/10/2026 no canal 2, exporta MP4 temporário e verifica
o arquivo com ffprobe e decodificação completa pelo FFmpeg.

Na validação de 28/10/2026, passaram vídeo real e mosaico com quatro canais
SD/HD, ampliação, fechamento isolado, substituição de câmera e alternância de
layout 4 → 1 → 4. O transporte agora mantém a autenticação com o coordenador,
responde a pedidos de ACK, solicita os pacotes ausentes e aceita o contador
de retransmissão no cabeçalho VDT. A negociação da janela ocorre somente na
abertura; o keepalive não reinicia essa negociação. Há regressões locais para
essas condições.
Os 26 testes UDP locais passaram também com AddressSanitizer e
UndefinedBehaviorSanitizer. Os campos corrigidos e a referência utilizada
estão em [notas do protocolo](linux/t2u/protocol-notes.md).

O runner completo (`-Mode all`) passou com a biblioteca final: descoberta dos
18 canais, vídeo ao vivo, mosaico, playback com pausa e seek e velocidades
2×/4×/8×/16×. Na medição de 16×, o relógio avançou 23 segundos em 1,55 segundo.
O MP4 exportado foi identificado como H.264 1280×720, duração de 10,000667
segundos, e decodificado integralmente com `ffmpeg -xerror`, sem erro.
O mosaico também passou no Windows usando a DLL original e uma cópia descartável
do banco de teste. No Linux foi usado o renderizador software, sem GPU.

## limites desta entrega

- Testado em um DVR, por IPv4 e relay, no WSL2. Outros firmwares, NATs, IPv6
  e situações de reconexão ainda precisam de validação.
- Um canal de aplicação por handle, sem listener público. Escritas confirmam
  cada pacote; leituras mantêm um buffer limitado e pedem retransmissão de
  dados fora de ordem. Não foi realizado benchmark de vazão máxima.
- NAT 5 e plataforma 3 são parâmetros de compatibilidade medidos na referência;
  ainda não há classificação completa de NAT.
- Sem inicialização automática no Linux, instalador ou publicação de release.

O NetSDK sozinho não resolveu o serial no modo 19 do teste anterior. O header
documenta o modo 16 como proxy que recebe um SOCKET já conectado. A solução
validada aqui é **transporte P2P Linux nativo + NetSDK Linux**, sem depender
desse modo para descobrir ou abrir a conexão.
