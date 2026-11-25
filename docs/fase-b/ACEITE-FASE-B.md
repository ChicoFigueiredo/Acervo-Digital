# Aceite da Fase B (Curadoria Bibliográfica) — B-01 a B-26, CA-B01 a CA-B24, WOW-01 a WOW-10 e metas do piloto

**Data:** 25/11/2026 · **Branch:** `worktree-agent-a40d6e9750d540563` (com `claude/project-thread-erur7s` mesclada) · **Commit base:** `8454ba3` · Requisitos: [REQUISITOS.md](REQUISITOS.md), [CRITERIOS-ACEITE.md](CRITERIOS-ACEITE.md), [CRITERIOS-SURPRESA.md](CRITERIOS-SURPRESA.md), [ARQUITETURA-PLANO.md](ARQUITETURA-PLANO.md), [PROTOCOLO-BIBLIOTECA.md](PROTOCOLO-BIBLIOTECA.md), [GUIA-OPERACAO.md](GUIA-OPERACAO.md) e ADR [0008](../adr/0008-curadoria-bibliografica-calibre.md).

Legenda:
- **Atendido**: há teste automático que prova o critério como escrito, e não há medição real que o contradiga.
- **Parcial**: parte provada por teste, parte não implementada ou não provada (a parte está descrita).
- **Depende de validação real**: o critério exige Calibre 9.14 real, GPU real, biblioteca real do Chico ou demonstração manual. Teste simulado não basta, então não foi declarado aprovado.
- **Não atendido**: não implementado ou explicitamente fora do escopo atual.

**Método.** Os testes foram localizados com busca nos arquivos `crates/**/tests/*.rs` e nos `#[test]` de `crates/**/src`. Cada nome citado foi conferido na árvore desta branch. A suíte não foi reexecutada para este documento (ver "Como reproduzir"). As medições reais vêm de [MEDICOES-CEREBRO.md](/home/user/briefs/MEDICOES-CEREBRO.md) (25/11/2026, computador do Chico: Ubuntu, GPU de 16 GB, Calibre 9.14, Ollama 0.33.3). Nada foi medido na biblioteca homologada além da leitura registrada lá.

Abreviações de caminho (todos em `crates/`):

| Sigla | Arquivo |
| --- | --- |
| `ag` | `dag-agent/tests/biblioteca.rs` |
| `st` | `dag-store/tests/biblioteca.rs` |
| `cli` | `dag-cli/tests/biblioteca.rs` |
| `mcp` | `dag-mcp/tests/protocolo.rs` |
| `gui` | `dag-app/tests/gui.rs` |
| `cal` | `dag-calibre/src/testes/` (`t_calibredb.rs` usa executor simulado `ExecutorFalso`; `t_real.rs` usa o `calibredb` real e sai sem falhar, com aviso, se ele não existir; `t_leitura.rs`; `t_trava_instantaneo.rs`) |
| `ia` | `dag-biblio-ia/tests/` (`classificacao.rs`, `servidor.rs`, `benchmark.rs`; servidores HTTP falsos) |
| `pdf` | `dag-biblio-pdf/tests/pdf.rs` e `djvu.rs` (pulam com aviso sem `tesseract`, `poppler` ou `djvulibre`) |
| `ebk` | `dag-biblio-ebook/tests/` (`epub.rs`, `mobi_fb2.rs`, `compactados.rs`) |
| `dom` | `#[test]` em `dag-biblio/src/*.rs` (`identidade.rs`, `duplicidade.rs`, `descarte.rs`, `espaco.rs`, `prioridade.rs`, `reconciliacao.rs`, `vocabulario.rs`, `taxonomia.rs`, `normalizacao.rs`, `autores.rs`, `isbn.rs`) |

## 0. Resumo por estado

| Conjunto | Atendido | Parcial | Depende de validação real | Não atendido | Total |
| --- | --- | --- | --- | --- | --- |
| Requisitos B-01..B-26 (P0 e P1) | 15 | 7 | 2 | 2 | 26 |
| Critérios CA-B01..CA-B24 | 13 | 4 | 6 | 1 | 24 |
| Surpresa WOW-01..WOW-10 | 0 | 9 | 1 | 0 | 10 |
| Metas de qualidade do piloto | 1 | 2 | 5 | 0 | 8 |

Nenhum item foi marcado Atendido por inferência. Onde o teste cobre só a lógica, a linha diz isso.

## 1. Requisitos B-01 a B-26

| Item | Estado | Evidência | O que falta |
| --- | --- | --- | --- |
| B-01 Conectar Homologada em leitura e extrair inventário | Atendido | `ag::ca_b01_sincronizar_nao_altera_a_homologada`; `cal::t_leitura::conexao_e_somente_leitura`, `::ler_nao_altera_nada`, `::perfil_estantes_tags_de_controle_e_compatibilidade`; `st::bibliotecas_papeis_e_perfil`. Medição real: leitura da Homologada do Chico, `sha256` do `metadata.db` inalterado (MEDICOES). | Relatório de compatibilidade gravado para o acervo do Chico (o perfil existe; não foi exportado para documento). |
| B-02 Clonar estrutura para A Classificar, com diferenças explícitas | Parcial | `cal::t_real::real_leitura_perfil_clone_round_trip_idempotencia_e_recusas` (testa `clonar_estrutura`, `dag-calibre/src/calibredb.rs:790`); lista `nao_clonadas` testada em `t_leitura::perfil_estantes_tags_de_controle_e_compatibilidade`. Medição: clone de cópia em 2,1 s, colunas renumeradas (mapear por label). | `clonar_estrutura` não é chamado pelo registro da A Classificar (`dag-agent/src/biblioteca/bibliotecas.rs`: cria a biblioteca vazia e só chama `garantir_colunas_dag`). Falta ligar o clone ao registro e testar no fluxo do agente. |
| B-03 Comparar inventário DAG com formatos e hashes da Homologada | Atendido | `st::ligacao_com_o_catalogo_reusa_hashes_e_acha_iguais`; `st::espelho_idempotente_remove_ausentes_e_preserva_hash_valido` (preserva hash com tamanho e mtime iguais); `ag::ca_b04_duplicata_exata_sem_extracao_nem_ocr`. Medição: 4.342 formatos da Homologada, todos com hash no Catálogo; comparação completa em 63 s. | Nenhum no mecanismo. |
| B-04 Alerta de novo arquivo com hash homologado | Atendido | `ag::ca_b04_duplicata_exata_sem_extracao_nem_ocr`; `dom::duplicidade.rs::exata_por_hash_e_tamanho_e_unica_com_espaco`, `::livro_repetido_na_homologada_e_alerta_proprio`; `st::alertas_idempotentes_vistos_e_resumo`. Medição: 11.440 cópias de 4.297 formatos fora da Homologada. | Alerta disparado por download real (ver WOW-01). |
| B-05 Fila persistente alimentada pelo watcher, com compactados e limites | Atendido | `st::fila_nao_duplica_e_respeita_prioridade_com_aging`; `st::job_orfao_volta_a_pendente_com_checkpoint_e_ciclo_de_vida`; `ag::ca_b12_part_debounce_e_conteudo_alterado`; `ag::compactado_com_dois_livros_gera_dois_itens_ligados_ao_conteiner`; `ebk::compactados.rs::zip_bomb_e_rejeitada_pela_razao_de_expansao_sem_descompactar`, `::bomba_aninhada_e_limite_de_bytes_lidos`, `::limite_de_membros_e_de_tempo`. | Overflow do inotify não é testado no fluxo bibliográfico (ver CA-B12). Membros de compactado não são cadastráveis: a A Classificar recusa com explicação (ADR 0006). |
| B-06 Priorizar arquivos grandes e candidatos, sem starvation | Atendido | `dom::prioridade.rs::grande_e_suspeito_passam_na_frente_no_inicio`, `::simulacao_pequeno_nao_morre_de_fome`, `::sem_aging_a_fome_e_possivel`; `ag::aging_nao_deixa_arquivo_pequeno_morrer_de_fome`. | Pesos e `aging_por_hora` só foram testados com os valores padrão. Ajuste no acervo real a fazer. |
| B-07 Metadados nativos, texto, capa, ficha, ISBN e sumário | Atendido | `pdf::pdf_textual_com_ficha_catalografica_e_isbn_valido`, `::marcadores_do_pdf_viram_sumario_nativo`, `::limites_de_tamanho_paginas_e_saida`; `ebk::epub.rs::epub3_metadados_sumario_capa_texto_ficha`; `ebk::mobi_fb2.rs::fb2_metadados_capa_sumario_texto`. | Qualidade real da extração não medida (metas do piloto). |
| B-08 OCR seletivo para PDFs digitalizados, preservando o original | Atendido | `pdf::pdf_so_imagem_usa_ocr`, `::pdf_hibrido_so_passa_por_ocr_nas_paginas_sem_texto`, `::caminho_e_bytes_dao_o_mesmo_resultado_e_o_original_nao_muda`; `pdf::djvu.rs::djvu_sem_camada_de_texto_usa_ocr_e_gera_capa_png` (pula sem as ferramentas). Medição (21 PDFs de `_classifica`): 11 só imagem, 10 textuais; OCR tesseract por+eng a 300 dpi com mediana de 1,13 s/página (p90 2,77 s), confiança média 92,1. | Acerto do OCR contra gabarito não medido. |
| B-09 Identificar título, autores, editora, ISBN, série, capa etc. com evidência | Atendido | `dom::isbn.rs::valida_10_e_13`, `::encontra_em_ficha`; `dom::reconciliacao.rs::sem_evidencia_e_desconhecido_nunca_inventado`, `::isbn_invalido_e_ignorado_ca_b09`; `ia::classificacao.rs::evidencia_inventada_e_rejeitada`, `::isbn_invalido_nao_passa`; `ebk::epub.rs::livro_sem_capa_nao_inventa_capa`. | Precisão só no conjunto ouro (metas). |
| B-10 Aprender vocabulário de tags e estantes por exemplos aprovados | Atendido | `dom::vocabulario.rs::sugere_so_do_vocabulario_com_exemplos`, `::tag_nova_nunca_e_aplicada_so_proposta`, `::correcao_humana_muda_proximas_sugestoes_antes_depois`, `::rejeicao_humana_reduz_a_tag`; `ia::classificacao.rs::tag_fora_do_vocabulario_vira_proposta_e_nunca_tag`, `::esquema_enviado_tem_enum_de_tags_candidatas_e_estantes`; `ag::correcao_humana_muda_a_proxima_sugestao_de_tags`. | A medição de 27 de 91 tags fora do vocabulário foi feita com schema sem `enum`. O código atual envia o `enum`, então é preciso medir de novo. |
| B-11 Separar obra, edição, formato e arquivo | Atendido | `dom::identidade.rs::ca_b06_pdf_e_epub_da_mesma_edicao_um_agrupamento_zero_economia`, `::traducao_vira_edicao_separada_na_mesma_obra`, `::volumes_nao_sao_agrupados_nem_pela_obra`; `st::obras_edicoes_formatos_autores_sao_idempotentes`; `ag::ca_b11_reprocessar_tres_vezes_reaproveita`. | Nenhum. |
| B-12 Revisão lado a lado, aceitar/corrigir/rejeitar em lote com desfazer | Parcial | `st::desfazer_lote_restaura_sugestoes_e_guarda_historico`; `cli::decidir_e_resumo_validam_argumentos_antes_de_chamar_o_agente`; `ag::revisao_traz_paginas_resumidas_e_marca_de_ocr_nos_avisos`; `gui::paginas_da_biblioteca_desenham_vazias_e_em_janela_baixa` (só desenho). | A interação da GUI (lado a lado, lote, desfazer) não tem teste. A revisão visual é manual. |
| B-13 Cadastrar na A Classificar de forma idempotente | Depende de validação real | `cal::t_calibredb::adicionar_tres_vezes_nao_duplica`, `::falha_no_meio_deixa_pendencia_e_a_retomada_nao_duplica` (executor simulado); `ag::calibredb_real_cadastra_na_a_classificar_sem_tocar_a_homologada` e `cal::t_real` (calibredb real, pulam sem ele); `st::cadastros_homologacoes_e_backups` (chave `dag:<blake3>`). Medição no Chico (9.14): `add -I dag:<blake3>` em 1,5 s; a idempotência exige `search identifiers:dag:<hash>` antes do `add`, porque o detector do Calibre é título e autor. | O contêiner tem Calibre 7.6, não 9.14. Falta rodar o cadastro idempotente no 9.14, numa biblioteca temporária. |
| B-14 Duplicata exata por hash e duplicidade bibliográfica com cautela | Parcial | Exata: `dom::duplicidade.rs::exata_vence_e_homologado_vence_em_empate`, `::mesmo_arquivo_fisico_nao_recupera_espaco`. Bibliográfica: `dom::identidade.rs::titulo_sozinho_nunca_basta`, `::isbn_validado_mesma_edicao_mas_nao_prova_isolada`. | "Semelhança de conteúdo" não existe: não há fingerprint textual (busca por `fingerprint`, `simhash` e `minhash` sem resultado). Só título, autor, ISBN e metadados. Falta implementar e medir falsos positivos. |
| B-15 Relatório de espaço recuperável realista por volume | Atendido | `dom::espaco.rs::logico_alocado_e_estimativa_sao_distintos`, `::hard_links_nao_contam_em_dobro`, `::reflink_e_economia_incerta`, `::descartado_vira_efetivamente_recuperado_sem_dupla_contagem`, `::homologado_e_a_mantida_e_grupos_ordenados_por_bytes`; `st::candidatos_por_bytes_recuperaveis`. | Relatório por Volume gerado no acervo do Chico não existe ainda. A medição de 25/11 dá só os totais (251 GB fora da Homologada). |
| B-16 Sugerir lixeira só com cópia preservada íntegra e Cobertura | Atendido | `dom::descarte.rs::ca_b16_copia_inacessivel_bloqueia`, `::ca_b16_hash_nao_conferido_ou_backup_nao_verificado_bloqueia`, `::sem_cobertura_no_mesmo_dispositivo_fisico`, `::caminho_feliz_sugere_e_exige_dupla_confirmacao`; `ag::ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica`. | Nenhum no escopo "somente sugerir". A execução é da Lixeira da Fase A (ver CA-B18). |
| B-17 Consultar e corrigir via GUI, CLI e MCP com escopo | Parcial | `cli::decidir_e_resumo_validam_argumentos_antes_de_chamar_o_agente`; `mcp::biblioteca_traduz_argumentos_para_comandos`, `::biblioteca_valida_argumentos_obrigatorios`, `::biblioteca_tools_list_com_schema_estrito`, `::propor_homologacao_nunca_envia_aprovacao`; `gui::biblioteca_visao_geral_pergunta_ao_agente_e_mostra_a_configuracao`. | Correção pela GUI não tem teste de interação. Falta teste ponta a ponta de correção pelo MCP com agente real. |
| B-18 IA local GGUF/llama.cpp, dois modelos, OCR especializado, GPU de 16 GB | Parcial | `ia::servidor.rs::gpu_sem_memoria_cai_para_cpu_e_detecta_queda`, `::sem_fallback_o_oom_vira_erro_tipado`, `::argumentos_levam_ngl_contexto_e_mmproj`; `ia::benchmark.rs::benchmark_com_servidores_falsos_mede_e_nao_inventa`. Medição no Chico: só Ollama com gemma3:12b (VRAM 7,81 GB; pico de GPU 13.969 MiB com 5.059 MiB de outros apps; sem OOM). | Sem segundo modelo medido, sem OCR especializado, sem `llama-server` com GGUF real. Os testes usam servidores falsos. |
| B-19 Segunda IA quando a confiança é baixa ou há divergência | Atendido | `ia::classificacao.rs::divergencia_entre_modelos_vai_para_revisao_sem_escolher`, `::concordancia_entre_modelos_sobe_a_confianca`, `::divergencia_com_metadado_interno_aciona_segundo_parecer_e_revisao`, `::confianca_autodeclarada_do_modelo_e_ignorada`. | O comportamento com dois modelos reais não foi medido (ver CA-B15). |
| B-20 Classificar matemática, xadrez, ficção, HQ segundo a taxonomia | Parcial | `dom::taxonomia.rs::matematica_hierarquia`, `::xadrez_subassuntos_so_com_xadrez`, `::ficcao_quadrinhos_e_niveis`, `::mapeia_so_para_tags_existentes_e_propoe_o_resto`; `dom::vocabulario.rs::quadrinho_recebe_o_conjunto_de_sinonimos`. | A taxonomia precisa ser confirmada pelo Chico. Falta medir acertos nos livros reais dessas áreas (CA-B20). |
| B-21 Processar PDF, EPUB, MOBI, AZW, DJVU, FB2, CBZ/CBR e compactados; DRM e senha sem contornar | Atendido | `ebk::epub.rs::drm_adobe_e_detectado_e_nao_extrai_texto`, `::drm_lcp_e_fairplay_pelos_arquivos_de_licenca`; `ebk::mobi_fb2.rs::mobi_com_drm_e_marcado_e_nao_extrai_texto`, `::fb2_zip_e_codificacao_cp1251`, `::integracao_ebook_convert_mobi_e_azw3`; `ebk::compactados.rs::livros_dentro_de_zip_rar_7z`, `::zip_com_senha_e_corrompido`; `pdf::pdf_com_senha_e_drm_nao_sao_contornados`; `pdf::djvu.rs::djvu_corrompido_e_arquivo_de_outro_formato`. | DJVU e `ebook-convert` reais dependem das ferramentas instaladas (os testes pulam sem elas). |
| B-22 PDF pesquisável como derivado separado | Não atendido | Fora do escopo declarado em `dag-biblio-pdf/src/lib.rs` (linhas 28 a 30): o tesseract gera um PDF por imagem, e o derivado não é montado. | Implementar o derivado com camada de texto, separado do original, com hash do original preservado e teste. |
| B-23 Histórico, proveniência, versões, métricas, tempo e custo por arquivo | Atendido | `ag::ia_recebe_contexto_e_as_inferencias_guardam_tokens_e_tempo_reais`; `st::inferencias_cache_por_prompt`; `st::desfazer_lote_restaura_sugestoes_e_guarda_historico`; `ag::resumo_do_periodo_conta_jobs_classificados_e_duplicatas`; `ia::classificacao.rs::info_servidor_e_ollama_sem_modelo`. | Visão de custo por arquivo na GUI não é testada. |
| B-24 Homologação controlada, idempotente, com verificação e rollback | Depende de validação real | Simulado: `cal::t_calibredb::homologar_exige_aprovacao_valida_antes_de_qualquer_escrita`, `::homologar_recusa_livro_que_ja_existe_na_homologada`, `::homologar_com_servidor_servindo_recusa_local`, `::homologar_falha_no_meio_reverte_o_livro_criado`. Real: `cal::t_real::real_homologacao_via_calibredb_com_verificacao_e_rollback`. CLI: `cli::homologar_cadastro_inexistente_nao_pergunta_nada_e_orienta`. | Escrita na Homologada só foi testada com `calibredb` 7.6 no contêiner. Falta rodar em uma cópia da Homologada com o 9.14 e medir rollback. A original não deve ser usada para isso. |
| B-25 Monitorar adições e mudanças, desassociar sugestões, recalcular hash com debounce | Parcial | `ag::ca_b12_part_debounce_e_conteudo_alterado` (inclui "sugestões do conteúdo antigo foram desassociadas"); `st::job_orfao_volta_a_pendente_com_checkpoint_e_ciclo_de_vida`. | O vigia depende da reconciliação do monitor da Fase A. Não há teste de renomeação e exclusão no fluxo bibliográfico. O recálculo de hash após mudança é da Fase A e não foi retestado aqui. |
| B-26 Backup consistente de biblioteca e metadados, com restauração amostral | Não atendido | Só a camada de dados: `st::cadastros_homologacoes_e_backups` (`registrar_backup`, `registrar_verificacao_backup`, `registrar_restauracao_e_cobertura` em `dag-store/src/biblioteca/cadastros.rs`). Nenhum código do agente chama essas funções (grep). | Gerar manifesto com hashes e metadados, copiar, verificar e executar a restauração amostral. Expor no agente, na CLI e na GUI. |

## 2. Critérios de aceite CA-B01 a CA-B24

| CA | Estado | Evidência | O que falta |
| --- | --- | --- | --- |
| CA-B01 Nenhuma alteração na Homologada durante o aprendizado | Depende de validação real | `ag::ca_b01_sincronizar_nao_altera_a_homologada`; `cal::t_leitura::ler_nao_altera_nada`; `cal::t_trava_instantaneo::instantaneo_detecta_alteracao_criacao_e_remocao`. Medição (25/11): leitura para a comparação de duplicatas em 63 s, `sha256` da Homologada inalterado. | O critério pede o instantâneo antes e depois do aprendizado (`processar`) na biblioteca real. Falta rodar `dag biblioteca sincronizar` e `processar` no Chico e obter `PROVA DE NÃO ALTERAÇÃO: OK`. |
| CA-B02 Exportação mapeia todos os campos e explica os não suportados | Parcial | `cal::t_leitura::perfil_estantes_tags_de_controle_e_compatibilidade`; `cal::t_real::real_leitura_perfil_clone_round_trip_idempotencia_e_recusas`. | Igual a B-02: o clone não está ligado ao registro. |
| CA-B03 A Classificar independente, com colunas compatíveis e reabertura no Calibre | Depende de validação real | `cal::t_calibredb::criar_a_classificar_e_reabrir`, `::recusa_criar_a_classificar_na_homologada_ou_perto_dela`; `ag::calibredb_real_cadastra_na_a_classificar_sem_tocar_a_homologada`. Medição (9.14): `calibredb list` em diretório vazio cria o `metadata.db`. | Criação e reabertura no 9.14 não foram testadas. As colunas `#dag_hash` e `#dag_confianca` foram medidas em 7.6. |
| CA-B04 Hash igual a formato homologado gera alerta com confiança EXATA, sem OCR | Atendido | `ag::ca_b04_duplicata_exata_sem_extracao_nem_ocr`; `dom::duplicidade.rs::exata_por_hash_e_tamanho_e_unica_com_espaco`, `::hash_igual_tamanho_diferente_nunca_e_exata`. Medição de 25/11: 11.440 cópias identificadas sem OCR. | Nenhum no mecanismo. |
| CA-B05 Mesmo título com edição ou idioma distintos não é duplicata | Atendido | `dom::identidade.rs::idiomas_diferentes_sao_edicoes_diferentes_da_mesma_obra`, `::traducao_e_edicao_separada_da_mesma_obra`, `::quadrinhos_com_numero_diferente_nunca_agrupam`; `dom::duplicidade.rs::mesma_obra_e_incerta_nao_geram_espaco`. | Nenhum. |
| CA-B06 PDF e EPUB da mesma edição: um agrupamento e sem falsa economia | Atendido | `dom::identidade.rs::ca_b06_pdf_e_epub_da_mesma_edicao_um_agrupamento_zero_economia`; `dom::duplicidade.rs::outro_formato_da_mesma_edicao_nao_gera_espaco`. | Nenhum. |
| CA-B07 PDF textual, imagem e híbrido: OCR só nas páginas sem texto | Atendido | `pdf::pdf_textual_com_ficha_catalografica_e_isbn_valido`, `::pdf_so_imagem_usa_ocr`, `::pdf_hibrido_so_passa_por_ocr_nas_paginas_sem_texto`, `::medicao_por_pagina_e_ferramentas_detectadas`. Medição: `pdftotext` em 0,002 s/página; relatório por página em `paginas` e `paginas_ocr` (`PROTOCOLO-BIBLIOTECA.md`). | Nenhum no mecanismo. Qualidade contra gabarito: metas. |
| CA-B08 EPUB com OPF/nav, PDF com índice tardio, CBR/CBZ: amostragem progressiva | Atendido | `ebk::epub.rs::epub3_metadados_sumario_capa_texto_ficha`, `::epub2_ncx_capa_por_meta_serie_calibre_e_isbn_invalido`; `pdf::pdf.rs::indice_tardio_e_achado_por_expansao`, `::sem_identificacao_expande_ate_o_miolo_e_respeita_o_teto`; `ebk::compactados.rs::cbz_com_comicinfo`, `::cbr_e_cb7_com_imagens_e_comicinfo`, `::comicinfo_com_capa_declarada_fora_da_primeira`. | Nenhum no mecanismo. CBR e CBZ reais do acervo não medidos. |
| CA-B09 ISBN inválido, homônimos e livro sem capa: sem invenção | Atendido | `dom::reconciliacao.rs::isbn_invalido_e_ignorado_ca_b09`, `::sem_evidencia_e_desconhecido_nunca_inventado`; `dom::autores.rs::homonimos_nunca_sao_unidos_mesmo_com_evidencia`; `ia::classificacao.rs::isbn_invalido_nao_passa`; `ebk::epub.rs::livro_sem_capa_nao_inventa_capa`. | Nenhum. |
| CA-B10 Tags e estantes respeitam o vocabulário; novas tags em separado | Atendido | Igual a B-10: `dom::vocabulario.rs::tag_nova_nunca_e_aplicada_so_proposta`; `ia::classificacao.rs::tag_fora_do_vocabulario_vira_proposta_e_nunca_tag`; `ag::correcao_humana_muda_a_proxima_sugestao_de_tags`. | Medir de novo com o `enum` vigente (ver metas, M5). |
| CA-B11 Reprocessar três vezes: sem duplicar e reaproveitando hash e decisão | Atendido | `ag::ca_b11_reprocessar_tres_vezes_reaproveita`; `st::extracao_e_sugestoes_reprocessadas_3x_nao_duplicam_e_mantem_decisao`; `st::obras_edicoes_formatos_autores_sao_idempotentes`. | Nenhum. |
| CA-B12 Download incompleto, renomeação, exclusão, overflow do inotify | Parcial | `ag::ca_b12_part_debounce_e_conteudo_alterado` (`.part`, debounce e conteúdo alterado). Overflow e reconciliação: `dag-monitor/tests/monitor.rs::ca_a11_overflow_sintetico_registra_lacuna_e_reconcilia` (Fase A, sintético). | Overflow, renomeação e exclusão não são testados no fluxo bibliográfico. Falta teste ponta a ponta. |
| CA-B13 Processo morto durante OCR ou escrita no Calibre: recuperação idempotente | Depende de validação real | `st::job_orfao_volta_a_pendente_com_checkpoint_e_ciclo_de_vida`; `ag::ca_b13_processo_morto_retoma_do_catalogo`; `cal::t_calibredb::falha_no_meio_deixa_pendencia_e_a_retomada_nao_duplica`, `::homologar_falha_no_meio_reverte_o_livro_criado`. | A "morte" é simulada pelo estado gravado no Catálogo, não por `kill` durante OCR. Falta matar o processo no meio do OCR e da escrita do 9.14. |
| CA-B14 Compactados maliciosos: limites e rejeição segura | Atendido | `ebk::compactados.rs::zip_bomb_e_rejeitada_pela_razao_de_expansao_sem_descompactar`, `::bomba_como_epub_e_como_quadrinho_tambem_e_rejeitada`, `::bomba_aninhada_e_limite_de_bytes_lidos`, `::caminhos_perigosos_sao_ignorados_com_aviso`, `::aninhados_respeitam_profundidade`; `ebk::epub.rs::xml_hostil_entidades_nao_explodem`; `dag-agent/tests/seguranca_disco.rs` (nenhuma escrita em disco fora da lista revisada). | Os casos são sintéticos. Não foi testado com compactado real malicioso. |
| CA-B15 GPU de 16 GB com dois modelos: benchmark de VRAM, tokens/s, precisão e fallback | Depende de validação real | `ia::benchmark.rs::benchmark_com_servidores_falsos_mede_e_nao_inventa`, `::sem_nvidia_smi_e_sem_casos_o_relatorio_diz_que_nao_rodou`; `ia::servidor.rs::gpu_sem_memoria_cai_para_cpu_e_detecta_queda`. Medição (Ollama, gemma3:12b, fora do comando `dag modelos benchmark`): 3,9 s/livro, geração de 58,7 tokens/s, JSON válido em 21 de 21, sem OOM. | Falta: segundo modelo; relatório do `dag modelos benchmark` com VRAM, tokens/s e precisão por modelo; OOM provocado. |
| CA-B16 Hash igual, mas cópia homologada inacessível ou backup não verificado: bloquear | Atendido | `dom::descarte.rs::ca_b16_copia_inacessivel_bloqueia`, `::ca_b16_hash_nao_conferido_ou_backup_nao_verificado_bloqueia`; `ag::ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica`. | Nenhum. |
| CA-B17 Cópia única ou edição distinta: bloquear a lixeira; nada automático por IA ou MCP | Atendido | `dom::descarte.rs::ca_b17_unica_copia_bloqueia`, `::ca_b17_edicao_distinta_e_nao_exata_bloqueiam`, `::ia_ou_mcp_sozinha_nunca`; `ag::ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica`. | Nenhum. |
| CA-B18 Aprovação para lixeira: validação final, dupla confirmação, log e teste de restauração | Parcial | Dupla confirmação na Lixeira da Fase A: `mcp::mover_para_lixeira_exige_as_duas_confirmacoes_explicitas`, `dag-cli/tests/lixeira.rs` (Fase A). Bibliográfico: `dom::descarte.rs::caminho_feliz_sugere_e_exige_dupla_confirmacao`. | Teste de restauração antes da exclusão definitiva não existe (ver B-26). A validação final de identidade e estado no momento da aprovação não tem teste no fluxo bibliográfico. |
| CA-B19 Relatório por disco: lógico, alocado, estimado e recuperado, sem dupla contagem | Atendido | `dom::espaco.rs::logico_alocado_e_estimativa_sao_distintos`, `::hard_links_nao_contam_em_dobro`, `::membros_de_conteiner_nunca_somam`, `::descartado_vira_efetivamente_recuperado_sem_dupla_contagem`. | Relatório real no Chico não gerado. |
| CA-B20 Matemática, xadrez e HQ do piloto: evidências revisáveis e registro de acertos | Depende de validação real | `dom::taxonomia.rs::matematica_hierarquia`, `::xadrez_subassuntos_so_com_xadrez`; `dom::identidade.rs::quadrinhos_com_numero_diferente_nunca_agrupam`. Medição: 21 PDFs de `_classifica` com estante no enum (21 de 21), sem avaliação de acerto. | Conjunto ouro dos estratos matemática, xadrez e quadrinhos ainda não exportado nem avaliado. |
| CA-B21 Round-trip com Unicode, autorias múltiplas, séries e colunas customizadas | Depende de validação real | `cal::t_real::real_leitura_perfil_clone_round_trip_idempotencia_e_recusas`; `cal::t_leitura::leitura_tipada_de_todos_os_datatypes`; `cal::t_calibredb::argumentos_de_metadados`. Medição (9.14): `set_metadata --field` em 0,42 s, acentos preservados. | Falta round-trip de uma cópia da Homologada inteira (séries, autorias múltiplas, colunas customizadas, vários formatos) no 9.14. |
| CA-B22 Rejeição pelo usuário: nenhuma escrita na Homologada, decisão auditável, reprocessamento | Atendido | `dom::vocabulario.rs::rejeicao_humana_reduz_a_tag`; `st::desfazer_lote_restaura_sugestoes_e_guarda_historico`; `ag::ca_b01_sincronizar_nao_altera_a_homologada`. Reprocessamento: `enfileirar --motivo reprocessar` (`GUIA-OPERACAO.md`). | Nenhum. |
| CA-B23 GUI, CLI e MCP ao mesmo tempo: um único escritor e sem bypass de confirmação | Parcial | `mcp::propor_homologacao_nunca_envia_aprovacao`; `ag::ca_b23_parcial_mcp_nao_aprova_homologacao`; `cli::homologar_cadastro_inexistente_nao_pergunta_nada_e_orienta`; Fase A: `dag-agent/tests/servidor.rs::escritas_concorrentes_de_varios_clientes_sao_serializadas`. | Homologação pela GUI com confirmação não tem teste. GUI, CLI e MCP simultâneos não são testados no fluxo bibliográfico. |
| CA-B24 Backup com manifesto, verificação e restauração amostral antes de limpeza | Não atendido | Só o armazenamento: `st::cadastros_homologacoes_e_backups` (registro de backup, verificação e restauração). | Igual a B-26. Sem manifesto produzido pelo agente e sem restauração amostral executada. |

## 3. Surpresa WOW-01 a WOW-10

Nenhuma demonstração foi feita com a biblioteca real. "Parcial" indica que a parte de lógica existe e tem teste.

| WOW | Estado | Evidência | O que falta |
| --- | --- | --- | --- |
| WOW-01 Alerta "Você já tem este livro" em segundos | Parcial | `ag::ca_b04_duplicata_exata_sem_extracao_nem_ocr`; `ag::ca_b12_part_debounce_e_conteudo_alterado`; `dom::duplicidade.rs::hash_igual_tamanho_diferente_nunca_e_exata`. | Demonstração com download novo real, tempo até o alerta (debounce de 30 s) e alerta na GUI sem teste de interação. |
| WOW-02 "Economize espaço com segurança" | Parcial | `dom::espaco.rs::homologado_e_a_mantida_e_grupos_ordenados_por_bytes`; `st::candidatos_por_bytes_recuperaveis`; `cli` (`dag biblioteca espaco`). | Validação manual dos dez maiores grupos não feita. O painel da GUI não tem teste. |
| WOW-03 "Bibliotecário que aprende" | Parcial | `ag::correcao_humana_muda_a_proxima_sugestao_de_tags`; `dom::vocabulario.rs::correcao_humana_muda_proximas_sugestoes_antes_depois`. A explicação com exemplos existe no texto da sugestão ("parecido com «Título», decisão sua"). | Antes e depois com conjunto cego não feito. |
| WOW-04 Livro digitalizado sem nome útil | Parcial | `pdf::pdf_so_imagem_usa_ocr`, `pdf::indice_tardio_e_achado_por_expansao`; `ag::revisao_traz_paginas_resumidas_e_marca_de_ocr_nos_avisos`. Medição: 11 de 21 PDFs só imagem. | Demonstração com PDFs digitalizados de baixa qualidade não feita. |
| WOW-05 "Uma obra, muitos formatos" | Parcial | `dom::identidade.rs::traducao_vira_edicao_separada_na_mesma_obra`, `::volumes_nao_sao_agrupados_nem_pela_obra`; `st::obras_edicoes_formatos_autores_sao_idempotentes`. | Demonstração com obra real de várias edições e formatos. Tela sem teste. |
| WOW-06 "Estante automática" exata | Depende de validação real | `ia::classificacao.rs::esquema_enviado_tem_enum_de_tags_candidatas_e_estantes`; `dom::taxonomia.rs::estante_do_assunto_raiz`; `dom::vocabulario.rs::sugere_so_do_vocabulario_com_exemplos`. | Comparação com as estantes reais homologadas. Medição: 21 de 21 estantes dentro do enum, acerto não avaliado. |
| WOW-07 "Trabalho noturno" com pausa e retomada | Parcial | `st::job_orfao_volta_a_pendente_com_checkpoint_e_ciclo_de_vida`; `ag::ca_b13_processo_morto_retoma_do_catalogo`; `ag::resumo_do_periodo_conta_jobs_classificados_e_duplicatas`; `max_discos_extracao` e `extracoes_por_dispositivo` (config). Progresso como `Progresso::Mensagem`, sem variante estruturada (`PROTOCOLO-BIBLIOTECA.md`). | Encerrar o agente de verdade durante OCR. Barra de progresso estruturada depende de acordo com as frentes de GUI e agente. |
| WOW-08 "Por que você classificou assim?" | Parcial | `ag::revisao_traz_paginas_resumidas_e_marca_de_ocr_nos_avisos`; `ag::reconciliacao_com_divergencia_vai_para_revisao_e_humano_vence`; `dom::reconciliacao.rs::conjunto_de_campos_lista_razoes`. | Revisar cinco livros só pela GUI não foi feito. O painel não tem teste de interação. |
| WOW-09 "Meu acervo está protegido?" | Parcial | `dom::descarte.rs::ca_b16_copia_inacessivel_bloqueia`, `::sem_cobertura_no_mesmo_dispositivo_fisico`; `ag::ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica`. | Disco desconectado simulado só com fixtures. O indicador de Cobertura na GUI e o teste de restauração (B-26) não existem. |
| WOW-10 "Novidades da biblioteca" (resumo, silenciar) | Parcial | `st::alertas_idempotentes_vistos_e_resumo`; `ag::resumo_do_periodo_conta_jobs_classificados_e_duplicatas`; `dag biblioteca resumo` (`GUIA-OPERACAO.md`). | Resumo diário agendado não existe. `silenciar_alerta` existe só no `dag-store` (`alertas.rs`), sem comando na CLI, MCP ou GUI. |

## 4. Metas de qualidade do piloto (CRITERIOS-ACEITE, seção "Qualidade mensurável")

**Nenhuma meta de precisão, cobertura ou desempenho foi medida ainda.** O conjunto ouro não foi exportado nem avaliado, e o benchmark de 100 livros não foi rodado. As medições de 25/11 são de inspeção e de processamento, sem gabarito.

| Meta | Estado | Evidência | O que falta |
| --- | --- | --- | --- |
| Conjunto ouro de 150 itens reais, autorizados e estratificados | Depende de validação real | `cli::ouro_exportar_e_avaliar_de_ponta_a_ponta_sem_itens` (só prova o fluxo sem itens). | Exportar com `dag biblioteca ouro exportar --n 150 --saida PASTA --com-texto` (pasta criada antes) e preencher o `gabarito` de cada caso com o Chico. |
| Metadados críticos (título, autor, edição) >= 95% em alta confiança | Depende de validação real | Cálculo em `dag biblioteca ouro avaliar --casos PASTA`. | Medir no conjunto ouro. Cobertura e abstenções reportadas separadamente. |
| Duplicata exata: 100% em fixtures e nenhum falso positivo no ouro | Parcial | Fixtures: `ag::ca_b04_duplicata_exata_sem_extracao_nem_ocr`, `dom::duplicidade.rs::exata_por_hash_e_tamanho_e_unica_com_espaco`. Medição: 11.440 cópias com hash e tamanho iguais (sem conferência manual). | Falsos positivos no ouro (`duplicata_exata` no `gabarito_extra`) não medidos. |
| Vínculo obra/edição >= 98% em agrupamentos automáticos | Depende de validação real | `dom::identidade.rs` (vários testes de agrupamento). Campos `mesma_edicao_que`, `mesma_obra_que` e `obra_diferente_de` do gabarito. | Medir no ouro, com as ambiguidades indo para revisão. |
| Tags >= 90% de precisão micro, com recall e divergências por área | Depende de validação real | Medição preliminar (inspeção, schema sem `enum`): 27 de 91 tags fora do vocabulário. | Medir com o `enum` vigente, por área (matemática, xadrez, ficção, HQ). |
| Integridade: 0 alterações não autorizadas na Homologada e 0 descartes automáticos | Parcial | 0 descartes automáticos: `dom::descarte.rs::ia_ou_mcp_sozinha_nunca`; `ag::ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica`. Alterações: `ag::ca_b01_sincronizar_nao_altera_a_homologada`; medição de 25/11 com `sha256` inalterado. | Prova antes e depois de sincronizar e processar a biblioteca real. |
| Desempenho: benchmark em 16 GB de VRAM, throughput, RAM, VRAM e tempo por 100 livros | Depende de validação real | Ad hoc (Ollama, não o `dag modelos benchmark`): 3,9 s/livro com gemma3:12b; textual ~5 s e escaneado ~10 s (projeção); pico de GPU 13.969 MiB. Testes: `ia::benchmark.rs::benchmark_com_servidores_falsos_mede_e_nao_inventa`. | `dag modelos benchmark --casos PASTA --modelo A B --saida benchmark.md` em 100 livros, com dois modelos. Sem meta de tempo antes da linha de base. |
| Carga: >= 100 mil entradas bibliográficas sintéticas, sem tudo em RAM | Atendido | `st::carga_100_mil_entradas_bibliograficas` (fila, consultas paginadas, retomada). | Nenhum no sintético. O acervo real do Chico (~22 mil arquivos a classificar) não foi carregado. |

## 5. Como reproduzir

Os comandos abaixo rodam dentro do worktree. Use um diretório de alvo próprio e apague-o no fim (regra do `CLAUDE.md`). Não rode em paralelo com outras frentes sem `-j 2`.

```
export CARGO_TARGET_DIR=/home/user/alvos/fase-b-aceite
cargo test -p dag-biblio -j 2                    # identidade, duplicidade, descarte, espaço, prioridade, reconciliação, vocabulário, taxonomia
cargo test -p dag-biblio-pdf -j 2                # PDF/DjVu; pula OCR e DjVu sem tesseract/djvulibre
cargo test -p dag-biblio-ebook -j 2              # EPUB, MOBI, FB2, CBZ/CBR, compactados, bombas; ebook-convert pula se ausente
cargo test -p dag-biblio-ia -j 2                 # classificação com servidores HTTP falsos, benchmark, llama-server (binário falso)
cargo test -p dag-calibre -j 2                   # t_calibredb (simulado) e t_real (calibredb real; pula com AVISO se ausente)
cargo test -p dag-store --test biblioteca -j 2   # esquema 11, espelho, fila, obras, decisões, alertas, cadastros, carga 100 mil
cargo test -p dag-agent --test biblioteca -j 2   # CA-B01, B04, B11, B12, B13, B16, B17, B23; compactados; IA com contexto
cargo test -p dag-agent --test seguranca_disco -j 2
cargo test -p dag-cli --test biblioteca -j 2    # ajuda, config, decidir/resumo, homologar sem terminal, ouro exportar/avaliar
cargo test -p dag-mcp --test protocolo -j 2     # schema estrito, tradução de argumentos, homologação sem aprovação, lixeira
cargo test -p dag-app --test gui -j 2           # páginas da Biblioteca (desenho) e visão geral
```

Antes de concluir, na raiz do worktree (regra do `CLAUDE.md`):

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo clean
```

Ensaios manuais no computador do Chico, depois de ele registrar a Homologada (comandos do `GUIA-OPERACAO.md`):

```
dag biblioteca registrar /mnt/e/Livros --papel homologada --nome "Livros"
dag biblioteca sincronizar 1                     # deve imprimir PROVA DE NÃO ALTERAÇÃO: OK
dag biblioteca duplicatas
dag biblioteca espaco
dag biblioteca ouro exportar --n 150 --saida /mnt/e2/ouro --com-texto
dag biblioteca ouro avaliar --casos /mnt/e2/ouro
dag modelos benchmark --casos /mnt/e2/ouro --modelo gemma3:12b OUTRO_MODELO --saida benchmark.md
```

## 6. Pendências para o piloto no computador do Chico

**Antes de medir (código):**
1. **B-02 e CA-B02:** ligar `clonar_estrutura` ao registro da A Classificar (`dag-agent/src/biblioteca/bibliotecas.rs`) e testar.
2. **B-26, CA-B24 e WOW-09:** gerar o manifesto de backup, verificar e executar a restauração amostral. Sem isso não há como liberar limpeza definitiva.
3. **CA-B18:** teste de restauração antes de qualquer exclusão definitiva, e validação de identidade e estado no momento da aprovação.
4. **B-22:** PDF pesquisável como derivado separado. Hoje está fora do escopo declarado.
5. **B-14:** semelhança de conteúdo. Hoje só há título, autor, ISBN e metadados.
6. **B-12, B-17, WOW-01, WOW-02, WOW-05, WOW-08:** testes de interação da GUI (lado a lado, lote, desfazer, painel de proveniência).
7. **WOW-10:** exposição de `silenciar_alerta` na CLI, MCP ou GUI, e resumo diário se for desejado.
8. **CA-B12 e B-25:** teste de overflow, renomeação e exclusão no fluxo bibliográfico.

**Para medir no Chico (ambiente real):**
9. Rodar os testes de `dag-calibre` (`t_real`, `t_calibredb`) e o `calibredb_real_...` com o `calibredb` 9.14, em biblioteca temporária, não na Homologada. Isso fecha CA-B03, CA-B13, CA-B21 e B-24.
10. Matar o processo no meio do OCR e no meio de uma escrita no Calibre (CA-B13, WOW-07).
11. Homologar um livro numa cópia da Homologada e medir o rollback (B-24).
12. Exportar o conjunto ouro de 150 itens, preencher o gabarito e rodar `ouro avaliar`. Medir todas as metas de precisão.
13. Rodar `dag modelos benchmark` com gemma3:12b e um segundo modelo, em 100 livros. Medir VRAM, tokens/s, RAM e tempo (CA-B15, B-18).
14. Medir de novo as tags com o `enum` vigente (B-10, M5). A medição de 27 de 91 é de antes.
15. Demonstrar WOW-01, WOW-03, WOW-04, WOW-05 e WOW-08, que são a condição de sucesso do piloto (CRITERIOS-SURPRESA.md).

**Perguntas que só o Chico pode responder (decisões, não fatos):**
- A taxonomia de `dag-biblio/src/taxonomia.rs` é a dele? Matemática, xadrez, ficção e HQ precisam de subdivisões diferentes?
- Caminho da A Classificar: o padrão é `/mnt/e2/Livros_A_Classificar`. Confirma?
- Modo de homologação: `calibredb` local com o Calibre fechado, ou servidor com credenciais?
- Ativar `usar_unshare` (rodar o `calibredb` da A Classificar em outro namespace)? O padrão é desligado.
- Qual é o segundo modelo para o benchmark?
- Quais 150 itens autoriza para o conjunto ouro? A especificação exige que sejam reais e autorizados.
- A ordem de prioridade (grandes e suspeitos primeiro, com aging) está do jeito que ele quer?

**Riscos já conhecidos (de `M0-INVENTARIO-TECNICO.md` e das medições):**
- A trava do Calibre é por usuário: com a GUI ou o `calibre-server` aberto, o `calibredb` recusa (`calibre_ocupado`). Feche antes de cadastrar ou homologar.
- Formatos de 0 byte na Homologada casam com ~139 mil arquivos vazios. Por isso a duplicata exata ignora tamanho menor que 1 KiB.
- Ferramentas de Calibre 9.14 podem gravar na biblioteca só por abri-la (`last_expired_trash_at`, `full-text-search.db`). Por isso a Homologada é lida só por SQLite em `mode=ro`.

Nenhuma parte deste aceite foi aprovada pelo Chico. É o estado técnico em 25/11/2026, para revisão.
