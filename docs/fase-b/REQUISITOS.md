# Histórias de usuário e requisitos — Curadoria Bibliográfica

Prioridades: P0 obrigatório para piloto; P1 obrigatório antes da entrega geral; P2 evolução. Cada resultado da IA deve conter evidência (arquivo, páginas, origem, confiança e versão do modelo) e possibilidade de correção.

| ID | Prioridade | História / comportamento |
|---|---|---|
| B-01 | P0 | Como proprietário, quero conectar a biblioteca homologada em leitura e extrair inventário completo de campos padrão e personalizados, tags, autores, séries, estantes e formatos, sem alterar nada. |
| B-02 | P0 | Quero clonar, quando compatível, a estrutura de colunas/configurações para uma biblioteca Calibre independente chamada A Classificar; confirmar as diferenças não clonáveis. |
| B-03 | P0 | Quero comparar o inventário DAG com os formatos e hashes dos livros homologados, calculando hashes ausentes e verificando validade por identidade/tamanho/mtime. |
| B-04 | P0 | Quero receber alerta quando novo PDF/EPUB/MOBI/AZW/AZW3/CBR/CBZ/DJVU/FB2 (conforme suporte) repetir hash homologado, mostrando obra, edição, localização, formato, tamanho e possível economia. |
| B-05 | P0 | Quero fila persistente, incremental e retomável alimentada pelo watcher, inclusive eventos dentro de arquivos compactados quando suportados e com limites contra bombas de descompressão. |
| B-06 | P0 | Quero priorizar arquivos grandes e candidatos de duplicidade, com política configurável que também evite starvation de arquivos pequenos. |
| B-07 | P0 | Quero extrair metadados nativos, texto, capa, folha de rosto, ficha catalográfica, ISBN e sumário/índice; começar por até 20 páginas configuráveis e expandir conforme evidências. |
| B-08 | P0 | Quero OCR seletivo para PDFs digitalizados, híbridos e imagens de quadrinhos; preservar original e registrar idioma, páginas, qualidade e falhas. |
| B-09 | P0 | Quero identificar título, subtítulo, autores/coautores, editora, edição, data, idioma, ISBN/DOI quando aplicável, série, volume, resumo, capa e tags com evidências e campos desconhecidos explícitos. |
| B-10 | P0 | Quero aprender meu vocabulário de tags, normalização de autores e estantes por exemplos aprovados, sem criar tags novas automaticamente. |
| B-11 | P0 | Quero separar obra, edição/tradução, formato e arquivo; agrupar PDF+EPUB da mesma edição, preservar edições distintas e sinalizar casos ambíguos. |
| B-12 | P0 | Quero revisão humana com comparação lado a lado entre dados extraídos, sugestão da IA e catálogo homologado; aceitar/corrigir/rejeitar em lote com desfazer. |
| B-13 | P0 | Quero cadastrar candidatos na biblioteca A Classificar por interfaces suportadas do Calibre, de modo idempotente, sem duplicar registros em reprocessamentos. |
| B-14 | P0 | Quero detectar duplicata exata por hash e duplicidade bibliográfica por identificadores, metadados e semelhança de conteúdo, sem confundir hipótese com certeza. |
| B-15 | P0 | Quero relatório de espaço recuperável realista por volume, com diferenciação entre tamanho lógico, alocado e economia efetiva incerta. |
| B-16 | P0 | Quero somente sugerir lixeira após verificar cópia preservada íntegra, acessível e, se necessário, backup restaurável; confirmação humana obrigatória e auditoria. |
| B-17 | P1 | Quero consultar e corrigir metadados via GUI, CLI e MCP, respeitando o agente escritor único, autorizações e escopo de cada ferramenta. |
| B-18 | P1 | Quero usar IA local quantizada GGUF/llama.cpp ou equivalente, comparar pelo menos dois modelos e OCR especializado na GPU de 16 GB, com fallback CPU e medição real. |
| B-19 | P1 | Quero revisão por segunda IA quando a confiança for baixa ou modelos divergirem, mas com validador determinístico e evidência obrigatória. |
| B-20 | P1 | Quero classificar matemática (cálculo, álgebra, geometria, estatística), xadrez (aberturas, finais, estratégia), ficção, romances, gibis e demais assuntos segundo minha taxonomia. |
| B-21 | P1 | Quero processar PDF, EPUB/EPUB3, MOBI, AZW/AZW3, DJVU, FB2, CBZ/CBR e livros em ZIP/RAR/7Z quando bibliotecas permitirem, identificando DRM/senha/formatos não suportados sem contorná-los. |
| B-22 | P1 | Quero geração opcional de PDF pesquisável como derivado separado, mantendo hash e original imutáveis. |
| B-23 | P1 | Quero histórico de decisões, correções, proveniência, versão de regras/modelos, métricas, erros, tempo e custo computacional por arquivo. |
| B-24 | P1 | Quero homologação controlada para biblioteca oficial somente após minha aprovação, com operação idempotente, verificação e rollback quando possível. |
| B-25 | P1 | Quero monitorar novas adições e mudanças de conteúdo, desassociar classificações obsoletas e recalcular hashes com debouncing de downloads incompletos. |
| B-26 | P1 | Quero backup consistente de biblioteca e metadados, com restauração amostral comprovada antes de autorizar limpeza definitiva. |

## Regras de classificação
- Ordem de evidência: hash verificado > identificadores bibliográficos corroborados > conteúdo/índice e capa > metadados internos > nome do arquivo. ISBN pode identificar edição, mas deve ser validado e não tomado como prova isolada.
- Autor existente no Calibre deve ser reutilizado após normalização e confirmação de identidade; homônimos não são automaticamente a mesma pessoa.
- Metadados e tags pessoais homologados prevalecem sobre sugestões automáticas; o sistema não altera a taxonomia sem consentimento.
- Não misturar quadrinhos de números diferentes, coleções distintas, traduções, edições ou livros com título coincidente.
- Fila de exceções: ilegível, criptografado, DRM, OCR ruim, metadados contraditórios, ausência de evidências, duplicidade incerta.
- Catálogo A Classificar é área de trabalho; não autoriza descarte de originais.
