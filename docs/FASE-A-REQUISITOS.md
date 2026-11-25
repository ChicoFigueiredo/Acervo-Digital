# Fase A — Requisitos e critérios de aceite (revisão 2)

**Data:** 23/11/2026 · **Base:** [especificação v1.0](FASE-A-ESPECIFICACAO.md) + decisões do grilling · Vocabulário em [CONTEXT.md](../CONTEXT.md) · Decisões em [docs/adr/](adr/)

A Fase A passa a entregar a especificação v1.0 inteira **mais** o escopo da antiga Fase B (ADR 0001) e um servidor MCP (ADR 0002). Onde este documento e a v1.0 divergem, este prevalece.

## 1. O que muda em relação à v1.0

| Tema | v1.0 | Revisão 2 |
| --- | --- | --- |
| Hash | Proibido como obrigação na Fase A | Hash Completo BLAKE3 de **todos** os arquivos, em etapa própria do Full Scan |
| Duplicados | Fase B | Grupos de Duplicados, pastas clones (a confirmar) e espaço recuperável na Fase A |
| Marcações | Não existiam | Marcação de Backup, Marcação para Deleção, Cobertura, Cópia Principal |
| Somente leitura | Total | O DAG nunca apaga nem altera arquivos. Única escrita: mover cópia com Marcação para Deleção para a `DAG-Lixeira` do mesmo Volume e de volta (ADR 0005) |
| Inventário na nuvem | Nunca | Exceção: cliente MCP configurado pelo usuário (ADR 0002) |
| Interfaces | GUI Bancada | GUI Bancada + CLI `dag` + MCP `dag mcp`, todas sobre o mesmo núcleo |

## 2. Requisitos mantidos da v1.0

A-01 a A-22 continuam obrigatórios como escritos na v1.0, com dois ajustes:
- **A-03** ganha a etapa "Hash" entre classificação e agregação, executada após o inventário, com fila persistente própria. O hash roda **um grupo de leitores por Dispositivo Físico** (padrão 1 thread por disco, até 16 discos ao mesmo tempo; volumes do mesmo disco ficam em sequência, volumes sem dispositivo conhecido contam como um disco cada), com um único escritor do Catálogo. `dag status` mostra a etapa de hash (arquivos e bytes feitos/pendentes, por disco, vazão recente) lida da fila persistente; sem mudança de esquema, uma execução parada no hash retoma com `dag scan --retomar <id>` e a fila é refeita a partir dos hashes já gravados.
- **A-12** passa a reaproveitar também o Hash Completo de arquivos inalterados (mesmo inode, tamanho, mtime e ctime).

## 3. Requisitos novos

| ID | Requisito |
| --- | --- |
| A-23 | Calcular Hash Completo BLAKE3 de todo arquivo regular do escopo, numa etapa posterior ao inventário, com fila persistente, progresso próprio (bytes lidos, taxa, restante), pausa, retomada e limites de I/O. Árvore e tamanhos ficam utilizáveis antes do fim do hash. |
| A-24 | Caminhos do Mesmo Arquivo Físico (hard links) são lidos e hasheados uma vez e nunca formam Grupo de Duplicados. |
| A-25 | Formar Grupos de Duplicados por (tamanho, Hash Completo) entre arquivos físicos distintos, entre volumes e dispositivos, incluindo volumes desconectados com o último hash conhecido. Arquivos vazios ficam fora. |
| A-26 | Escolher a Cópia Principal de cada grupo pela Prioridade de Volume/pasta definida pelo usuário, com troca manual por grupo. |
| A-27 | Marcação de Backup por regra (Volume ou pasta inteira) e manual (arquivo ou grupo). A cópia marcada sai do espaço recuperável. |
| A-28 | Cobertura: indicar, por arquivo, pasta e volume, quais arquivos têm cópia com Marcação de Backup em outro **Dispositivo Físico**; relatório de arquivos sem Cobertura. Cópias em volumes desconectados mantêm Cobertura com selo "não verificada desde <data>". |
| A-29 | Marcação para Deleção em qualquer arquivo, manual ou por grupo, somente no Catálogo. Arquivo sem outra cópia recebe alerta de Única Cópia. Relatório exportável (CSV/JSON) das marcações. O DAG nunca apaga nem altera arquivos. Lixeira em dois níveis (ADR [0005](adr/0005-lixeira-em-dois-niveis.md)): a cópia com Marcação para Deleção pode ser movida para a `DAG-Lixeira` na raiz do próprio Volume (`renameat2` com `RENAME_NOREPLACE`, mesmo sistema de arquivos, nunca entre Volumes) e restaurada; fica **Na Lixeira** e fora de grupos, Única Cópia, Cobertura e espaço recuperável; quando o usuário a esvazia, o registro vira **Descartado**. Mover exige duas confirmações (GUI: dois diálogos; CLI `dag lixeira mover`: pré-visualização e duas confirmações; MCP `mover_para_lixeira`: `confirmacao_1` e `confirmacao_2`). Única Cópia só vai com aceite explícito; pasta só vai se todo arquivo está marcado (`marcar_tudo` é passo à parte). Hard links e arquivos cujo inode, tamanho ou mtime divergem do Catálogo são recusados. A elevação por Polkit para mover onde o usuário não tem escrita está **pendente**: hoje a operação falha com `permissao_negada` e nada é movido. |
| A-30 | Uma cópia tem no máximo uma marcação. Se o Hash Completo mudar após a marcação, ela vira Marcação Pendente e sai de todas as contas até nova confirmação. Toda marcação e mudança de estado fica no histórico com autor (GUI, CLI ou MCP) e data. |
| A-31 | Espaço recuperável por grupo, pasta, volume e computador = bytes das cópias com Marcação para Deleção, mais estimativa das cópias redundantes sem marcação, descontando Marcação de Backup e Mesmo Arquivo Físico; indicar limitações (reflinks, COW, compressão). |
| A-32 | Pastas clones: detectar pastas cujo conteúdo recursivo é idêntico ao de outra pasta. *(Pendente de decisão: não implementado; a confirmação do produto ainda não saiu. Ver [ACEITE-FASE-A.md](ACEITE-FASE-A.md).)* |
| A-33 | Verificação de Integridade agendável por volume/raiz: relê o conteúdo, compara com o hash guardado, registra divergências como possível corrupção, sem alterar o arquivo. |
| A-34 | CLI `dag` com todas as operações: volumes, raízes, Full/Fast Scan, pausar/retomar/cancelar, status, consultas, duplicados, marcações, prioridade de volumes, relatórios, verificação, serviço e backup do Catálogo. Saída legível e `--json`. |
| A-35 | Servidor MCP por stdio (`dag mcp`) expondo todas as operações da CLI, inclusive instalar/remover serviço e pedidos Polkit; repassa tudo ao worker, que segue como único escritor. Senha sempre no diálogo do sistema, nunca pelo MCP. |
| A-36 | GUI ganha a página **Duplicados** (grupos, Cópia Principal, marcações, Cobertura, espaço recuperável) e indicadores de hash/marcação no Explorador. |
| A-37 | Execução Interrompida e Ponto de Confiança (ADR [0003](adr/0003-retomada-automatica-e-ponto-de-confianca.md)). Ao abrir como escritor (agente em segundo plano ou GUI), o DAG detecta execuções que um processo morto deixou em andamento (inventário ou hash), marca-as `interrompida` com a flag "continuar de onde parou" e as retoma sozinho, sem reler o que já foi gravado; a retomada automática é configurável (padrão ligada). Depois da retomada, repete o Fast Scan nas mesmas Raízes até uma passada sem mudanças (teto de 3). Com o monitor ativo, a Raiz que estabilizou entra no Ponto de Confiança; lacuna do monitor ou encerramento não limpo a tira. Ao abrir, toda Raiz disponível passa por Fast Scan e só o conteúdo novo ou alterado é hasheado (padrão configurável: sempre; só as fora do Ponto de Confiança; nunca). Hash de tudo só por pedido explícito (Full Scan). `dag status`, GUI e MCP mostram "interrompida, retomando" e o Ponto de Confiança por Raiz. |

## 4. Critérios de aceite

CA-A01 a CA-A17 da v1.0 continuam valendo. CA-A15 passa a incluir: "marcações não alteram arquivos; nenhum comando da GUI, CLI ou MCP apaga ou altera arquivo; a única escrita permitida é mover cópia com Marcação para Deleção para a `DAG-Lixeira` do próprio Volume e de volta (ADR 0005), por `rename` sem sobrescrever, com dupla confirmação e sem nunca cruzar Volumes". Teste estático: `dag-agent/tests/seguranca_disco.rs`.

**CA-A18 — Hash:** num conjunto de teste com conteúdos conhecidos, todo arquivo regular tem BLAKE3 igual ao da referência `b3sum`; a árvore é navegável antes do fim da etapa de hash; pausar, encerrar abruptamente e retomar não refaz arquivos já hasheados nem deixa hash parcial gravado.

**CA-A19 — Hard links:** 3 hard links de um arquivo de 10 MB são lidos uma vez, não formam grupo e não contam espaço recuperável.

**CA-A20 — Duplicados:** arquivos idênticos em pastas, volumes e dispositivos diferentes formam um grupo; arquivos de mesmo tamanho e conteúdo diferente não; arquivos vazios ficam fora; um volume desconectado mantém seus membros no grupo com estado offline.

**CA-A21 — Cópia Principal:** com prioridade "SSD > HD-Externo", todos os grupos com cópias nos dois escolhem a do SSD; uma troca manual persiste após Fast Scan.

**CA-A22 — Backup e Cobertura:** marcar o volume HD-Externo como backup dá Cobertura aos arquivos do SSD com cópia lá; cópia em outra partição do mesmo disco não dá Cobertura; desconectar o HD mantém Cobertura com selo de data.

**CA-A23 — Deleção:** marcar uma cópia para deleção altera só o Catálogo (verificado por hash e mtime do arquivo antes/depois); arquivo sem duplicado recebe alerta de Única Cópia; o relatório exportado lista exatamente as marcações.

**CA-A24 — Marcação Pendente:** alterar o conteúdo de um arquivo marcado e rodar Fast Scan torna a marcação pendente, retira-a das contas de espaço e Cobertura e registra no histórico.

**CA-A25 — Fast Scan com hash:** arquivos inalterados não são relidos (verificado por contador de bytes lidos); arquivos novos ou alterados são rehasheados.

**CA-A26 — Verificação de Integridade:** corromper um byte de um arquivo sem mudar mtime (via teste controlado) e rodar a verificação aponta divergência; o arquivo não é alterado.

**CA-A27 — Espaço recuperável:** os totais por grupo, pasta e volume batem com o cálculo manual no conjunto de teste, descontando backup e hard links.

**CA-A28 — CLI e MCP:** toda operação da GUI tem equivalente na CLI e no MCP; um teste de integração percorre scan → duplicados → marcação → relatório pela CLI e pelo MCP; com GUI e MCP abertos ao mesmo tempo existe um único escritor.

**CA-A29 — Escala com hash:** o ensaio de 1 milhão de entradas (CA-A07) inclui a etapa de hash e o agrupamento, com memória limitada e consultas de duplicados paginadas.

**CA-A30 — Retomada e Ponto de Confiança:** matar o processo no meio do inventário e no meio do hash e reabrir marca a execução `interrompida`, retoma de onde parou sem reler arquivo já hasheado e termina com o mesmo Catálogo de uma execução sem queda; arquivos criados enquanto o programa estava fora aparecem na passada de alcance; uma passada sem mudanças marca a Raiz como confiável e uma lacuna do monitor (ou encerramento não limpo) a desmarca; a verificação ao abrir hasheia só arquivos novos ou alterados; Full Scan com hash relê todos.

## 5. Incrementos (um PR cada)

1. **Fundação:** workspace Rust, Bancada por git fixado em commit, esquema SQLite e migrações, CLI e GUI vazias, CI.
2. **Inventário:** volumes, dispositivos físicos, Full Scan (descoberta, metadados, classificação, agregados), links e montagens.
3. **Hash e duplicados:** etapa de hash, grupos, Cópia Principal, marcações, Cobertura, espaço recuperável, relatórios.
4. **Exploração:** Tree View, Treemap, página Duplicados, filtros.
5. **Atualização:** Fast Scan com reuso de hash, watcher, Marcação Pendente, Verificação de Integridade, volumes offline.
6. **Operação:** MCP completo, serviço opcional, Polkit/helper, backup do Catálogo, resiliência e ensaio de 1 milhão.
