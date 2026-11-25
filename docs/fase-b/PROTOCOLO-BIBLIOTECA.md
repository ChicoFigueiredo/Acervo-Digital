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
| `BibliotecaCadastrar` | `itens` (file_id) | `cadastrada` {resultados (com `cadastro_id`), bloqueado?} | sim (job) |
| `BibliotecaCadastros` | `estado?`, `limite?`, `cursor?` | `cadastros` {itens [{cadastro_id?, file_id, titulo?, calibre_id?, estado?, pronto_para_cadastrar, erro?}], proximo_cursor} | não |
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

## Fase 3: o que o pipeline passou a fazer

- **Reconciliação real** (`dag_biblio::reconciliacao::reconciliar`): cada campo junta os
  candidatos (metadado interno, ISBN, nome do arquivo, ficha, conteúdo, OCR, Homologada, IA,
  decisões humanas). Divergência entre fontes fortes derruba a confiança e vira motivo de revisão
  (`CampoLadoALado.motivos`); a decisão humana (100) vence qualquer reprocessamento e não volta a
  ser pedida.
- **Vocabulário e estante**: o `Classificador` (TF-IDF/kNN) é treinado com o espelho da
  Homologada e com as decisões humanas (peso 3). O cache é invalidado quando o espelho ou as
  decisões mudam. As sugestões do kNN entram com origem `Conteudo` (a origem `Homologada` do
  reconciliador tem confiança fixa 0,99 e é reservada ao que vem do próprio livro homologado) e
  dizem por quê ("parecido com «Título» (similaridade 0,52), decisão sua").
- **IA com contexto**: `EntradaClassificacao` leva as tags candidatas (pré-filtradas, até 80), até
  40 estantes, autores parecidos, até 5 exemplos aprovados e a imagem da página 1/capa. Tokens e
  tempo reais gravados em `inferencias`.
- **Identidade bibliográfica** (`identidade::comparar`) só entre candidatos pré-filtrados por
  título principal (`livros_homologados.titulo_principal`, migração 0012): nunca quadrático.
- **Compactados**: cada livro dentro de um compactado é um item de revisão próprio
  (`ItemRevisaoInfo.membro`; `file_id` é o do Contêiner). A decisão do Membro usa
  `AlvoRevisao::Edicao(edicao_id)`. O Membro é codificado em `versao_regras` como
  `<regras>|m:<caminho interno>`: as chaves únicas já existentes passam a valer por Membro sem
  mexer no esquema. PDF/DjVu de dentro do compactado vão da memória (memfd) ao extrator; nada é
  extraído para disco (ADR 0006). Cadastrar um Membro é recusado com explicação.
- **Fila**: prioridade de `dag_biblio::prioridade`; aging configurável (`aging_por_hora`,
  mínimo 0,01) aplicado ao gravar a prioridade, sem mexer na coluna `ordem`; 1 extração por
  Dispositivo Físico, até `max_discos_extracao` (2) dispositivos ao mesmo tempo; pausa/cancelamento
  entre itens.
- **Homologada**: livros com `!Catalogar` ou sem tag viram itens de revisão com
  `proposta_homologada` (versão `|h`); só proposta, nada é escrito. A opção
  `tag_controle_livro_novo` (desligada) aplica a tag de controle aos livros cadastrados na A
  Classificar, nunca na Homologada.
- **Senha do servidor Calibre**: `BibliotecaConfigDefinir {"homologacao": {"senha": "..."}}`
  guarda a senha no Cofre (ADR 0006); nenhuma resposta a devolve (só `senha_no_cofre`). `senha`
  vazia apaga. Sem chaveiro utilizável: `cofre_indisponivel`.
- **Resumo (WOW-10)**: `jobs_concluidos_periodo`, `classificados_periodo`, `decisoes_periodo`,
  `duplicatas_periodo` e `duplicatas_exatas_periodo`, além de revisões pendentes.
- **Revisão** (campos novos, todos `#[serde(default)]`): `membro`, `proposta_homologada`,
  `paginas` [{numero, via_ocr, qualidade, trecho}], `paginas_ocr` e `avisos` (começa com
  `ocr: N de M páginas lidas por OCR` quando houve OCR, para distinguir PDF de imagem de PDF
  textual). `HomologacaoInfo.resumo` {titulo, autores, formatos} mostra o plano antes da aprovação.

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
`politica_trava`, `homologacao` (modo local/servidor; senha no Cofre, `senha_env` só como legado),
`max_discos_extracao`, `aging_por_hora`, `tag_controle_livro_novo`,
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
