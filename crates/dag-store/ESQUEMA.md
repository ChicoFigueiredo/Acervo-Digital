# Esquema do Catálogo (`dag-store`)

Versão do esquema (`PRAGMA user_version`): **4**. Migração publicada nunca é editada; mudanças entram como novo degrau em `MIGRACOES` (`src/lib.rs`). SQLite em WAL, chaves estrangeiras ligadas, tabelas `STRICT`. O worker é o único escritor. Um Catálogo com versão diferente da build é recusado (`CatalogoDesatualizado`) em leitura; em escrita é migrado para a frente.

| Versão | Arquivo | O que traz |
| --- | --- | --- |
| 1 | `0001_inicial.sql` | Esquema da Fase A rev. 2: inventário, varredura, hash, grupos, prioridades, marcações e histórico. |
| 2 | `0002_inventario.sql` | `files.kind` (symlinks e especiais ficam em `files`, nunca em `folders`) e alvo bruto do symlink; `folder_stats.recursive_special_count`; `scan_errors.folder_id` + índice único de idempotência; tabela `volume_stats`; índices de paginação (`idx_files_folder_size`), de hard link (`idx_files_hardlink`) e de alias de diretório (`idx_folders_physical`). |
| 3 | `0003_reconciliacao.sql` | Fast Scan, monitor e volumes offline: `first_run_id` em `files`/`folders` (itens novos e renomeação); `scan_events` ganha `old_path_raw`, `run_id`, `detail` e índices; tabela `monitor_saude` (linha única); visões `v_arquivos` e `v_pastas` com `estado_efetivo` (`indisponivel` = linha ativa de Volume desconectado, sem reescrever a linha). |
| 4 | `0004_retomada_e_confianca.sql` | Retomada automática e Ponto de Confiança (ADR 0003): `scan_runs.parada_deliberada` e `scan_runs.retomada_automatica` (flag "continuar de onde parou"); tabela `ponto_confianca` (por Raiz: `confiavel_em`, `perdido_em`, `motivo_perda`). |

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

## Índices principais

`folders(parent_folder_id)`, `files(folder_id)`, `files(folder_id, logical_bytes DESC, id)`, `files(volume_id, state)`, `files(volume_id, native_dev, native_file_id)`, `files(volume_id, hardlink_group)` parcial, `files(logical_bytes)`, `seen_run_id` e `first_run_id` em pastas e arquivos, `hashes(size_bytes, hash)`, `grupo_membros(grupo_id)`, `fila_hash(state, priority)`, `marcacoes(tipo, estado)`, `scan_events(observed_at DESC, id DESC)`.

## Backup

`Catalogo::copiar_para` usa a API de backup do SQLite (cópia consistente com o WAL, nunca sobrescreve) e `verificar_integridade` roda `PRAGMA integrity_check`.
