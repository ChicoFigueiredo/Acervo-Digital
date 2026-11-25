# Arquitetura e plano de execução — Fase B

## Integração sem reescrita
Reusar crates e contratos atuais de dag-core, dag-store, dag-agent, dag-scanner, dag-monitor, dag-app, CLI e MCP. Antes de criar crates novos, mapear extensões existentes, migrações SQLite e interfaces de jobs. Não modificar os repositórios de referência.

## Pipeline
1. Importar perfil Calibre homologado em leitura: versão, colunas padrão/customizadas, tags, autores, séries, formatos, biblioteca virtual/estantes e políticas do usuário. Registrar snapshot e relatório de compatibilidade.
2. Indexar formatos homologados por hash de conteúdo (calculado pelo DAG, sem confiar em hash externo), identidade bibliográfica e formato. Distinguir formatos e arquivos físicos.
3. Reutilizar eventos e hashes DAG para detectar novos livros. Esperar arquivo estabilizar e inserir em fila persistente. Identificar hash exato antes de OCR/IA.
4. Priorizar fila por economia potencial, tamanho e novidade, com aging para evitar starvation; permitir limites por dispositivo e pausas.
5. Extração por adaptadores de formato: metadados internos, conteúdo estruturado, imagens, capa, ficha, sumário e OCR seletivo. Processamento em sandbox, limites de memória/CPU/I/O, tamanho e tempo.
6. Classificação local: candidatos GGUF de Hugging Face compatíveis com llama.cpp, incluindo Inkling 2 Mini apenas após confirmar modelo, licença, multimodalidade e quantizações; comparar segundo modelo e OCR especializado com benchmark na GPU 16 GB. Nenhuma dependência de um modelo específico.
7. Validação determinística de identificadores e evidências, resolução de autores, obra, edição, idioma, série e tags; ambiguidades para revisão.
8. Persistir metadados e artefatos de extração no DAG com proveniência, checkpoints, versão de modelo e regras. Inserir biblioteca A Classificar por CLI/API suportada e idempotente.
9. Revisão/homologação via GUI, CLI e MCP, com confirmação explícita para alterações na biblioteca oficial.
10. Propor economia e lixeira apenas com hash/identidade e cobertura verificada; preservar histórico, restauração e backup.

## Modelo lógico proposto
- bibliotecas_calibre (caminho, papel HOMOLOGADA/A_CLASSIFICAR, versão, status, perfil)
- campos_calibre (nome, tipo, mapeamento, obrigatoriedade, origem)
- obras (identidade canônica, título normalizado, idioma original quando conhecido)
- edicoes (obra_id, editora, edição, tradução, idioma, ano, ISBN)
- formatos_bibliograficos (edicao_id, formato, arquivo DAG, hash, estado)
- autores (identidade canônica, nomes variantes e homônimos)
- assuntos_tags (vocabulário homologado, aliases, hierarquia)
- sugestoes_classificacao (campo, valor, confiança, evidências, versão de modelo)
- decisoes_humanas (aprovação/correção/rejeição, autor, data, efeito)
- jobs_bibliograficos (estado, checkpoint, prioridade, erros, retry)
- candidatos_duplicidade (tipo EXATA/EQUIVALENTE/MESMA_OBRA, evidências, risco)
- backups_biblioteca (manifesto, verificação, restauração, cobertura)

Os nomes são conceituais; implementar migrações compatíveis com o esquema DAG atual. Separar hashes do arquivo físico e fingerprints textuais; fingerprint semântico nunca é autorização de descarte.

## Interfaces
- GUI Bancada: Biblioteca > Visão Geral, A Classificar, Homologada (leitura), Revisão, Duplicatas, Modelos IA, Configurações e Monitoramento.
- CLI sugerida: dag biblioteca perfil|scan|fila|revisar|duplicados|relatorio|homologar; dag modelos benchmark.
- MCP: consultas por biblioteca/obra/edição, propostas de revisão e relatórios. Escritas exigem escopo e confirmação real, sem autorizar descarte por prompt.
- Status: trabalhos, bytes, páginas, OCR, GPU/RAM, confiança, falhas, tempo e espaço recuperável.

## Sequência de implementação
M0 — Inventário técnico: estudar upstream Calibre, calibre-port, DAG real, esquemas e formatos; identificar biblioteca real com autorização; mapa de campos/estantes; matriz de suporte e riscos.
M1 — Biblioteca de referência e duplicata exata: perfil homologado, hashes de formatos, A Classificar, watcher, alerta de duplicata.
M2 — Extração: PDF/EPUB/OCR, metadados, índices, CBR/CBZ e formatos adicionais por adaptadores; fila e checkpoints.
M3 — IA: benchmark GGUF em GPU 16 GB, segundo parecer, tags e matching obra/edição.
M4 — UX e segurança: revisão, homologação controlada, relatórios, backups e recomendações de limpeza.
M5 — Piloto: matemática, xadrez e quadrinhos, 150 itens ouro; corrigir falhas e medir qualidade.

## Definition of Done
Todos CA-B bloqueantes com evidência; nenhum arquivo original alterado; nenhuma escrita não autorizada na biblioteca homologada; sem duplicatas falsas classificadas como exatas; relatórios reproduzíveis de benchmark; documentação, migrações, testes e guia de operação atualizados. PR de implementação separado desta branch de especificação.
