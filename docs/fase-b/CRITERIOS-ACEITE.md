# Critérios de aceite verificáveis — Fase B

## Portões obrigatórios (todos bloqueantes para lançamento)
| ID | Dado / quando | Então / evidência exigida |
|---|---|---|
| CA-B01 | Biblioteca homologada conectada e snapshot/hash antes/depois | Nenhuma alteração no metadata.db, arquivos, capas ou configurações durante aprendizado; comparar checksums e auditoria. |
| CA-B02 | Biblioteca com campos customizados, tags, séries e estantes | Exportação mapeia todos os campos acessíveis e explica explicitamente os não suportados; nenhuma perda silenciosa. |
| CA-B03 | Biblioteca A Classificar criada | É independente da homologada; colunas, tipos e restrições compatíveis; teste de escrita via API/CLI Calibre e reabertura no Calibre. |
| CA-B04 | Novo arquivo com hash igual ao de formato homologado | Alerta automático mostra obra, edição, formato, caminho, tamanho, hash e confiança EXATA; nenhum OCR necessário para detectar. |
| CA-B05 | Arquivo com mesmo título, mas edição/idioma distintos | Não marca como duplicata descartável; apresenta vínculo de obra com edição separada. |
| CA-B06 | Mesmo livro em PDF e EPUB, mesma edição | Um agrupamento bibliográfico, dois formatos preservados e sem falsa economia de espaço. |
| CA-B07 | PDF textual, imagem e híbrido | Texto nativo quando confiável, OCR seletivo nas páginas sem texto; relatório de páginas e qualidade; original inalterado. |
| CA-B08 | EPUB com OPF/nav, PDF com índice tardio, CBR/CBZ com imagens | Extrai metadados/índice/capa adequadamente; amostragem progressiva não se limita às primeiras 20 páginas quando insuficientes. |
| CA-B09 | ISBN inválido, autores homônimos, livro sem capa | Sem invenção; campo desconhecido ou revisão, com origem e confiança; ISBN validado por checksum. |
| CA-B10 | Tags/estantes da biblioteca homologada | Sugestões respeitam vocabulário; novas tags propostas em separado; correções humanas persistem e influenciam próximos casos. |
| CA-B11 | Reprocessar o mesmo arquivo três vezes | Sem duplicar obras, formatos ou registros; hash e decisão anterior reaproveitados se válidos. |
| CA-B12 | Download incompleto, renomeação, mudança e exclusão, overflow inotify | Fila consistente, debounce e reconciliação; nunca catalogar arquivo incompleto como definitivo. |
| CA-B13 | Processo morto durante OCR ou escrita Calibre | Checkpoint e recuperação idempotente; nenhum original corrompido, sem registro parcial não reconciliável. |
| CA-B14 | PDF em ZIP/RAR/7Z e compactado malicioso | Limites de profundidade, bytes, quantidade, tempo e expansão; rejeição segura e sem escrita fora do sandbox. |
| CA-B15 | GPU de 16 GB e dois candidatos de modelo | Benchmark documentado de VRAM real, tokens/s, tempo/página, precisão e fallback; sem OOM que derrube agente. |
| CA-B16 | Hash igual mas cópia homologada inacessível ou backup não verificado | Bloquear recomendação executável de descarte; exibir razão. |
| CA-B17 | Cópia única ou edição distinta | Bloquear lixeira por fluxo bibliográfico; nenhum descarte automático por IA ou MCP. |
| CA-B18 | Aprovação para lixeira | Exigir validação final de identidade/estado, dupla confirmação, log e teste de restauração; nunca esvaziar automaticamente. |
| CA-B19 | Relatório por disco | Distingue bytes lógicos, alocados, estimativa e espaço realmente recuperado; sem dupla contagem de hard links/reflinks. |
| CA-B20 | Livros de matemática, xadrez e gibis do piloto | Evidências revisáveis para título, autor, edição, assuntos, estantes e formatos; registrar acertos/erros e fila de revisão. |
| CA-B21 | Biblioteca homologada contendo caracteres Unicode, autorias múltiplas, séries e colunas customizadas | Round-trip preserva valores, acentos, relações e múltiplos formatos. |
| CA-B22 | Usuário rejeita sugestão | Nenhuma escrita na homologada; decisão auditável, com possibilidade de corrigir e reprocessar. |
| CA-B23 | GUI, CLI, MCP simultâneos | Um único escritor DAG; comandos autenticados, auditados, sem bypass de confirmação destrutiva. |
| CA-B24 | Backup de biblioteca consolidada | Manifesto com hashes e metadados, verificação de integridade e restauração amostral aprovada antes de limpeza definitiva. |

## Qualidade mensurável do piloto
- Conjunto ouro: no mínimo 150 itens reais e autorizados, estratificados entre matemática, xadrez, ficção, quadrinhos, PDF textual, PDF imagem, EPUB e demais formatos presentes; incluir duplicatas exatas, obras com mesmo título e edições distintas. Amostras avaliadas manualmente.
- Metadados críticos (título, autor, edição quando disponível): >= 95% de precisão nos campos emitidos como alta confiança; cobertura e abstenções reportadas separadamente.
- Duplicata exata: 100% de correspondência em fixtures de hash verificado; nenhum falso positivo no conjunto ouro.
- Vínculo obra/edição: >= 98% de precisão em agrupamentos automáticos; casos ambíguos vão para revisão.
- Tags: >= 90% de precisão micro no conjunto ouro, reportar recall e divergências por área; nenhuma tag nova aplicada sem aprovação.
- Integridade: 0 alterações não autorizadas na biblioteca homologada e 0 descartes automáticos.
- Desempenho: benchmark em máquina de 16 GB VRAM com throughput, RAM, VRAM e tempo por 100 livros; metas de tempo só após baseline medido, sem prometer desempenho inventado.
- Carga: ensaio >= 100 mil entradas bibliográficas sintéticas para fila, consultas paginadas e retomada; sem exigir todas em RAM.

## Testes mínimos
Unitários (ISBN, normalização, hash, idempotência, classificação, pontuação); integração (calibredb/ebook-meta e versões suportadas, SQLite DAG, watchers, OCR, arquivos compactados); end-to-end (biblioteca temporária homologada e A Classificar, aprovação, importação e rollback); falhas (energia, volume desconectado, permissões, disco cheio, corrupção parcial, timeout/OOM GPU); segurança (path traversal, symlink, zip bomb, prompts maliciosos dentro de livros, DRM/senha, execução não privilegiada).

Não declarar CA aprovado por teste apenas simulado quando requer validação no Calibre real ou GPU real. Registrar comando, ambiente, data, resultado e artefatos.
