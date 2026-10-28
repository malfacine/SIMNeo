# recuperação e manutenção da sessão VDT

Referência local: `assets/libt2u.dll`, SHA-256
`528b8289c6f59e447c91508790d21abe888ad16ee1b1f6e30a52c4799bbf4d02`.
As posições abaixo são endereços virtuais dessa versão; não são uma API pública.
Nenhum payload de vídeo, serial ou credencial é necessário para os testes.

- `0x18000bb70`: ACK `0x1003`, com janela uint16 em +4, quantidade de
  sequências ausentes uint16 em +6, sequência recebida em +8 e lista uint32
  a partir de +16, todos em ordem de rede.
- `0x18000cf0c`: pedido de ACK `0x1004`; o receptor guarda a sequência em +4 e
  sinaliza o envio de confirmação. `0x18000ba00` inclui na lista também a cauda
  ausente até essa sequência. Ignorar esse pacote impede recuperar uma cauda
  perdida quando o remetente aguarda confirmação antes de enviar mais dados.
- `0x18000ce5a`: byte +11 do pacote de dados indica retransmissão e alimenta
  uma estatística. Não é um campo reservado que precise ser zero.
- `0x180014570`: heartbeat de sinalização `2002`, 24 bytes, com sequência
  incremental, versão 121 e indicador de conexão 1. Não é fechamento.
- `0x18000be59` e `0x18000c0c0`: tipo VDT `0x1000` é negociação; os uint16 em
  +16/+18 anunciam a janela. O receptor usa +16 para configurar o limite de
  envio, com limite inicial de 20 pacotes na referência. Anunciar 1 e repetir
  esse pacote depois da abertura restringe o envio continuamente. O Linux
  negocia a capacidade do buffer apenas durante a abertura; depois mantém a
  sessão com `2002` e a renovação do coordenador.
- `0x180012350`: resposta `1013` confirma autenticação e informa o intervalo
  para renovação com `1012`. A renovação continua depois da abertura do túnel.
- A captura de referência confirma fechamento da aplicação seguido de FIN
  VDT `0x1007` com 12 bytes, menor que o cabeçalho de dados.

A implementação Linux entrega somente bytes contíguos. Dados recebidos fora de
ordem são descartados e solicitados novamente, incluindo a própria sequência
descartada. A lista é limitada a 256 entradas por ACK; a janela anunciada é
limitada pela capacidade do buffer local. Não há alocação ou iteração
proporcional a um número de sequência arbitrário recebido pela rede.
O buffer de recepção tem 512 KiB, permitindo anunciar até 256 datagramas de
2048 bytes, sem ampliar o limite público de escrita de 64 KiB. O limite anterior
de 32 datagramas restringia a quantidade de dados em trânsito em uma conexão
com latência. O teste de pressão retém 160 KiB durante uma escrita aguardando
ACK e confirma depois a integridade dos bytes recebidos.

`tests/test_vdt.py` reproduz perda da cauda com pedido de ACK, reordenação,
contador de retransmissão diferente de zero, número de sequência extremo,
renovação/revogação da autenticação e encerramento do transporte. Esses testes
falharam nas condições correspondentes antes das correções.
