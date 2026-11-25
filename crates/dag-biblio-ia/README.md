# dag-biblio-ia

Classificação bibliográfica com IA local (Fase B, frente C; requisitos B-09, B-10, B-18, B-19, B-20, B-23).
Sem dependência de um modelo específico. Nada neste crate escreve em disco, na Biblioteca
Homologada ou no Catálogo: a entrada é um `EntradaClassificacao` em memória e a saída são
`ValorSugerido` (origem `Ia`, com evidências) mais uma fila de revisão humana.

## Fluxo

```
EntradaClassificacao ──► prompt (DADO não confiável, tags candidatas, estantes, few-shot)
        │                         │
        │                  ClienteLlm  ──► Ollama /api/chat | llama-server /v1/chat/completions
        │                         │            (JSON Schema, T=0, seed fixa, timeout, retries, limite)
        │                  validador determinístico ◄── material exatamente como o modelo o viu
        │                         │
        │            (página 1 em imagem? passada de visão ──► concordância texto×visão)
        │                         │
        └──────────► segundo parecer só se preciso ──► combinação por regras ──► ResultadoClassificacao
                                                                   └─ RegistroInferencia (B-23) por chamada
```

Uso mínimo:

```rust
let cfg = ConfigIa::de_arquivo(Path::new("modelos.json"))?;      // ver exemplos/modelos.json
let c = Classificador::da_config(&cfg, &ConfigCliente::default())?;
let r: ResultadoClassificacao = c.classificar(&entrada);          // nunca entra em pânico
```

## Backends

| Backend | Endpoint | Saída estruturada | Métricas |
|---|---|---|---|
| `ollama` (primeira classe) | `POST /api/chat` | `format` = JSON Schema; `options` = `temperature 0`, `seed`, `num_ctx`, `num_predict`; `images` em base64 | `eval_count`, `eval_duration`, `prompt_eval_*`; `/api/tags` (modelo instalado?), `/api/show` (quantização, contexto, `vision`), `/api/ps` (`size_vram`) |
| `open_ai` (llama-server, Ollama `/v1`) | `POST /v1/chat/completions` | `modo_saida`: `json_schema` (`response_format.json_schema`, padrão), `json_object_com_schema` (legado llama.cpp: `response_format {type: json_object, schema}`), `json_schema_topo` (campo `json_schema`), `gramatica` (`grammar` GBNF), `sem_restricao` | `usage` + `timings` do llama-server; `/v1/models` e `/props` (`n_ctx`, `model_path`, `build_info`, `modalities`) |

Os formatos de campo seguem a documentação do llama.cpp que conheço; **confirmar contra a versão
instalada** quando o llama-server existir no computador do Chico (hoje só há Ollama). `extras` do
perfil é mesclado no corpo (ex.: `{"think": false}` no Qwen3 via Ollama).

Cliente HTTP: `ureq` 3.5.2 com `default-features = false` (síncrono, Rust puro, sem TLS, gzip,
cookies nem JSON embutido). Justificativa: o servidor é local (loopback; endpoint remoto é recusado
sem `permitir_nao_local`, porque o texto dos livros não deve sair da máquina), o agente já usa
threads, e a árvore de dependências fica mínima. Configuração em JSON (`serde_json` já está no
workspace) em vez de TOML, para não acrescentar dependência.

### Gerenciador de processo (`servidor`)

* `ServidorLlama`: inicia `llama-server` com `-ngl` (GPU), espera `/health` (503 = carregando),
  cai para CPU (`-ngl 0`) se a GPU falhar por falta de memória ou saída inesperada, expõe
  `verificar_vivo()` (queda/OOM viram `ErroIa::MemoriaInsuficiente`/`Servidor` com o stderr) e
  `reiniciar_em_cpu()`. `Drop` encerra o processo. Nada derruba o agente (CA-B15).
* `ServicoOllama::verificar`: só checa que o serviço responde e que o modelo existe. Se faltar,
  devolve `ModeloAusente` pedindo ao usuário `ollama pull <modelo>`. **Nunca baixa modelo nem para
  o serviço.**

## Garantias de segurança e de não-invenção

* O texto do livro entra num bloco `<<<DADO_NAO_CONFIAVEL id=…>>>…<<<FIM_DADO_NAO_CONFIAVEL id=…>>>`;
  sequências `<<<`/`>>>` e caracteres de controle são neutralizados (o bloco não fecha por dentro) e o
  sistema manda ignorar qualquer comando ali. Frases típicas de injeção são detectadas e registradas
  (`RegistroInferencia.suspeitas_injecao`); trecho com ordem nunca vale como evidência. Chaves de saída
  desconhecidas (ex.: `"acao":"deletar"`) são ignoradas e registradas. Não existe campo capaz de
  expressar "deletar": a IA só descreve o livro.
* **Validador determinístico**: toda evidência precisa aparecer literalmente no material enviado
  (só normalização de espaços, pontuação, acentos e ligaduras); valor precisa de suporte nos trechos
  citados; ISBN passa no `dag_biblio::isbn` e está no trecho; DOI/data plausíveis; tags só do
  vocabulário (o resto vai para `tags_novas_propostas`); estante só do vocabulário (o DAG nunca cria
  estante); sem evidência o campo fica **desconhecido explícito** (`valor = None`). Cada rejeição sai
  com motivo (`MotivoRejeicao`).
* **Confiança determinística** (a autodeclarada pelo modelo nem é pedida: medido 1,0 em tudo):
  base 0,70 (texto literal), 0,80 (ISBN válido), 0,65 (estante), 0,55 (tags), 0,40 (resumo);
  +0,10 com 2+ evidências; +0,10 se a evidência é ficha catalográfica/ISBN validado; +0,15 se metadado
  interno/ISBN da extração/Homologada concordam; +0,15 se a visão da página 1 concorda; +0,10 × fração
  de autores já existentes na Homologada; −0,05 por evidência descartada. Teto 0,85 só IA, 0,95
  corroborada, 0,6 resumo, 0,8 idioma. Concordância entre dois modelos: `max + (1−max)/2` (até 0,97).
* **Autores com papel**: `autores = [{nome, papel: autor|organizador|tradutor|ilustrador|outro}]`.
  Só `autor` (ou `organizador` quando não há autor) entra em `Campo::Autores`; os demais vão para
  `ResultadoClassificacao.colaboradores`. "Vários autores", "Diversos", "Anônimo" etc. nunca são autor.
* **Tags**: o esquema leva `enum` com até `max_tags` (120) tags **candidatas** do vocabulário, escolhidas
  por similaridade de radicais com o material (nunca as ~1800), e `enum` com todas as estantes.
* **Segundo parecer** (B-19) quando um campo crítico (título, autores, ISBN, estante) conhecido tem
  confiança < limiar (0,75), está desconhecido (título/autores), foi rejeitado pelo validador ou diverge
  de metadado interno/Homologada. Concordância sobe a confiança; divergência deixa o campo
  desconhecido e vai para revisão humana com os dois candidatos; um modelo só sugerindo → teto 0,74 e
  revisão nos campos críticos. Segundo parecer indisponível → revisão. Primário fora do ar → o
  secundário assume sozinho, tudo marcado.
* **Visão** (página 1): se o primário é `multimodal` e há `imagem_pagina1` (ou `extracao.capa`), uma
  passada separada lê a imagem (evidência `fonte=visao_pagina_1`, sem trecho). Concordância sobe a
  confiança; divergência → `DivergenciaVisao` (o valor verificado no texto permanece, teto 0,74); valor
  só da visão entra com teto 0,6.
* **Proveniência** (B-23): `RegistroInferencia` por chamada (etapa texto/visão, papel, modelo,
  quantização real do servidor, `n_ctx`, versão do servidor, versão e hash BLAKE3 do prompt, seed,
  temperatura, tokens in/out, tempos, tokens/s, resposta bruta limitada, decisão do validador, erro).

## Benchmark (`benchmark`)

`carregar_casos(dir)` lê `*.json` (`{id, entrada: EntradaClassificacao, gabarito: {campo: valor|lista|null}}`;
`null` = deve ficar desconhecido). `executar_benchmark(casos, &[Classificador], &OpcoesBenchmark)` mede
por item: tempo, tokens in/out, tokens/s (do servidor), ms/página, VRAM antes e pico durante (amostragem
de `nvidia-smi`; Ollama também `/api/ps`), e por campo: corretos, errados, omitidos, **inventados**,
precisão e cobertura. `para_markdown()` / `para_json()`. Sem `nvidia-smi` a VRAM sai `n/d`; modelo
inacessível ou ausente sai **NÃO EXECUTADO** com o motivo; nenhum número é estimado.

## Candidatos a avaliar (configuração, não dependência)

VRAM = pesos + KV cache + overhead. Pesos ≈ parâmetros × bits/8, com ≈ 4,8 bits (Q4_K_M), 5,7 (Q5_K_M),
6,6 (Q6_K), 8,5 (Q8_0). KV cache e overhead somam ordens de 1–3 GB conforme o contexto. **São
estimativas de ordem de grandeza: medir no benchmark antes de decidir.** Verificar licença antes de usar.

| Candidato | Parâmetros | Papel sugerido | Pesos Q4_K_M | Q5_K_M | Q8_0 | Observações |
|---|---|---|---|---|---|---|
| `gemma3:12b` | ~12 B | primário (instalado) | ~7 GB | ~8 GB | ~12 GB | multimodal (visão); ver medição abaixo |
| Qwen2.5 14B instruct (`qwen2.5:14b-instruct-q4_K_M`) | ~14,7 B | segundo parecer (**a instalar pelo usuário**) | ~9 GB | ~10 GB | ~15 GB | só texto; Q4 cabe em 16 GB com contexto moderado |
| Qwen3 14B (`qwen3:14b`) | ~14,8 B | alternativa de segundo parecer (**a instalar**) | ~9 GB | ~10 GB | ~15 GB | modo "thinking": usar `extras {"think": false}` |
| Qwen2.5 / Qwen3 7B–8B instruct | 7–8 B | rápido / CPU | ~4–5 GB | ~5–6 GB | ~8 GB | bom para triagem e fallback CPU |
| Llama 3.1 8B instruct | 8 B | candidato | ~5 GB | ~6 GB | ~8,5 GB | português razoável; licença própria |
| Mistral Nemo 12B | 12 B | candidato | ~7 GB | ~8,5 GB | ~13 GB | contexto longo |
| Qwen2.5-VL 7B | ~8 B + projetor | capas/visão alternativa | ~5–6 GB | ~6–7 GB | ~9 GB | multimodal; no llama.cpp exige `--mmproj` |
| "Inkling 2 Mini" | ? | citado na spec | ? | ? | ? | **confirmar existência, licença, multimodalidade e quantizações antes de usar** |

### Referência medida (computador do Chico, RTX 4080 SUPER 16 GB, 25/11/2026; só ordem de grandeza)

`gemma3:12b` via Ollama 0.33.3, T=0, seed 42, contexto 8192, `format` = schema: carga a frio ≈ 25 s;
≈ 4 s por livro; prompt ≈ 3200 tok/s; geração ≈ 59 tok/s; VRAM do modelo ≈ 7,8 GB; visão da página 1
3–4,5 s. Não são metas nem garantias: o benchmark deste crate deve reproduzi-los. Por isso o timeout
padrão do cliente é folgado (300 s por tentativa) e a primeira chamada pode incluir a carga a frio.

## Testes

`cargo test -p dag-biblio-ia`: servidor HTTP falso em `127.0.0.1:0` (respostas roteirizadas: boa,
evidência inventada, ISBN inválido, tag fora do vocabulário, JSON quebrado, timeout/retries, 5xx, OOM,
resposta grande demais, divergência e concordância entre modelos, injeção de prompt, visão), benchmark
com servidores falsos e gerenciador de processo com um `llama-server` falso (script POSIX).
