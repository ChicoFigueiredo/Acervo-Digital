# Esquema do Catálogo (`dag-store`)

Versão do esquema (`PRAGMA user_version`): **8**. Migração publicada nunca é editada; mudanças entram como novo degrau em `MIGRACOES` (`src/lib.rs`). SQLite em WAL, chaves estrangeiras ligadas, tabelas `STRICT`. O worker é o único escritor. Um Catálogo com versão diferente da build é recusado (`CatalogoDesatualizado`) em leitura; em escrita é migrado para a frente.

| Versão | Arquivo | O que traz |
| --- | --- | --- |
| 1 | `0001_inicial.sql` | Esquema da Fase A rev. 2: inventário, varredura, hash, grupos, prioridades, marcações e histórico. |
| 2 | `0002_inventario.sql` | `files.kind` (symlinks e especiais ficam em `files`, nunca em `folders`) e alvo bruto do symlink; `folder_stats.recursive_special_count`; `scan_errors.folder_id` + índice único de idempotência; tabela `volume_stats`; índices de paginação (`idx_files_folder_size`), de hard link (`idx_files_hardlink`) e de alias de diretório (`idx_folders_physical`). |
| 3 | `0003_reconciliacao.sql` | Fast Scan, monitor e volumes offline: `first_run_id` em `files`/`folders` (itens novos e renomeação); `scan_events` ganha `old_path_raw`, `run_id`, `detail` e índices; tabela `monitor_saude` (linha única); visões `v_arquivos` e `v_pastas` com `estado_efetivo` (`indisponivel` = linha ativa de Volume desconectado, sem reescrever a linha). |
| 4 | `0004_retomada_e_confianca.sql` | Retomada automática e Ponto de Confiança (ADR 0003): `scan_runs.parada_deliberada` e `scan_runs.retomada_automatica` (flag "continuar de onde parou"); tabela `ponto_confianca` (por Raiz: `confiavel_em`, `perdido_em`, `motivo_perda`). |
| 5 | `0005_configuracoes.sql` | Tabela `configuracoes` (chave/valor): hoje o paralelismo do Hash Completo (`hash.threads_por_disco`, `hash.max_discos`). |
| 6 | `0006_fila_hash_volume.sql` | `fila_hash.volume_id` e índice `idx_fila_hash_volume` (reserva por volume em ordem de prioridade sem varrer a fila). |
| 7 | `0007_contador_hashes.sql` | Tabela `contadores` e gatilhos que mantêm o total de arquivos com hash (`hashes`) sem `COUNT(*)`; também guarda `hashes_mudancas`, que marca os grupos como desatualizados. |
| 8 | `0008_lixeira.sql` | Lixeira em dois níveis (ADR 0005); ver a seção Lixeira. |
| 9 | `0009_conteineres.sql` | Conteúdo de contêineres (ADR 0006), só acrescenta: `conteineres` (estado e fila persistente da etapa), `membros` (Membros, com BLAKE3, estado e aninhamento por `pai_ordem`), `senhas_conteiner` (senha **cifrada** ligada ao Hash Completo do contêiner), `cofre_senhas` (senhas **cifradas** tentadas automaticamente), `pedidos_servico` (Fila de Serviço: `pedido_senha` e `falha_leitura`; no máximo um aberto por tipo e arquivo) e `grupo_membros_conteiner` (Membros que são cópia de um grupo). Gatilhos em `membros` sobem `contadores.hashes_mudancas`. |

Convenções: caminhos e nomes em `*_raw` (BLOB, bytes exatos do Linux) mais `*_display` (TEXT seguro para exibir). Datas de eventos em segundos Unix; `mtime/ctime/birthtime` em nanossegundos Unix. **NULL = indisponível**, nunca zero inventado (`allocated_bytes`, `birthtime_ns`, `physical_unique_bytes`; hash ausente = sem linha em `hashes`). Enums são `CHECK`.

## Inventário

| Tabela | Propósito e chaves | Cardinalidade / consistência |
| --- | --- | --- |
| `dispositivos` | Dispositivo Físico (disco/mídia). `identificador` único. | 1 dispositivo : N volumes. Cobertura (A-28) compara `device_id`. |
| `volumes` | Sistema de arquivos; `platform_id` (UUID) único; `device_id` FK (SET NULL). | Desconectado continua no Catálogo (`mounted=0`). |
| `mount_points` | Pontos de montagem observados do volume. Único (volume, `path_raw`). | N por volume. |
| `folders` | Pastas. `parent_folder_id` NULL = raiz do volume. Único (volume, `path_raw`). | Árvore por volume; filhos em cascata. `state`: ativo/ausente/removido/indisponivel. |
| `files` | Arquivos (tabela separada de pastas). `folder_id` obrigatório; único (volume, `path_raw`). | `hardlink_group`/`native_dev`/`native_file_id` identificam o Mesmo Arquivo Físico (inode não é identidade imutável). `allocated_bytes` e `birthtime_ns` anuláveis. |
| `file_types`, `classifications` | Tipo/formato e classificação por regra, com confiança 0..1. | `files.mime_id`, `class_id` e `folders.class_id` são FKs SET NULL. |
| `folder_stats` | Agregados recursivos por pasta (1:1 com `folders`). | `aggregate_state` indica se está pendente/parcial/completo/desatualizado; recalculável a partir de `files`. Contagens de pastas filhas saem de `GROUP BY` + `LEFT JOIN` (subconsulta correlacionada por pasta degenerava em quadrático a ~50 mil pastas). |
| `volume_stats` | Totais por Volume da última agregação (pastas, arquivos, especiais, bytes lógicos/alocados/físicos exclusivos). | 1:1 com `volumes`; é a base dos totais do `dag status`. |
| `monitor_saude` | Saúde do monitor (estado, heartbeat, backlog, watches, overflows, última reconciliação). | Linha única (`id=1`), legível por qualquer processo. |

## Varredura e serviço

| Tabela | Propósito | Regras |
| --- | --- | --- |
| `scan_runs` | Uma varredura: modo, raízes (JSON), fase, contadores, checkpoint. | `seen_run_id` em pastas/arquivos aponta a geração que os viu. Só se considera ausente o que não foi visto após reconciliação concluída sem erro impeditivo (spec §12.1); volume desconectado não é remoção. |
| `scan_queue` | Fila persistente de diretórios (lease, tentativas). Único (run, fase, caminho). | Permite retomar após queda de forma idempotente. |
| `scan_events` | Eventos do monitor (inotify etc.) e lacunas (`overflow`, `reconcile_required`). | Índice parcial nos não processados. |
| `scan_errors` | Erros por caminho/fase com severidade. | Em cascata com a execução. |
| `scan_exclusions` | Exclusões configuradas. | |
| `service_state` | Linha única (`id=1`): modo desejado/atual, heartbeat. | Criada pela migração. |
| `catalog_snapshots` | Registro dos backups do Catálogo e resultado da integridade. | |

## Hash, duplicados, prioridade e marcações (Fase A rev. 2)

| Tabela | Propósito e chaves | Regras |
| --- | --- | --- |
| `hashes` | Hash Completo BLAKE3 (32 bytes) por arquivo, PK `file_id`. Guarda a evidência do momento do hash (`size_bytes`, `mtime_ns`, `ctime_ns`, `native_file_id`) e a Verificação de Integridade (`verified_at`, `verification_state` ok/divergente). | Reuso (Fast Scan) só se toda a evidência continuar igual. Índice `(size_bytes, hash)` para achar duplicados. Hard links são lidos uma vez; os demais caminhos copiam o resultado. |
| `fila_hash` | Fila persistente de hash/verificação (estado, prioridade, `bytes_lidos`). 1 linha por arquivo. | Base do progresso, pausa e retomada. O agendador de hash filtra a reserva por volume (`files.volume_id` → `volumes.device_id`) para dar uma sequência de leitura a cada Dispositivo Físico; nenhuma coluna a mais. |
| `grupos_duplicados` | Grupo por (`logical_bytes`, `hash`), único; tamanho > 0 (vazios ficam fora). `copia_principal_file_id` + modo `auto`/`manual`. | Modo `manual` exige cópia principal. Principal automática vem de `prioridades`. |
| `grupo_membros` | Arquivo físico distinto → grupo (PK `file_id`: um arquivo em um grupo). | Mesmo Arquivo Físico entra uma vez só (a aplicação escolhe o representante). |
| `prioridades` | Prioridade de Volume/pasta: `ordem` menor vence; `folder_id` NULL = volume. | Único por volume e por pasta. |
| `marcacoes` | Marcação de Backup/Deleção. `escopo`: `regra_volume`, `regra_pasta`, `manual_arquivo`, `manual_grupo`; `estado` ativa/pendente; `hash_na_marcacao`; `autor` gui/cli/mcp. | CHECK amarra colunas ao escopo. Manual exige hash. Índices únicos: no máximo uma marcação por arquivo (A-30) e uma regra por volume/pasta. Hash mudou ⇒ `pendente` (fora das contas). |
| `marcacoes_historico` | Histórico de cada marcação e mudança de estado (evento, estados, alvo, hash, autor, `em`). | **Somente de acréscimo** (triggers do `bancada-armazem` recusam UPDATE/DELETE). Sem FK, para sobreviver à remoção da marcação ou do arquivo. A aplicação grava a linha na mesma transação da mudança. |

## Lixeira (migração 0008, ADR 0005)

- `files.na_lixeira` (0/1), `files.lixeira_origem_path`/`lixeira_origem_display` (de onde veio) e `files.lixeira_movido_em` (segundos Unix). Quem está Na Lixeira continua uma linha de `files` (o scan reconcilia pela raiz extra `<montagem>/DAG-Lixeira`), mas todo filtro de grupos, Única Cópia, Cobertura, espaço e relatórios exclui `na_lixeira = 1`.
- `descartados`: histórico de quem estava Na Lixeira e sumiu (o usuário a esvaziou): caminhos, hash, bytes, `movido_em`, `descartado_em`. Nunca é apagado pelo DAG.
- O Catálogo só registra o que o agente fez; o único código que toca o disco é `dag-agent/src/lixeira/fs.rs`.

## Consultas de duplicados em SQL

A listagem paginada de grupos, o resumo e o espaço recuperável por Volume/pasta/grupo são calculados **em SQL** (`src/duplicados/agregado.rs`), sem carregar grupos no processo: CTEs agregam as cópias de cada grupo (Marcação efetiva = arquivo > regra da pasta mais específica > regra do Volume) e a página sai por keyset `(espaço recuperável desc, id)` ou `(tamanho desc, id)`. Os `CROSS JOIN` e o `INDEXED BY idx_grupo_membros_grupo` fixam a ordem de junção, porque sem `ANALYZE` o planejador varria `files` inteiro por grupo. As versões que carregam tudo na memória sobrevivem só como referência de teste (`*_referencia`).

## Contêineres, Membros e Fila de Serviço (ADR 0006)

`conteineres` é, ao mesmo tempo, o estado e a fila da etapa "Conteúdo de contêineres" (`pendente`, `lendo`, `lido`, `aguardando_senha`, `aguardando_chaveiro`, `falha`, `nao_conteiner`); `hash_lido` guarda o Hash Completo do momento da leitura, e o contêiner só é relido se o hash mudar. A leitura é tudo-ou-nada por contêiner: ao falhar, cancelar ou receber senha, os Membros parciais são apagados. **Exceção, as Imagens de Disco** (leitor retomável, pode ter centenas de GB): cada lote de Membros é gravado na mesma transação do cursor `retomada_indice` (Membros do nível raiz concluídos), `retomada_ordem` (próxima `ordem`), `retomada_bytes` e `retomada_hash` (Hash Completo da leitura que o gerou). Cancelar, pausar ou uma queda de energia mantém Membros e cursor; a próxima leitura pula os já concluídos sem ler o conteúdo e continua. Com `retomada_hash` diferente do hash atual do arquivo, o cursor vale nada e a leitura recomeça do zero; `concluir` e `apagar_membros` limpam o cursor. Cópias idênticas (mesmo Hash Completo) reaproveitam a lista de Membros com `INSERT ... SELECT`, por isso `membros.pai_ordem` aponta o pai pela `ordem` dentro do mesmo contêiner, não por id.

Membros **contam** em Grupos de Duplicados (`grupo_membros_conteiner`, reconstruída com os grupos), em Única Cópia e na Cobertura (a Marcação de Backup do contêiner vale para seus Membros), mas **nunca** entram no espaço recuperável: as consultas de espaço só olham `grupo_membros`. Um grupo precisa de ao menos um arquivo comum (a Cópia Principal é sempre um arquivo); grupos só de Membros não existem nesta versão. Senhas e a chave de cifra nunca ficam em claro no Catálogo: `senhas_conteiner.cifrada` e `cofre_senhas.cifrada` são `nonce + texto cifrado` (XChaCha20-Poly1305) e a chave fica no keyring do sistema.

## Índices principais

`folders(parent_folder_id)`, `files(folder_id)`, `files(folder_id, logical_bytes DESC, id)`, `files(volume_id, state)`, `files(volume_id, native_dev, native_file_id)`, `files(volume_id, hardlink_group)` parcial, `files(logical_bytes)`, `seen_run_id` e `first_run_id` em pastas e arquivos, `hashes(size_bytes, hash)`, `grupo_membros(grupo_id)`, `fila_hash(state, priority)`, `membros(tamanho, hash)` parcial (hash não nulo), `membros(conteiner_file_id, pai_ordem, caminho_raw)`, `conteineres(estado, volume_id)`, `marcacoes(tipo, estado)`, `scan_events(observed_at DESC, id DESC)`.

## Backup

`Catalogo::copiar_para` usa a API de backup do SQLite (cópia consistente com o WAL, nunca sobrescreve) e `verificar_integridade` roda `PRAGMA integrity_check`.

## Curadoria Bibliográfica (migração 0011, Fase B)

API em `src/biblioteca/` (funções recebem `&Connection`; instantes `agora` vêm do chamador; listas por cursor keyset; contagens por contadores mantidos por gatilhos, chaves `biblio_*` em `contadores`). A Biblioteca Homologada é só leitura: o Catálogo guarda apenas um **espelho** e o que o DAG calculou.

| Tabela | Propósito e chaves | Regras |
| --- | --- | --- |
| `bibliotecas_calibre` | Bibliotecas conhecidas: `caminho` único, `papel` homologada/a_classificar, `library_uuid`, versão do Calibre, `estado`, `perfil_json`, `geracao_espelho`. | Um caminho tem um papel. Várias Homologadas são permitidas (cópias antigas ficam `arquivada`); índice parcial garante **uma só A Classificar ativa** (destino único dos cadastros). |
| `instantaneos_biblioteca` | Cada leitura do `metadata.db`: hash, tamanho, manifesto, resultado da comparação (primeiro/igual/alterado/incompativel). | Índice `(biblioteca, quando DESC)`. |
| `campos_calibre` | Campos padrão/personalizados, `datatype`, `multiplo`, `clonavel`, `motivo`. PK `(biblioteca, rotulo)`. | Substituído por inteiro a cada perfil. |
| `livros_homologados` | Espelho de leitura: `calibre_id`, uuid, título e `titulo_norm`, autores/tags (JSON), `isbn13`, idioma, série, editora, `estante`, `geracao`. Único `(biblioteca, calibre_id)`. | Upsert idempotente por geração; quem não foi visto na geração corrente é removido (`sincronizar_espelho`). |
| `formatos_homologados` | Um arquivo por `(livro, formato)`: caminho, tamanho, mtime, **hash BLAKE3 calculado pelo DAG** e `hash_em`, `file_id` opcional para `files`. | Hash só sobrevive à ressincronização se tamanho e mtime não mudaram. Índice `(hash, tamanho)` parcial: alerta de duplicata exata (CA-B04) em O(log n). `catalogo.rs` liga a `files` por `(volume, path_raw)` e reaproveita `hashes` do Catálogo. |
| `obras`, `edicoes`, `formatos_bibliograficos` | Obra > Edição > Formato. `chave` canônica única em obras e edições; formato único `(edicao, formato, hash)` com `file_id` e `estado`. | "Garantir" = upsert por chave: reprocessar 3x não duplica (CA-B11); estado avançado nunca é rebaixado. |
| `autores_biblio`, `vinculo_autoria` | Autor (`nome_norm` + `desambiguacao` únicos, variantes JSON) e vínculo por edição e papel. | Homônimos distintos usam `desambiguacao`. |
| `jobs_bibliograficos` | Fila persistente: arquivo, motivo novo/alterado/reprocessar, `prioridade`, `enfileirado_em`, `ordem` (coluna gerada), estado, etapa, `checkpoint_json`, tentativas, erro, `versao_regras`. | Índice único parcial: **um job ativo por arquivo**. `ordem = prioridade*3600 - enfileirado_em` (1 ponto por hora de espera, aging); índice `(estado, ordem, id)` entrega o próximo em O(log n). "Pegar" é um `UPDATE ... RETURNING` atômico; órfãos `em_andamento` voltam a `pendente` (`retomar_orfaos`). Contadores `biblio_jobs:<estado>`. |
| `extracoes` | Resultado por `(arquivo, versao_regras)`: estado, `resultado` (JSON, puro ou comprimido), páginas, OCR, tempo, `hash_arquivo`. | Reaproveitada só se o hash do arquivo é o mesmo. |
| `sugestoes` | Campo, valor, lista, confiança, evidências, origem, modelo, `versao_regras`, estado (pendente/aceita/corrigida/rejeitada). Único `(arquivo, campo, origem, modelo, versao)`. | Regravar com o mesmo valor preserva a decisão; valor novo volta a `pendente`. Contador `biblio_sugestoes_pendentes`. |
| `decisoes_humanas` | Alvo, campo, ação aceitar/corrigir/rejeitar, antes/depois, `autor` gui/cli/mcp, `lote_id`, `desfeita_em`. | Nunca apagada: desfazer (por lote ou avulsa) só preenche `desfeita_em` e devolve a sugestão a `pendente` se não restar decisão válida. |
| `inferencias` | Chamadas de modelo: modelo, versão, `hash_prompt`, tokens, tempo, bruto. | Índice `(hash_prompt, modelo, versao)` serve de cache. |
| `candidatos_duplicidade` | Arquivo x alvo (formato homologado, arquivo, edição), tipo exata/mesma_edicao/mesma_obra/incerta, risco, estado, `bytes`. | Só `exata` soma em `bytes_recuperaveis_exatos`; descartado/confirmado não é reaberto por reprocessamento. Índice `(estado, tipo, bytes DESC, id)`. Contador `biblio_candidatos_abertos`. |
| `alertas_biblio` | Tipo (`ja_tem_este_livro`, ...), arquivo, `dados_json`, `visto_em`, `silenciado`, `chave` única opcional (emissão idempotente). | Índice parcial dos não vistos; contador `biblio_alertas_nao_vistos`; resumo por período (WOW-10). |
| `cadastros_a_classificar` | Arquivo/edição na A Classificar, `calibre_id` criado, `chave_idempotencia` = `dag:<hash hex>`, estado. Único `(biblioteca, chave)`. | Só aceita biblioteca A Classificar. |
| `homologacoes` | Aprovação: quem/quando, `antes_json`/`depois_json`, `calibre_id`, `rollback_id`, estado proposta/aprovada/aplicada/falhou/revertida. | Destino precisa ser Homologada; transições validadas (só aprovada é aplicada). O DAG só registra. |
| `backups_biblioteca` | Manifesto, verificação, restauração amostral e cobertura (JSON). | Mínimo: a Cobertura real é a da Fase A. |

Escala (Catálogo de ~20 milhões de arquivos): nada aqui varre `files` ou `hashes`. A ligação usa o índice único `(volume, path_raw)`, o reaproveitamento de hash usa a PK de `hashes`, e a busca de arquivos iguais aos homologados parte dos poucos milhares de formatos e usa `hashes(size_bytes, hash)`.
