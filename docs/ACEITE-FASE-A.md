# Aceite da Fase A (Linux) — critérios CA-A01 a CA-A29

**Data:** 24/11/2026 · **Branch:** `inc2` · Requisitos: [FASE-A-REQUISITOS.md](FASE-A-REQUISITOS.md) e [FASE-A-ESPECIFICACAO.md](FASE-A-ESPECIFICACAO.md) §13.

Legenda: **Atendido** = testes automáticos provam o critério como escrito. **Parcial** = parte provada, parte não (descrita). **Não verificável neste ambiente** = exige ambiente real que o contêiner de teste não tem. Nada foi marcado Atendido por inferência.

## 1. Tabela de critérios

Caminhos de teste são relativos a `crates/`. `ign` = teste `#[ignore]` rodado à mão.

| CA | Critério | Status | Teste(s) que provam | O que falta / observações |
| --- | --- | --- | --- | --- |
| CA-A01 | Inicialização Linux/Bancada | Parcial | `dag-app/tests/gui.rs::desenha_todas_as_paginas_com_catalogo_vazio_e_depois_populado`; `dag-app/src/app.rs::as_oito_abas_estao_presentes_na_ordem`; `dag-agent/tests/agente.rs::diretorio_0700_e_banco_0600` | Janela real não foi aberta (sem display): manual. "Nenhuma escrita fora do diretório do DAG" foi auditado por busca no código (únicas escritas: Catálogo, `.lock`, socket, unidade systemd do usuário, relatório/backup pedido, auditoria do helper), sem teste automático dedicado. |
| CA-A02 | Descoberta de volumes | Atendido | `dag-platform-linux/tests/real.rs::descobre_o_volume_da_raiz_e_marca_proc_como_pseudo`; `dag-platform-linux/src/volumes.rs` (ext4, NVMe, LUKS, dm, USB, NFS, btrfs com fixtures); `dag-agent/tests/reconciliacao.rs::volume_offline_fica_indisponivel_e_volta_ao_reconectar`; `dag-scanner/tests/fast_scan.rs::volume_desconectado_nao_remove_e_reconectar_restaura` | Mídia removível e NFS só por fixtures de `/sys` e mountinfo. |
| CA-A03 | Varredura e persistência | Atendido | `dag-scanner/tests/full_scan.rs::full_scan_confere_contagens_caminhos_hierarquia_tamanhos_e_datas`; `dag-store/tests/esquema.rs::arquivos_e_pastas_sao_tabelas_separadas` | |
| CA-A04 | Links e montagens | Atendido | `full_scan.rs::symlinks_em_laco_e_para_ancestral_terminam_sao_registrados_e_nao_seguidos`, `::hard_links_formam_um_grupo_e_uma_unidade_fisica`; `dag-scanner/tests/montagens.rs::bind_mount_alias_e_bind_de_ancestral_nao_geram_laco_nem_contagem_dupla`; `dag-store/tests/inventario.rs::agregacao_bottom_up_hardlinks_e_semantica_de_null` | `montagens.rs` usa tmpfs/bind reais e é pulado sem root (aqui rodou como root). |
| CA-A05 | Tamanhos | Atendido | `full_scan.rs::arquivo_esparso_tem_alocado_menor_que_logico_e_vazio_tem_zero`; `inventario.rs::alocado_indisponivel_vira_null_e_nunca_zero` | Sistemas de arquivos sem alocação só por simulação (NULL). |
| CA-A06 | MIME e categorias | Parcial | `dag-core/src/classificacao.rs::extensao_enganosa_e_detectada`; `classificacao/assinatura.rs` (assinaturas e divergências); `classificacao/formato.rs` (≥150 formatos); `classificacao/finalidade.rs`; `dag-agent/tests/agente.rs::finalidade_e_gravada_em_classifications` | A detecção por assinatura existe e é testada em `dag-core`, mas **o scan ainda não a usa** (só extensão e finalidade por caminho). Falta ligar a "verificação leve, quando habilitada". |
| CA-A07 | Escalabilidade | Atendido | `dag-agent/tests/escala.rs` (`ign`, §2); `dag-scanner/tests/escala.rs` (`ign`) | 1.000.000 de entradas medidas (§2). |
| CA-A08 | Tree View / Treemap | Parcial | `dag-store/tests/consulta_gui.rs` (paginação por keyset, bases, trilha, "outros"); `dag-app/src/treemap.rs` (área exata, proporções); `dag-app/src/bases.rs`; `dag-app/tests/gui.rs::analise_trata_os_dois_modos_e_desce_um_nivel`, `::explorador_carrega_arvore_sob_demanda_e_arquivos_da_pasta` | Cálculo, bases e carga incremental provados; interação real (zoom, clique) é manual. A ação "Filtros" existe no Ribbon, mas filtros por MIME, idade, tamanho e profundidade não estão provados como consulta. |
| CA-A09 | Fast Scan | Atendido | `dag-scanner/tests/fast_scan.rs::ca_a09_e_ca_a25_adicao_alteracao_renomeacao_remocao`, `::raiz_indisponivel_nao_remove_nada`, `::diretorio_ilegivel_protege_a_subarvore_dele`; `dag-agent/tests/reconciliacao.rs::fast_scan_rename_preserva_id_e_remocao_marca_ausente` | |
| CA-A10 | Monitoramento | Atendido | `dag-monitor/tests/monitor.rs::ca_a10_criacao_edicao_renomeacao_e_remocao_sao_persistidas`; `dag-agent/tests/reconciliacao.rs::ca_a10_a14_monitor_unico_escritor_e_reflete_mudancas`; `dag-app/tests/gui.rs::discos_e_varredura_mostram_o_que_o_worker_gravou` | A atualização visual ao vivo em janela real é manual. |
| CA-A11 | Perda de eventos | Atendido | `dag-monitor/tests/monitor.rs::ca_a11_overflow_sintetico_registra_lacuna_e_reconcilia`, `::limite_de_watches_registra_lacuna_e_a_reconciliacao_periodica_cobre`, `::fila_saturada_vira_lacuna_e_fast_scan` | Overflow é sintético, não provocado no inotify real. |
| CA-A12 | Serviço opcional | Parcial | `dag-platform-linux/tests/servico.rs` (instalação limpa sem serviço, instalar sem habilitar, autostart, parar, remover, linger); `dag-cli/tests/cli.rs::servico_estado_em_instalacao_limpa`; `dag-agent/tests/servidor.rs::monitor_do_agente_roda_apos_o_primeiro_scan_e_cede_a_pedidos` | Os testes usam executor falso de `systemctl`/`loginctl`. systemd real: **Não verificável neste ambiente** (o contêiner não roda systemd). O teste `real_estado_nao_falha` (`ign`) só lê o estado. Fechar a GUI com o serviço ativo é manual. |
| CA-A13 | Permissões | Parcial | `dag-privileged-helper/tests/helper.rs::pasta_protegida_negada_ao_usuario_e_lida_pelo_helper_root`, `::symlink_no_meio_ou_no_fim_e_recusado`, `::helper_nao_modifica_arquivos`; `dag-scanner/tests/elevado.rs::leitor_elevado_preenche_subarvore_negada`; `dag-platform-linux/tests/elevacao.rs`; `dag-agent/src/servico.rs::leitor_so_existe_com_elevacao_pedida` | O diálogo Polkit real (e a garantia de que a senha só vai ao diálogo) é **Não verificável**: sem agente Polkit nem sessão gráfica. `pkexec` é simulado. O helper rodou como root real no contêiner. |
| CA-A14 | Recuperação | Atendido | `full_scan.rs::cancelar_e_retomar_da_o_mesmo_resultado_de_uma_varredura_sem_interrupcao`, `::retomada_apos_queda_abrupta_refaz_diretorios_reservados_sem_duplicar`, `::pausar_persiste_o_estado_e_continuar_termina`; `hash.rs::cancelar_e_retomar_nao_rele_arquivos_concluidos`; `fast_scan.rs::ca_a14_cancelar_no_meio_nao_remove_e_retomar_conclui`; `dag-agent/tests/agente.rs::scan_cancelado_e_retomado_em_outro_processo` | A "queda abrupta" é simulada (lease de fila abandonado), não é `kill -9` no meio do hash. |
| CA-A15 | Segurança de dados | Atendido | `full_scan.rs::varredura_nao_altera_tamanho_datas_nem_atime_de_nenhum_arquivo`; `hash.rs::hash_nao_altera_tamanho_nem_datas`; `dag-privileged-helper/tests/helper.rs::helper_nao_modifica_arquivos`; `dag-agent/tests/duplicados.rs::ca_a23_marcar_so_altera_o_catalogo_e_relatorio_lista_exatamente_as_marcacoes`; `dag-cli/tests/mcp.rs::mcp_scan_full_e_listar_duplicados` (marca e confere conteúdo e mtime) | Watcher sem teste de instantâneo próprio. Desde a ADR 0005 a única escrita sobre arquivos catalogados é mover para a `DAG-Lixeira` do mesmo Volume e de volta: `dag-agent/tests/seguranca_disco.rs` (varredura estática: nenhuma chamada de apagar/renomear/truncar fora de `dag-agent/src/lixeira/fs.rs` e dos arquivos do próprio DAG; `fs.rs` só usa `renameat_with` com `NOREPLACE`), `dag-agent/tests/lixeira.rs` (recusas, volume diferente, inode divergente, destino ocupado), `dag-cli/tests/lixeira.rs` (dupla confirmação), `dag-app/tests/gui.rs::lixeira_dois_dialogos_mover_e_restaurar_de_ponta_a_ponta`. Pendente: elevação Polkit para mover sem permissão de escrita (falha com `permissao_negada`); lixeira não observada pelo monitor (reconcilia no próximo scan); unidade systemd com `ProtectHome=read-only` impede mover a partir do serviço. |
| CA-A16 | Banco | Atendido | `dag-store/tests/esquema.rs::copia_de_banco_wal_tem_mesmas_contagens_e_passa_na_integridade`, `::migracao_em_banco_vazio_cria_todas_as_tabelas`, `::reabrir_e_idempotente`; `inventario.rs::migracao_2_cria_o_que_o_inventario_precisa`; `dag-agent/tests/agente.rs::backup_do_catalogo_passa_na_integridade`; `dag-cli/tests/cli.rs::catalogo_backup_gera_arquivo_integro` | |
| CA-A17 | Observabilidade | Atendido | `dag-agent/tests/agente.rs::status_mostra_execucoes_e_totais`; `dag-store/tests/inventario.rs::problemas_sao_idempotentes_e_paginados`; `dag-cli/tests/cli.rs::scan_fast_eventos_e_status_pelo_cli`; `dag-agent/tests/reconciliacao.rs` (saúde do monitor); `dag-app/src/progresso.rs` | |
| CA-A18 | Hash | Parcial | `dag-scanner/tests/hash.rs::hashes_batem_com_blake3_e_com_b3sum`, `::cancelar_e_retomar_nao_rele_arquivos_concluidos`, `::alterado_durante_a_leitura_nao_e_gravado`; `dag-agent/tests/duplicados.rs::ca_a18_arvore_navegavel_antes_do_fim_do_hash` | O `b3sum` **não está instalado** aqui: a comparação é com a crate oficial `blake3`, e a checagem com o binário é pulada. Retomada testada por cancelamento, não por encerramento abrupto no meio do hash. |
| CA-A19 | Hard links | Atendido | `dag-scanner/tests/hash.rs::hard_links_sao_lidos_uma_vez`; `dag-store/tests/duplicados.rs::ca_a19_tres_hard_links_de_10_mb_nao_formam_grupo_nem_espaco`; `dag-agent/tests/duplicados.rs::full_scan_faz_hash_forma_grupos_e_hard_links_ficam_de_fora` | |
| CA-A20 | Duplicados | Atendido | `dag-store/tests/duplicados.rs::ca_a20_grupos_entre_pastas_volumes_dispositivos_e_offline`; `dag-agent/tests/duplicados.rs::dois_volumes_prioridade_backup_cobertura_e_offline` | Volumes e dispositivos distintos por fixtures de Catálogo. |
| CA-A21 | Cópia Principal | Atendido | `dag-store/tests/duplicados.rs::ca_a21_prioridade_escolhe_ssd_e_troca_manual_persiste`, `::prioridade_por_pasta_vence_a_do_volume`; `dag-agent/tests/duplicados.rs::dois_volumes_prioridade_backup_cobertura_e_offline` | |
| CA-A22 | Backup e Cobertura | Atendido | `dag-store/tests/duplicados.rs::ca_a22_backup_por_regra_cobertura_outro_dispositivo_e_offline`, `::volume_sem_dispositivo_conhecido_nao_da_nem_recebe_cobertura`; `dag-agent/tests/duplicados.rs::dois_volumes_prioridade_backup_cobertura_e_offline` | Dispositivo Físico comum a duas partições: fixtures e resolução dm/partição em `volumes.rs`; dois discos reais não foram usados. |
| CA-A23 | Deleção | Atendido | `dag-agent/tests/duplicados.rs::ca_a23_marcar_so_altera_o_catalogo_e_relatorio_lista_exatamente_as_marcacoes`; `dag-store/tests/duplicados.rs::unica_copia_alerta_em_delecao_manual_e_por_regra`, `::exportacao_csv_e_json_lista_exatamente_as_marcacoes`; `dag-cli/tests/duplicados.rs::scan_duplicados_marcar_relatorio_de_ponta_a_ponta` | |
| CA-A24 | Marcação Pendente | Atendido | `dag-store/tests/duplicados.rs::ca_a24_conteudo_muda_marcacao_vira_pendente_e_sai_das_contas`; `dag-agent/tests/duplicados.rs::ca_a24_conteudo_muda_rescan_torna_marcacao_pendente`; `dag-agent/tests/reconciliacao.rs::ca_a24_a25_fast_scan_torna_marcacao_pendente_e_so_le_o_alterado` | |
| CA-A25 | Fast Scan com hash | Atendido | `dag-agent/tests/reconciliacao.rs::ca_a09_a25_fast_scan_sem_mudancas_nao_le_conteudo_e_mantem_hashes`; `dag-scanner/tests/fast_scan.rs::arvore_inalterada_nao_reescreve_nada_e_nao_le_bytes`; ensaio de escala (§2): 5 MiB lidos de 6,4 GiB | |
| CA-A26 | Verificação de Integridade | Atendido | `dag-scanner/tests/hash.rs::verificacao_detecta_byte_corrompido_com_mtime_restaurado`, `::verificacao_por_raiz_e_hard_link_uma_leitura`; `dag-agent/tests/duplicados.rs::verificar_integridade_pelo_agente` | |
| CA-A27 | Espaço recuperável | Atendido | `dag-store/tests/duplicados.rs::ca_a27_espaco_recuperavel_bate_com_o_calculo_manual`, `::consultas_sql_equivalem_as_de_referencia_em_memoria`, `::relatorio_de_espaco_por_grupo_pagina_sem_pular_nem_repetir` | Limitações (reflink, COW, compressão) listadas em `LIMITACOES_ESPACO`. |
| CA-A28 | CLI e MCP | Atendido | `dag-cli/tests/duplicados.rs::scan_duplicados_marcar_relatorio_de_ponta_a_ponta` (CLI); `dag-cli/tests/mcp.rs::mcp_scan_full_e_listar_duplicados` (MCP: scan, duplicados, marcação, relatório); `dag-mcp/tests/protocolo.rs`; `dag-agent/tests/servidor.rs::com_o_agente_de_pe_nao_existe_segundo_escritor`, `::escritas_concorrentes_de_varios_clientes_sao_serializadas`; `dag-app/src/worker.rs::worker_conecta_ao_servico_sem_segundo_escritor...`; `dag-agent/tests/agente.rs::segundo_escritor_recebe_resposta_de_erro_clara` | Paridade GUI/CLI/MCP vem de os três enviarem o mesmo `Comando`. O monitoramento contínuo é só CLI (documentado). |
| CA-A29 | Escala com hash | Atendido | `dag-agent/tests/escala.rs` (`ign`) + `dag-store/tests/duplicados.rs::listagem_em_sql_aguenta_muitos_grupos_sem_alterar_a_semantica` | Consultas de duplicados em < 1 s após a correção (§2). |

**Contagem (29 critérios):** Atendido **23** · Parcial **6** (CA-A01, A06, A08, A12, A13, A18) · Não verificável neste ambiente **0** critérios inteiros (as partes não verificáveis, systemd e Polkit reais, estão dentro de A12 e A13).

## 2. Ensaio de escala (CA-A07 + CA-A29)

Código: `crates/dag-agent/tests/escala.rs`, fases `escala_gerar`, `escala_full`, `escala_consultas`, `escala_fast` (comandos no cabeçalho do arquivo), uma por processo para que `VmHWM` seja o da fase. O ensaio passa pelo agente: Full Scan, hash, Grupos de Duplicados, Fast Scan e consultas.

**Ambiente:** Intel Xeon 2,1 GHz, 4 CPUs, 16 GiB de RAM, kernel 6.18.44, ext4 em disco virtual `/dev/vda` (252 GB, 27 GB livres), rustc 1.98.1, `--release`. O cache de páginas foi esvaziado (`drop_caches=3`) antes do Full Scan, então a leitura veio do disco.

**Acervo sintético:** 1.000.000 de entradas (50.000 pastas, 950.000 arquivos), profundidade até 8, 6,44 GiB de conteúdo (média ~7 KB; ~2% vazios, maioria < 4 KB, cauda até ~1 MiB), ~5% de arquivos com conteúdo duplicado em outra pasta, 4.749 hard links.

| Etapa | Tempo | Vazão | Pico de memória (`VmHWM`) | Catálogo |
| --- | --- | --- | --- | --- |
| Full Scan, inventário e agregação | 55,6 s | ~18.000 entradas/s | | |
| Hash Completo (950.000 arquivos, 6.558 MiB lidos) | 229,2 s | ~4.100 arquivos/s, 30 MB/s | | |
| Classificação, Grupos de Duplicados e reavaliação | 15,3 s | 44.074 grupos, 90.619 cópias, 326 MiB recuperáveis | | |
| **Full Scan total** | **300 s** | | **40 MiB** | 946 MiB (+68 MiB de WAL) |
| Fast Scan, ~0,12% de mudanças (500 alteradas, 250 removidas, 250 renomeadas, 250 novas) | 32,0 s total: reconciliação 19,0 s (~52.000 entradas/s), hash 1,2 s, grupos 11,7 s | releu 753 arquivos, 6 MiB; reaproveitou 948.998 | **53 MiB** | 970 MiB |

A vazão do hash neste acervo é limitada pelo custo por arquivo (abrir, ler ~7 KB frios, gravar a linha), não pela banda: em arquivos grandes e quentes no cache `vazao_do_hash` (`ign`) mede 3,5 GB/s com 1 leitor e 13 GB/s com 4.

**Latência de consultas** (processo novo sobre o Catálogo de 1 milhão de entradas; 3 a 5 repetições; RSS 12 MiB no início):

| Consulta | Mediana | Observação |
| --- | --- | --- |
| `explorar` raízes | 47 ms | |
| `explorar` raiz (página de 100) / 2ª página | 1,7 ms / 1,5 ms | keyset |
| `explorar` pasta profunda (nível 8) | 0,2 ms | |
| `listar_duplicados` 1ª / 2ª página (50) | 410 ms / 415 ms | 44.002 grupos |
| `listar_duplicados` filtro por Volume + tamanho mínimo | 125 ms | |
| `resumo_duplicados` (totais) | 255 ms | |
| Relatório de espaço por Volume | 278 ms | |
| Relatório de espaço por pasta (1ª página) | 740 ms | carrega o mapa pasta→pai (memória proporcional às pastas) |
| `status` | 0,1 ms | |

Crescimento do `VmHWM` durante todas as consultas: 12 → 28 MiB (cache de páginas do SQLite e mapa de pastas), sem relação com o número de grupos.

### Defeitos de escala achados e corrigidos

1. **Agregação do inventário quadrática** (`dag-store/src/inventario/agregacao.rs`): a contagem de pastas filhas era uma subconsulta correlacionada por pasta e o planejador, sem `ANALYZE`, usava o índice de `seen_run_id`. Com 50.000 pastas o Full Scan não terminava (a etapa passou de 25 min sem escrever). Agora é um `GROUP BY` unido por `LEFT JOIN`: o inventário completo leva 55 s.
2. **Duplicados em memória** (`listar_grupos`, `resumo_duplicados`, `espaco_de_grupos`/relatório de espaço): carregavam todos os grupos, com memória e tempo proporcionais ao número de grupos. Medido na versão antiga (mantida como `*_referencia` para teste): **4,3 a 4,8 s** por consulta a 44.000 grupos. Agora são consultas SQL agregadas com keyset (`dag-store/src/duplicados/agregado.rs`); a API pública não mudou. Testes de equivalência com a referência cobrem Marcação de arquivo, grupo, pasta e Volume, Pendente, offline, Cópia Principal manual, hard links e o caso "poupa uma cópia".
3. **Reconstrução de grupos com grupos existentes** (`reconstruir_grupos`): a tabela temporária `_alvo` não tinha índice por (tamanho, hash) e o Fast Scan levava 54 s só nessa etapa (agora 11,7 s, com o índice).

Pendências de escala conhecidas, fora do escopo pedido: a reconstrução dos grupos é completa a cada scan (poderia usar `Escopo::Hashes`); os relatórios de Cobertura e sem Cobertura percorrem os arquivos (streaming, memória limitada, mas tempo linear); o relatório por pasta guarda o mapa pasta→pai.

## 3. O que continua manual

- **Janela real da GUI** (Ribbon, Treemap interativo, atualização ao vivo): sem display no ambiente de teste.
- **systemd real** (instalar, autostart, linger, o monitor continuar após fechar a GUI).
- **Diálogo Polkit real** com `pkexec` e a garantia de que a senha só é digitada ali.
- **NFS/CIFS reais** e mídia removível física (só fixtures).
- **Dois discos físicos reais** para Cobertura entre Dispositivos Físicos.
- **`b3sum` do sistema** como referência externa do hash.
- **Windows e macOS: fora de escopo**, além do contrato.

## 4. A-22: fronteira de plataforma

Antes desta revisão não havia nenhuma abstração de plataforma (só `FonteSistema`, interna ao crate Linux). Foi acrescentada a trait `Plataforma` em `dag-core::plataforma` (`sistema()`, `mecanismos()` com monitor, serviço e elevação nativos, e `descobrir_volumes()`), implementada por `PlataformaLinux` em `dag-platform-linux` e usada pelo agente para descobrir Volumes. Windows (UAC, `ReadDirectoryChangesW`) e macOS (FSEvents, helper) entram como novos crates que implementam a trait; **nenhum deles está implementado nem simulado**. Teste: `dag-platform-linux/src/plataforma.rs::contrato_linux_descreve_os_mecanismos_e_descobre_volumes`. Serviço e elevação ainda são módulos do crate Linux, fora da trait: ampliá-la é trabalho para quando existir uma segunda plataforma. O A-22 fica **Atendido como contrato mínimo**.

## 5. Pendências de decisão

- **A-32 (pastas clones):** pendente de decisão do produto; não implementado.
- Assinatura de conteúdo no scan (CA-A06): decidir se entra como opção do Full Scan.
