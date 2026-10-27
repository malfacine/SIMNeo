<p align="center">
  <img src="assets/icon.png" alt="Ícone do SIMNeo" width="96">
</p>

# SIMNeo

O SIMNeo é um programa de videomonitoramento para Windows, escrito em Rust e
pensado para ser uma alternativa leve ao SIMNext, com uma interface simples para
acompanhar suas câmeras.

**Versão inicial do projeto: 0.26.0.**

## O que você pode fazer

- Assistir às câmeras ao vivo em diferentes layouts, com qualidade SD ou HD.
- Cadastrar dispositivos por IP, serial ou ambos.
- Buscar e reproduzir gravações de uma câmera por vez.
- Controlar a reprodução em 1×, 2×, 4×, 8× e 16×, com buffer de vídeo.
- Exportar trechos em MP4, mantendo a resolução original da qualidade escolhida.
- Importar dispositivos de um ZIP exportado pelo SIM Next.
- Restaurar a última visualização, iniciar com o Windows e usar aceleração por GPU.

## Como usar

1. Baixe o pacote ZIP da versão desejada.
2. Extraia todo o conteúdo para uma pasta.
3. Abra `SIMNeo.exe`.
4. Cadastre seu dispositivo e clique em uma câmera para abrir o vídeo.

Mantenha os arquivos do pacote juntos. Execute o programa diretamente da pasta extraída.

## Conexão com os dispositivos

Você pode usar o IP do dispositivo, o serial ou os dois. Quando ambos estão
cadastrados, o SIMNeo tenta o IP primeiro e usa o serial se essa conexão falhar.

Para criar os túneis de conexão por serial, o SIMNeo utiliza a DLL original do
SIM Next, `libt2u.dll`, localizada na pasta `assets`. A conexão por serial também
depende da configuração do serviço P2P. Cadastros de dispositivos, senhas e
configurações pessoais não acompanham o pacote público.

## Sobre o projeto

O SIMNeo é um projeto independente e está em desenvolvimento. A versão 0.26.0
inicia sua distribuição pública, com foco em visualização ao vivo, reprodução
e exportação de gravações em dispositivos Intelbras/Dahua compatíveis.

## Apoie o projeto

Se quiser apoiar o desenvolvimento do SIMNeo, você pode fazer uma doação pelo LivePix.

[Faça uma doação pelo LivePix](https://widget.livepix.gg/embed/6ef25d27-6530-46dc-9e55-a0ef703b6f18)

## Licença

O código do SIMNeo é disponibilizado sob a licença **GNU AGPL-3.0**.
Consulte o arquivo [LICENSE](LICENSE).

A DLL original do SIM Next e as demais bibliotecas de terceiros mantêm suas
próprias licenças. Consulte [THIRD-PARTY.md](THIRD-PARTY.md).
