# Protocolo da Curadoria Bibliográfica (agente <-> CLI, MCP, GUI)

Contrato em `dag-core/src/biblioteca.rs` (tipos) e `dag-core/src/comando.rs` (variantes `Biblioteca*`);
execução em `dag-agent/src/biblioteca/`. Só o agente escreve no Catálogo (ADR 0004). Os comandos
são aditivos: o serde dos comandos da Fase A não mudou.

Toda resposta bibliográfica é `Resposta::Biblioteca { dados }`, com `dados` etiquetado por `tipo`
(`snake_case`). Falhas voltam como `Resposta::Erro { codigo, mensagem }`.

## Regras que valem para todos os comandos

- A Biblioteca Homologada é somente leitura: SQLite `mode=ro` (nunca `immutable`); nenhuma escrita
  nos arquivos nem no `metadata.db`. A única via que a altera é `BibliotecaHomologar` com
  aprovação de pessoa.
- Nada apaga, move ou altera arquivo catalogado. O descarte continua sendo o fluxo da Fase A
  (Marcação para Deleção + Lixeira do Volume, ADR 0005). Os campos `decisao` das duplicatas são
  só sugestões; com autor MCP a decisão sai sempre bloqueada (iniciativa automática).
- Contêineres são lidos em memória (ADR 0006); o MCP nunca recebe senha de volta.
- O MCP só PROPÕE homologação: `BibliotecaHomologar` com `aprovacao` vinda do MCP dá
  `aprovacao_humana_exigida`.
- `calibredb` com prefixo `unshare` só se `usar_unshare` for `true` (padrão `false`).

## Comandos

| Comando | Campos | Resposta (`dados.tipo`) | Escreve? |
|---|---|---|---|
| `BibliotecaRegistrar` | `caminho`, `papel` (`homologada`/`a_classificar`), `nome?` | `registrada` {biblioteca, criada, avisos} | sim |
| `BibliotecaListar` | - | `bibliotecas` {itens} | não |
| `BibliotecaPerfil` | `biblioteca` (id ou caminho) | `perfil` {biblioteca, perfil, campos} | não |
| `BibliotecaSincronizar` | `biblioteca` | `sincronizada` {resumo} | sim (job) |
| `BibliotecaDuplicatas` | `raiz?`, `limite?`, `cursor?` | `duplicatas` {grupos, bytes_totais_pagina, bytes_sugeridos_pagina, alertas_emitidos, proximo_cursor} | sim (candidatos e alertas) |
| `BibliotecaEnfileirar` | `raizes`, `motivo?` (`novo`/`alterado`/`reprocessar`) | `enfileirada` {resumo} | sim (job) |
| `BibliotecaFila` | `estado?`, `limite?`, `cursor?` | `fila` {contagem, itens, proximo_cursor} | não |
| `BibliotecaProcessar` | `limite?`, `ia` | `processada` {resumo} | sim (job) |
| `BibliotecaRevisao` | `limite?`, `cursor?` | `revisao` {itens, total_pendentes, proximo_cursor} | não |
| `BibliotecaDecidir` | `itens` [{alvo, campo, acao, valor?}], `lote?` | `decidida` {lote, aplicadas, ignoradas} | sim |
| `BibliotecaDesfazer` | `lote` | `desfeita` {lote, desfeitas} | sim |
| `BibliotecaCadastrar` | `itens` (file_id) | `cadastrada` {resultados, bloqueado?} | sim (job) |
| `BibliotecaHomologar` | `item` (id do cadastro), `aprovacao?` {quem, quando?, motivo?} | `homologacao` {homologacao} | sim (job) |
| `BibliotecaAlertas` | `nao_vistos`, `limite?`, `cursor?` | `alertas` {itens, nao_vistos_total, proximo_cursor} | não |
| `BibliotecaMarcarVisto` | `ids` | `vistos` {marcados} | sim |
| `BibliotecaResumo` | `desde?` (Unix s; padrão 24 h) | `resumo` {resumo} | não |
| `BibliotecaRelatorioEspaco` | `volume?` (id ou rótulo) | `espaco` {relatorio} | não |
| `BibliotecaConfigObter` | - | `config` {config} | não |
| `BibliotecaConfigDefinir` | `alteracoes` (merge-patch JSON) | `config` {config} | sim |

`alvo` de uma decisão é `{ "tipo": "arquivo"|"edicao"|"obra", "id": n }`. Cursores são strings
opacas (keyset, nunca `OFFSET`). `somente_leitura()` lista os comandos que não escrevem; o
`servidor` trata `Sincronizar`, `Duplicatas`, `Enfileirar`, `Processar`, `Cadastrar` e
`Homologar` como trabalho (`TipoJob::Biblioteca`), com pausa/cancelamento entre itens.

## Fluxo

1. `Registrar` a Homologada; `Sincronizar` (espelho, hashes do Catálogo, instantâneo antes/depois:
   `biblioteca_intacta`).
2. `Duplicatas` (exatas, só índices, sem OCR) e alertas "você já tem este livro".
3. `Enfileirar` (barato) e `Processar`: duplicata exata antes de qualquer extração; extração
   reaproveitada se arquivo e regras não mudaram; uma sugestão reconciliada por campo; Obra >
   Edição > Formato idempotentes; candidatos de duplicidade bibliográfica.
4. `Revisao` / `Decidir` / `Desfazer`; `Cadastrar` na A Classificar; `Homologar`.

## Vigia (B-25, CA-B12)

Dormente até a primeira sincronização. Depois, a cada varredura concluída e a cada
reconciliação do monitor, lê os eventos `criado`/`alterado`/`renomeado` novos (marca-d'água em
`configuracoes`, chave `biblioteca.watcher.ultimo_evento`), espera `debounce_s` (30 s), ignora
`.part`/`.crdownload`/`.tmp` e afins, desassocia sugestões se o conteúdo mudou, avisa na hora se
o hash é de um Formato homologado e enfileira. `Agente::vigiar_biblioteca()` força uma passada.

## Erros com código estável

`calibre_ocupado`, `calibredb_ausente`, `aprovacao_humana_exigida`, `aprovacao_invalida`,
`biblioteca_nao_registrada`, `sem_homologada`, `sem_a_classificar`, `cadastro_inexistente`,
`cadastro_nao_pronto`, `ja_existe_na_homologada`, `config_invalida`, `papel_invalido`.

## Configuração (`biblioteca.config`, JSON em `configuracoes`)

`destino_a_classificar`, `ocr`, `ia` (desligada; Ollama em loopback, `gemma3:12b`),
`extracoes_por_dispositivo`, `max_bytes_arquivo`, `tentativas_max`, `usar_unshare` (falso),
`politica_trava`, `homologacao` (modo local/servidor; senha só por variável de ambiente),
`debounce_s`, `extensoes_ignoradas`, `tamanho_minimo_duplicata`, `pastas_a_classificar`,
`versao_regras`. Endpoint de IA fora do loopback é recusado.

## Pontos de encaixe

`ExtratorLivro` e `ClassificadorIa` (traits em `dag-agent`): `ExtratorPadrao` usa
`dag-biblio-pdf` (PDF/DjVu) e `dag-biblio-ebook` (demais, compactados em memória);
`ExtratorIndisponivel` e `IaIndisponivel` falham de forma explícita. `Agente::com_extrator_livro`,
`com_classificador_ia`, `com_executor_calibre`, `com_processos_calibre`, `com_relogio_biblioteca`.

## Eventos (ADR 0004) e proposta para GUI/MCP

O progresso do job vai como `Progresso::Mensagem` (texto de `ProgressoBiblioteca::texto()`, no
máximo 2 por segundo) e o alerta novo como mensagem `alerta: ...`. Para não quebrar as
correspondências exaustivas da GUI e do MCP, não foram criadas variantes novas. Proposta, a
combinar com os donos desses crates: `Progresso::Biblioteca(ProgressoBiblioteca)` e
`EventoAgente::AlertaBiblioteca(AlertaInfo)`.

## Escritas em disco do código novo

Só `dag-calibre` (via `calibredb`) escreve, e só na A Classificar (criar a pasta, adicionar livro)
e, com aprovação humana, na Homologada. O código novo em `dag-agent` e `dag-store` não usa
nenhuma chamada proibida por `seguranca_disco.rs`.
