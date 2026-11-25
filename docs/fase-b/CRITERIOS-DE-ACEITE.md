# Critérios de aceite obrigatórios

Cada caso deve ter fixture, passos, resultado esperado, evidência automatizada ou manual reproduzível, versão e status. Não declarar aprovação sem execução.

| ID | Dado / Quando / Então |
|---|---|
| CA01 | Dada main existente, executar regressão do PR1; inventário, watcher, hashes e compactados continuam funcionando. |
| CA02 | Dada biblioteca com campos nativos e customizados de vários tipos, mapear upstream e clonar estrutura; nenhuma alteração na homologada; comparar snapshot lógico antes/depois. |
| CA03 | Dado conjunto de autores aliases, tags hierárquicas, estantes e séries, classificar novos livros; respeitar vocabulário e ordenação; novas tags ficam pendentes. |
| CA04 | Dado PDF textual, escaneado e híbrido, extrair texto e OCR seletivo, localizar ficha/índice/capa e evidência por página; testar 10/15/20 páginas e fallback. |
| CA05 | Dados EPUB, MOBI, AZW3, CBR/CBZ e demais formatos declarados, extrair metadados e conteúdo; registrar explicitamente DRM, senha, formato não suportado e falha. |
| CA06 | Dado CBR com ComicInfo e outro sem, identificar série/número, ordenar páginas corretamente e não inventar autores. |
| CA07 | Dado compactado aninhado e arquivo malicioso, respeitar limites e impedir escape de diretório, execução e consumo ilimitado. |
| CA08 | Dados livros de tamanhos variados, priorizar maiores e grupos redundantes, permitir prioridade manual e não deixar pequenos eternamente pendentes. |
| CA09 | Dado arquivo já homologado com hash igual, identificar ID, título, formato, caminho e hash; não importar duplicata. |
| CA10 | Dados mesma obra em PDF/EPUB/MOBI, agrupar formatos da mesma edição; edições/traduções/anotações diferentes permanecem preservadas. |
| CA11 | Dados dois PDFs similares com hashes diferentes, sugerir match com evidência; não liberar lixeira por similaridade. |
| CA12 | Dado download em andamento, aguardar estabilidade; após conclusão emitir aviso de duplicidade e atualizar catálogo; testar renomeação e overflow do watcher. |
| CA13 | Dada correção humana de tag/autor/estante, persistir aprendizado e reaplicar em caso semelhante, com possibilidade de desfazer. |
| CA14 | Dada sugestão com campo ausente ou conflitante, mostrar desconhecido/evidência; validar ISBN e schema; não fabricar ficha. |
| CA15 | Dado livro aprovado, promover via interface suportada do Calibre e verificar todos os campos e formatos por round-trip; repetir job sem duplicar. |
| CA16 | Dada falha entre importação e atualização DAG, reiniciar e reconciliar sem duplicar ou perder arquivos. |
| CA17 | Dada GPU 16 GiB, comparar dois modelos locais, registrar pico VRAM e qualidade, demonstrar execução sem OOM; meta proposta 14,5 GiB; justificar exceção antes de aceite. |
| CA18 | Dado candidato à lixeira, revalidar cópia homologada e hash; bloquear cópia única, raízes Calibre, arquivo alterado, match semântico e compactado com outros membros; testar recuperação. |
| CA19 | Dado backup da biblioteca, restaurar em destino isolado e conferir hashes, metadados, capas, tags e colunas; sem backup verificado, purga bloqueada. |
| CA20 | Dado fechamento da GUI ou interrupção do worker, retomar filas e cache sem repetir OCR concluído; respeitar configuração existente do serviço. |
| CA21 | Dado catálogo grande, UI paginada e responsiva, painel de cobertura e bytes sem dupla contagem; medir latência, RAM, I/O e throughput. |
| CA22 | Dado documento com prompt injection, tratar conteúdo como dado; nenhuma ação externa, comando ou descarte disparado pelo texto. |
| CA23 | Dada biblioteca real autorizada, validar piloto matemática, xadrez e gibis com proprietário; não publicar dados privados. |

## Qualidade mensurável proposta
Conjunto ouro estratificado de pelo menos 120 itens (incluindo negativos, digitalizados, edições distintas e gibis), separado dos exemplos de aprendizado. Medir precisão/recall por campo, cobertura, taxa de abstinência, agrupamento e tags multilabel. Metas iniciais propostas: precisão ≥95% para título/autor em sugestões de alta confiança; precisão ≥90% nas tags sugeridas de alta confiança; zero falso positivo de descarte no conjunto; 100% dos candidatos com estado explícito. Reportar denominadores e cobertura para impedir otimização por abstinência. Ajustar metas somente com aprovação do proprietário. Nenhum teste sintético substitui piloto real.

## Definition of Done
Todos os critérios obrigatórios aprovados ou bloqueios explicitamente aceitos pelo proprietário; testes unitários, integração, regressão, segurança, benchmark, manual Linux e evidências; sem defeito crítico de perda de dados. Atualizar README/ADR e matriz de rastreabilidade. Não fazer merge automático.