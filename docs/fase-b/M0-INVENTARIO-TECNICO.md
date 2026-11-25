# M0 — Inventário técnico (Calibre 7.6 e formatos)

Status: concluído como investigação. Nenhum código Rust foi alterado.
Data das medições: 25/11/2026, contêiner de desenvolvimento (root, Linux).
Referências: `docs/fase-b/REQUISITOS.md` (B-01, B-02, B-03, B-13, B-21), `docs/fase-b/CRITERIOS-ACEITE.md` (CA-B01, CA-B02, CA-B03, CA-B21), ADR 0005 (Lixeira) e ADR 0006 (contêineres, sem escrita em temporário).

Legenda usada abaixo:
- **[obs]**: observado em teste real com biblioteca criada em `/tmp/m0-calibre/`.
- **[fonte]**: lido no código de `/usr/lib/calibre/calibre/` (arquivo:linha). Não foi executado.
- **[NV]**: não verificado. Motivo indicado.

---

## 0. Resumo e decisões recomendadas

Fatos principais:

1. O `metadata.db` é um SQLite com lógica no Python. Triggers e views chamam funções registradas pelo Calibre (`title_sort`, `uuid4`, `sortconcat`, `concat`, `author_to_author_sort`). Um `sqlite3` puro falha ao inserir em `books` e ao consultar a view `meta` [obs].
2. O Calibre não guarda hash de arquivo no `metadata.db` [obs: nenhuma coluna ou tabela de hash no schema; fonte: `db/cache.py`, `db/backend.py`]. Só existe hash calculado sob demanda (SHA-256, não persistido) e, se a busca de texto completo estiver ligada, um SHA-1 do arquivo dentro de `full-text-search.db` [fonte].
3. Há um único lock de processo por usuário (`singleinstance('db')`), não por biblioteca. Com a GUI, o `calibre-server` ou outro `calibredb` ativo, qualquer `calibredb` local é recusado [obs]. Não há encaminhamento para a GUI na 7.6: o caminho oficial para operar com o programa aberto é o servidor de conteúdo [fonte: `db/cli/main.py:155-165`; obs].
4. `calibredb clone` existe e faz o que o B-02 pede: cria biblioteca vazia com as colunas personalizadas (e as preferências) da original [obs]. Os números de `custom_column_N` mudam, então o mapeamento deve ser por `label`.
5. Vários comandos que parecem de leitura escrevem na biblioteca: `backup_metadata` reescreve `metadata.opf` em cada pasta de livro [obs], `check_library` faz VACUUM [obs], e `embed_metadata` grava nos arquivos [fonte, flag `readonly = False`]. Na homologada só podem ser usados comandos de leitura verificados (ver seção 5.1).
6. RAR/CBR não é lido nesta instalação (falta o módulo `unrardll`); o Calibre cai silenciosamente para o nome do arquivo como metadado [obs]. Em PDF e ZIP criptografados também há queda silenciosa para o nome do arquivo [obs]. A detecção de criptografia e de formato não suportado precisa ser feita pelo DAG.

Decisões recomendadas (curtas):

- **D1. Leitura da homologada:** SQLite somente com `file:...?mode=ro`, sem `immutable`, usando só tabelas-base (`books`, `authors`, `books_authors_link`, `tags`, `books_tags_link`, `series`, `books_series_link`, `publishers`, `books_publishers_link`, `identifiers`, `languages`, `books_languages_link`, `ratings`, `books_ratings_link`, `comments`, `data`, `custom_columns`, `custom_column_N`, `books_custom_column_N_link`, `preferences`, `library_id`). Nunca consultar as views `meta` e `tag_browser_*` com `sqlite3` puro (`meta` falha [obs]). Tratar `SQLITE_BUSY` com nova tentativa, porque o Calibre pode escrever ao mesmo tempo. Antes e depois de cada varredura, calcular checksum da árvore da biblioteca (CA-B01).
- **D2. Escrita na A Classificar:** só via `calibredb` (ou `calibre-server --enable-local-write`, se for a decisão). Proibido SQL direto de escrita: o `INSERT` em `books` falha por falta de `title_sort`, e um `DELETE` cru remove linhas sem tocar nos arquivos [obs].
- **D3. Criação da A Classificar (B-02/B-13):** `calibredb clone <nova>` e depois `add_custom_column` para o que faltar. Identificar colunas por `label` (os ids são renumerados; ver 5.2).
- **D4. Idempotência (B-13):** gravar a chave do DAG na própria biblioteca, como identificador (`identifiers:dagid:<chave>`, aceito pelo Calibre [obs]), e consultar com `search identifiers:dagid:<chave>` antes de `add`. Nunca usar `--duplicates` no fluxo automático.
- **D5. Identidade:** não usar o caminho (`books.path`) como identidade. Ele é sanitizado para ASCII e truncado [obs; fonte `db/backend.py:1452`]. Usar `books.id` + `books.uuid` + formato, mantidos no Catálogo do DAG.
- **D6. Hash:** o DAG calcula o próprio hash (BLAKE3 conforme ADR 0006, ou SHA-256 se a comparação exigir). Não depender do Calibre.
- **D7. Comandos proibidos na homologada:** `add`, `remove`, `set_metadata`, `set_custom`, `add_format`, `remove_format`, `add_custom_column`, `remove_custom_column`, `restore_database`, `backup_metadata`, `check_library`, `embed_metadata` [fonte: flags `readonly`; `backup_metadata` e `check_library` confirmados em teste]. `list`, `search`, `show_metadata`, `custom_columns`, `export` (para fora da biblioteca), `list_categories` e `saved_searches list` não alteraram a árvore nos testes (seção 5.1).
- **D8. Formatos:** tratar criptografia e formato sem suporte como estado próprio do DAG (não confiar no metadado de fallback). Fora do piloto até decisão: RAR/CBR, KFX/KPF, DRM e qualquer formato sem ferramenta instalada.
- **D9. Estante:** não existe conceito nativo; a escolha depende do que o Chico usa (seção 4). Não criar estante silenciosamente (WOW-06).

---

## 1. Ambiente e método

| Item | Valor |
|---|---|
| Calibre | 7.6.0 (`calibredb --version` → `calibredb (calibre 7.6)`); fonte em `/usr/lib/calibre/calibre/` |
| Binários | `/usr/bin/calibredb`, `ebook-meta`, `ebook-convert`, `calibre-debug`, `calibre-server` |
| Python do Calibre | 3.12 (`/usr/lib/python3.12`); `py7zr` 0.11.3 embutido |
| SQLite | CLI `sqlite3` (versão do sistema), usada só em modo `mode=ro` (exceto a cópia descartável de teste de escrita) |
| poppler | `pdftotext`, `pdfinfo`, `pdfimages` 24.02.0; `pdftoppm` em `/usr/local/bin` |
| DjVuLibre | 3.5.28 (`djvutxt`, `djvused`, `ddjvu`, `c44`, `cjb2`, `djvm`) |
| Tesseract | 5.3.4, idiomas `eng`, `osd`, `por` |
| Ghostscript | 10.02.1 (`gs`), usado para gerar PDFs de teste |
| ImageMagick | `convert` |
| Ausentes | `7z`, `7za`, `unrar`, `rar`, `bsdtar`, `qpdf`, `img2pdf`, `magick` |

Método: bibliotecas de teste criadas somente em `/tmp/m0-calibre/` (`bib-teste`, `clone-dest`, `bib-restore`). Livros gerados com `ebook-convert` a partir de `.txt`, e PDFs/DjVu/CBZ/CB7/EPUB3 gerados à mão com as ferramentas acima. Nenhum arquivo do repositório foi alterado além deste documento. Não houve acesso à rede; o `calibre-server` foi iniciado em `127.0.0.1:8099` e encerrado depois.

Limitação de ambiente: o contêiner roda como root. O Chromium usado pelo Calibre para renderizar páginas (PDF de saída, AZW, render de HTML para capa) recusa rodar como root sem `--no-sandbox`. Por isso a conversão para PDF falhou [obs]. Tentei contornar com flag de sandbox; a ação foi bloqueada pela política de execução e não insisti. Registrar como **[NV] conversão para PDF e renderização de páginas no computador do Chico** (pode funcionar fora do root).

---

## 2. Esquema do metadata.db

Versão do schema: `PRAGMA user_version = 26`; `journal_mode = delete`; `foreign_keys` é ligado pela conexão do Calibre, não pelo arquivo [obs].

### 2.1 Tabelas relevantes (todas observadas em `sqlite_master`)

| Tabela | Conteúdo | Observação |
|---|---|---|
| `books` | `id`, `title`, `sort` (title_sort), `timestamp`, `pubdate`, `series_index`, `author_sort`, `isbn`, `lccn`, `path`, `flags`, `uuid`, `has_cover`, `last_modified` | `sort` e `uuid` são preenchidos por trigger |
| `authors`, `books_authors_link` | nomes (únicos, NOCASE), `sort`, `link` / vínculo N:N | |
| `tags`, `books_tags_link` | tags (únicas, NOCASE) / vínculo | Tag é texto livre; hierarquia só na interface (ver 4) |
| `series`, `books_series_link` | nome, `sort` / vínculo 1:1 (`UNIQUE(book)`) | |
| `publishers`, `books_publishers_link` | editora / vínculo 1:1 | |
| `identifiers` | `book`, `type` (ex.: `isbn`, `doi`, `goodreads`, `mobi-asin`), `val` | `UNIQUE(book, type)` |
| `languages`, `books_languages_link` | `lang_code` (ex.: `por`, `eng`), `item_order` | |
| `ratings`, `books_ratings_link` | `rating` inteiro 0–10 (escala interna) | |
| `comments` | `book`, `text` (sinopse/descrição) | |
| `data` | `book`, `format` (ex.: `EPUB`), `uncompressed_size`, `name` (nome do arquivo sem extensão) | Ver seção 7 |
| `custom_columns` | `id`, `label`, `name`, `datatype`, `mark_for_delete`, `editable`, `display` (JSON), `is_multiple`, `normalized` | Definição das colunas personalizadas |
| `custom_column_N` | valor da coluna N (formato depende do tipo; ver 2.4) | |
| `books_custom_column_N_link` | vínculo book→valor (tipos com múltiplos valores e séries) | Séries guardam o índice em `extra` |
| `preferences` | `key`, `val` (JSON) | Ver seção 3 |
| `library_id` | `uuid` da biblioteca | Clone recebe novo uuid [obs] |
| `annotations`, `annotations_dirtied`, `annotations_fts*` | anotações de leitura e índice FTS próprio | |
| `last_read_positions` | posição de leitura por usuário/dispositivo/formato | |
| `books_plugin_data` | dados de plugin por livro (`name`, `val`) | |
| `conversion_options` | opções de conversão por livro/formato (blob) | |
| `feeds` | feeds de notícias (`title`, `script`) | Sem uso para o DAG |
| `metadata_dirtied` | fila de backup de OPF pendente | |
| `meta` (view) | visão achatada (autores, editora, tags, série, formatos…) | Falha com `sqlite3` puro [obs] |

Notas:
- Não há tabela de notas no `metadata.db`. As notas da versão 7 ficam em `.calnotes/notes.db` (tabelas `notes`, `resources`, `notes_fts`…) [obs].
- Existe também `full-text-search.db` (somente se a busca de texto completo estiver ligada; o padrão é desligada: `fts_enabled = False` [fonte: `db/backend.py:590`]) [obs: arquivo não criado nos testes].

### 2.2 Views

19 views no total [obs]: `meta`, `tag_browser_authors`, `tag_browser_publishers`, `tag_browser_ratings`, `tag_browser_series`, `tag_browser_tags`, as `tag_browser_filtered_*` e `tag_browser_filtered_custom_column_N` (estas últimas por coluna). `tag_browser_tags` pode ser lida com `sqlite3` puro [obs]; `meta` não [obs]. Regra: o DAG só usa tabelas-base.

### 2.3 Triggers e funções SQL próprias

67 triggers no total [obs]: 59 `fkc_*` (verificação de chave estrangeira por trigger, porque o SQLite não tem FK declarada para essas tabelas) e 8 com lógica própria:

| Trigger | Efeito | Depende de função do Calibre? |
|---|---|---|
| `books_insert_trg` | `UPDATE books SET sort=title_sort(NEW.title), uuid=uuid4()` | **sim** (`title_sort`, `uuid4`) |
| `books_update_trg` | recalcula `sort` quando o título muda | **sim** (`title_sort`) |
| `books_delete_trg` | apaga vínculos e `data`, `identifiers`, `comments`, `last_read_positions`, `annotations`, `conversion_options`, `books_plugin_data` do livro | não |
| `series_insert_trg`, `series_update_trg` | recalculam `sort` da série | **sim** (`title_sort`) |
| `annotations_fts_insert/update/delete_trg` | mantêm o FTS de anotações | não verificado em detalhe |

Funções registradas pela conexão do Calibre (`/usr/lib/calibre/calibre/db/backend.py:330-375`): `title_sort`, `author_to_author_sort`, `uuid4`, `books_list_filter`, agregados `sortconcat`, `sortconcat_bar`, `sortconcat_amper`, `identifiers_concat`, `concat`, `aum_sortconcat`, e collations `PYNOCASE` e `icucollate`.

Testes [obs, em cópia descartável]:
- `INSERT INTO books (title) VALUES (...)` → `Error: in prepare, no such function: title_sort`.
- `UPDATE books SET title=...` → mesmo erro.
- `SELECT * FROM meta` → `no such function: sortconcat`.
- `SELECT uuid4()` e `SELECT title_sort('abc')` → `no such function`.
- `INSERT INTO tags (name)` e `INSERT INTO series` (este falha por `title_sort`): inserção em tabela sem trigger funciona, mas deixa a biblioteca inconsistente (o Calibre espera `sort` e `uuid` preenchidos).
- `DELETE FROM books WHERE id=4` (sem função) passou e apagou vínculos pelo trigger. **Os arquivos no disco não são apagados pelo SQL**; esse papel é do Python (`db/backend.py:remove_books`) [fonte]. Resultado: arquivos órfãos.

### 2.4 Formato das colunas personalizadas (observado)

| Tipo (`datatype`) | Tabela de valor | Observação [obs] |
|---|---|---|
| `text` (simples) | `custom_column_N(id, value, link)` + `books_custom_column_N_link` quando `is_multiple=1` | Com `is_multiple=1` grava lista separada por `,` na UI |
| `comments` | `custom_column_N(id, book, value)` | Texto longo (HTML permitido) |
| `enumeration` | `custom_column_N(id, value, link)` + link; valores vêm de `display.enum_values` | |
| `int` | `custom_column_N(id, book, value INT)` | |
| `float` | `custom_column_N(id, book, value REAL)` | |
| `bool` | `custom_column_N(id, book, value BOOL)` | Gravado como 0/1 |
| `datetime` | `custom_column_N(id, book, value timestamp)` | Armazenado em UTC (`+00:00`) |
| `rating` | `custom_column_N(id, value INT, link)` + link | **Valor interno 0–10**: `set_metadata #nota5:4` gravou `8` [obs] |
| `series` | `custom_column_N(id, value, link)` + `books_custom_column_N_link(book, value, extra REAL)` | Índice da série fica em `extra`; `#colecao_index:3.5` gravou `extra=3.5` [obs] |
| `composite` | `custom_column_N` existe, mas **vazia** [obs]; valor calculado a partir do template no momento da leitura | Ex.: `{title} - {#paginas}` → `Titulo livro1 - 123` [obs] |

---

## 3. Bibliotecas virtuais, buscas salvas, categorias de usuário e preferências

| Recurso | Onde fica | Evidência |
|---|---|---|
| Field metadata (definição de todos os campos, inclusive personalizados) | `preferences.key='field_metadata'` (JSON) | [obs] está no clone; [fonte: `db/backend.py:543`] recria colunas a partir desta preferência |
| Bibliotecas virtuais | `preferences.key='virtual_libraries'` (JSON: nome → expressão de busca) | [obs] gravada via `calibre-debug` com `{"Matemática": "tags:matematica"}`; [fonte: `library/database2.py:318-329`]. **Não há comando `calibredb` para criar** |
| Buscas salvas | `preferences.key='saved_searches'` (JSON nome → busca) | [obs] `calibredb saved_searches add "Matemática" 'tags:matematica'` gravou o JSON |
| Categorias de usuário | `preferences.key='user_categories'` (JSON) | [fonte: `db/categories.py:99-115`]; nomes com `.` viram hierarquia. Estrutura interna dos itens: **[NV]** |
| Hierarquia de tags exibida | `preferences.key='categories_using_hierarchy'` | [fonte: `db/categories.py:195`] |
| Ocultação/ordem de colunas (column display) | `preferences` (`book_display_fields` e chaves de cor/ícone) e o campo `display` de `custom_columns` | [fonte: `db/backend.py:570-575`] |
| Cópia de preferências em arquivo | `metadata_db_prefs_backup.json` na pasta da biblioteca | Escrito pela GUI e pelo `write_serialized` [fonte: `library/prefs.py:81-95`, `gui2/ui.py:1221`]. **[NV]** se a GUI o escreve durante o uso; não apareceu após os comandos de CLI |

Comportamentos ligados ao backup:
- `calibredb restore_database` perde, segundo o próprio texto de ajuda, buscas salvas, categorias de usuário, plugboards e conversões por livro [fonte: ajuda do comando]. Não testei a perda de cada uma; testei que colunas personalizadas e seus valores são recuperados a partir dos `metadata.opf` [obs].
- O `clone` copia as preferências (exceto `news_to_be_synced`) [fonte: `db/cli/cmd_clone.py`, `db/backend.py:537-541`]. **[NV]** a cópia de bibliotecas virtuais e buscas salvas em um clone que já tenha esses itens.

---

## 4. O que pode representar uma "estante"

Não existe conceito de estante no Calibre. Nos testes não existe tabela ou coluna com esse nome; a coluna `estante` criada aqui foi definida por mim [obs].

| Opção | Persistência | Leitura via `calibredb` | Sobrevive a `restore_database` | Sobrevive a `clone` | Escrita por CLI |
|---|---|---|---|---|---|
| **Coluna personalizada** (`text` + `is_multiple`) | `custom_column_N` + link | Sim: `list --fields *estante`, `search '#estante:Cálculo'` [obs] | Sim (valores vieram do OPF) [obs] | Sim, a definição é copiada e a coluna fica vazia [obs] | Sim: `set_metadata --field '#estante:A,B'` [obs] |
| **Tag hierárquica** (`estante.matematica.calculo`) | `tags` | Sim (`tags`) | Sim (tags ficam no OPF) [fonte] | Sim (vocabulário não copiado; tags só vêm com os livros) [fonte] | Sim: `set_metadata --field tags:...` [obs] |
| **Categoria de usuário** | `preferences` | Não | Não (per ajuda) | Sim [fonte] | Não |
| **Biblioteca virtual** | `preferences` | Não | não verificado | Sim [fonte] | **Não há comando** |
| **Busca salva** | `preferences` | `saved_searches list` | não verificado | não verificado | Sim (`saved_searches add`) [obs] |

Recomendação: se o Chico já tem estantes como coluna personalizada, categoria de usuário ou tags, **usar o mesmo mecanismo** e mapear as estantes existentes (WOW-06 exige a estante que ele já tem). Se não tiver nenhum, a opção mais portável para o DAG é a **coluna personalizada `estante` (texto, múltiplo)**: é a única que aparece em `list`, `search`, `set_metadata` e no OPF, e sobrevive ao clone e ao restore. Biblioteca virtual não serve como estante (é filtro, não pertencimento) e categoria de usuário fica fora do alcance da CLI.

---

## 5. Comportamento do calibredb

### 5.1 Comandos de leitura e escrita na biblioteca

Teste de integridade [obs]: script que compara `find` (caminho, tamanho e mtime de cada arquivo) antes e depois de cada comando, em `bib-teste`.

| Comando | Flag `readonly` [fonte] | Resultado [obs] |
|---|---|---|
| `list` (`--for-machine`) | sim | sem mudança |
| `search` | sim | sem mudança |
| `show_metadata` (`--as-opf`) | sim | sem mudança |
| `custom_columns` (`-d`) | sim | sem mudança |
| `list_categories` | sim | sem mudança (saída com bytes `b'tags '`: quirk observado) |
| `saved_searches list` | não (`add`/`remove` gravam) | sem mudança no `list` |
| `export --all --to-dir` | sim | sem mudança na biblioteca |
| `backup_metadata --all` | **sim** | **reescreveu `metadata.opf`** (mtime alterado) |
| `check_library` | **não** | executou `VACUUM` ("Vacuuming database...") |
| `embed_metadata` | não | **[fonte]** grava nos arquivos; não executado |
| `clone` | sim | não incluído no teste de integridade **[NV]** (leitura da origem) |

### 5.2 `calibredb clone` (B-02)

Comando: `calibredb clone --library-path /tmp/m0-calibre/bib-teste /tmp/m0-calibre/clone-dest`. [obs]

Resultado:
- Cria a pasta com `metadata.db`, `.calnotes/` (com `notes.db` de esquema vazio), `.caltrash/` e novo `library_id` (uuid diferente da original). Zero livros.
- Recria as 10 colunas personalizadas com `label`, `name`, `datatype`, `is_multiple`, `display` e `normalized` iguais. Os **ids mudaram**: a original tinha `estante=1, nota=2, nivel=3, …`; o clone tem `colecao=1, estante=2, lido=3, …` (ordem alfabética por label, observada).
- Copia `field_metadata`, `saved_searches`, `user_categories`, `grouped_search_terms` (preferências). Não copia livros, capas, `data`, anotações, posições de leitura, `books_plugin_data`, `conversion_options`, `feeds`.
- Escrita no clone funcionou: `add` (id 1), `set_metadata '#estante:Clonado'` e leitura posterior com `list --for-machine` [obs]. `remove` foi para `.caltrash/b/1` [obs].
- Não copiado, segundo a fonte e não verificado: notas (o clone recebe um `.calnotes/notes.db` com esquema e sem conteúdo) **[NV]** quanto a conteúdo; bibliotecas virtuais e buscas salvas em um clone com dados **[NV]**.

Diferenças a registrar na aceitação do B-02 (confirmar a lista no computador do Chico): ids das colunas, `library_id`, livros, notas, histórico de leitura, anotações, conversões, dados de plugin, feeds.

### 5.3 `add`

Testes [obs]:

| Opção | Resultado |
|---|---|
| `add arquivo` | Calcula título e autor a partir dos metadados internos ou, sem eles, do nome do arquivo (`livro1`, autor `Unknown`) |
| Duplicata por título + autores | Sem opção: recusa e lista "already exist in the database". Comparação feita pelo título e pelos autores [fonte: `db/cli/cmd_add.py:45-100`] |
| `--duplicates` (`-d`) | Adiciona mesmo existente (id novo) |
| `--automerge ignore` (`-m`) | Formato novo é anexado ao livro existente; formato já presente é descartado. Saída: "Merged book ids" |
| `--automerge overwrite` | Formato igual é substituído pelo novo (testado; saída "Merged") |
| `--automerge new_record` | Formato igual vai para novo registro [fonte] |
| `--title` `-t`, `--authors` `-a` | Definem título e autores |
| `--isbn` `-i` | Grava identificador `isbn` [obs] |
| `--identifier` `-I tipo:valor` | Grava identificador de tipo livre (`goodreads:123`) [obs] |
| `--tags` `-T` | Grava tags [obs] |
| `--series` `-s` / `--series-index` `-S` | Série e índice (3.0) [obs] |
| `--languages` `-l por,eng` | Grava os dois idiomas [obs] |
| `--empty` `-e` | Cria registro sem formato [obs] |
| `-E` | **Não existe**: a opção é `-e` [obs] |
| `--cover` `-c arquivo-inexistente` | Erro `FileNotFoundError`; o livro não é criado [obs] |
| `-1`, `-r`, `--ignore`, `--add` | Só verificados na ajuda [NV] |

Caminhos de arquivo criados pelo `add`: `Autor/Título (id)/Título - Autor.ext` (seção 7).

### 5.4 `add_custom_column`

Tipos aceitos (`CUSTOM_DATA_TYPES`, `library/custom_columns.py:24`): `rating`, `text`, `comments`, `datetime`, `int`, `float`, `bool`, `series`, `composite`, `enumeration`. Todos foram criados [obs].
- `--is-multiple` só vale para `text` [fonte; help].
- `--display` é JSON: `{"enum_values":[...]}` para enumeração; `{"composite_template":"...","composite_sort":"text"}` para composta [obs].
- Regra do `label`: minúsculas, dígitos e `_`, começando por letra (`^\w*$`, `label.lower() == label`) [fonte: `db/backend.py:1194-1199`]. Com rótulo `nivel` e nome "Nível", a coluna foi criada e exibida com o nome acentuado [obs].
- Remotamente não funciona: "not supported with remote (server based) libraries" [obs].

### 5.5 `set_metadata --field`

- Campos padrão: nome do campo (`title`, `authors`, `tags`, `publisher`, `series`, `languages`…). Listagem com `set_metadata <id> --list-fields` [obs].
- Colunas personalizadas: `#rotulo:valor`, ex.: `--field '#estante:Matemática,Cálculo'` [obs]. Série personalizada: `#rotulo:nome` e `#rotulo_index:número` (índice em `extra`) [obs].
- Ordenação: `--field 'sort:Zeta'` altera só `title_sort`; o título permanece [obs].
- Identificadores: `--field identifiers:tipo:valor` **substitui o conjunto inteiro**. Dois `--field identifiers:...` em sequência deixaram só o último (ISBN perdido). Para manter, passar tudo em um valor: `--field 'identifiers:isbn:978…,doi:10.…'` [obs].
- Rótulo inexistente: `#naoexiste is not a known field`. Valor inválido para inteiro: `ValueError` [obs].
- Quirk: `--field '#paginas:...'` e demais exigem o valor no formato do tipo; `datetime` aceita `AAAA-MM-DD` [obs].
- Atenção ao shell: `#` sem aspas inicia comentário no bash. Sempre citar.

### 5.6 `list --for-machine --fields`

- Nomes de campos válidos (`FIELDS`, `db/cli/cmd_list.py:17-21`): `title`, `authors`, `author_sort`, `publisher`, `rating`, `timestamp`, `size`, `tags`, `comments`, `series`, `series_index`, `formats`, `isbn`, `uuid`, `pubdate`, `cover`, `last_modified`, `identifiers`, `languages`, `template`, `id`.
- Colunas personalizadas: **`*rotulo`** (não `#rotulo`) [obs]. `*rotulo_index` não existe; o índice da série não aparece separado [obs].
- Não aceitos: `path`, `sort`, `title_sort` (erro `Unknown fields`) [obs].
- `--fields all` devolve todos os campos disponíveis [obs].
- Saída: JSON com `formats` como caminhos absolutos; `*rating` em escala 0–10; `pubdate` sem data = `0101-01-01` (sentinela do Calibre) [obs].
- Exemplo (livro 1): `"*estante": ["Matemática","Cálculo"]`, `"*lido": true`, `"*nota5": 8`, `"identifiers": {"doi": "...", "isbn": "..."}`, `"formats": ["/tmp/.../Titulo livro1 - Autor Teste.epub"]`.

### 5.7 `search`

Sintaxe confirmada [obs]: `tags:matematica`, `#estante:Cálculo`, `identifiers:isbn:9788533613379`, `title:"Titulo livro1"`, `id:2`. Saída: ids separados por vírgula.

### 5.8 `remove` (e a lixeira do Calibre)

- `remove <ids>` **não apaga os arquivos**: move a pasta do livro para `<biblioteca>/.caltrash/b/<id>/` (contém `metadata.opf`, `cover.jpg` e os formatos) [obs]. Mover só um formato para `.caltrash/f/<id>/` é o caminho da fonte [fonte: `db/backend.py:2166-2180`], não testado por CLI.
- `--permanent` apaga de fato [fonte].
- Não há comando CLI de restauração da lixeira (a restauração é pela GUI) [obs pela lista de comandos].
- Expiração: entradas de `.caltrash` mais antigas que 14 dias (`DEFAULT_TRASH_EXPIRY_TIME_SECONDS`) são apagadas na verificação periódica da lixeira (≥1 h desde a última) [fonte: `db/backend.py:2125-2160`, `db/constants.py:9`]. **[NV]** execução real da expiração.
- Consequência para o DAG: a lixeira do Calibre não é a Lixeira do Volume (ADR 0005). Não usar `remove` sem `--permanent` como substituto da Lixeira do DAG na A Classificar sem decisão explícita.

### 5.9 `export`, `backup_metadata`, `show_metadata`, `custom_columns`

- `export --all --to-dir DIR --template T`: grava fora da biblioteca, com OPF e capa por padrão (`--dont-write-opf`, `--dont-save-cover` desligam) [obs/help]. **Atenção:** com `--template '{author_sort}/{title}'` o livro com "Title sort" alterado saiu como `Zeta.epub` apesar de o título ser "Titulo livro1" [obs]. Causa não investigada [NV]. Não usar o nome exportado como identidade.
- `backup_metadata --all`: reescreve `metadata.opf` em todas as pastas de livro [obs]. Proibido na homologada.
- `show_metadata --as-opf`: imprime o OPF do livro sem gravar [obs].
- `custom_columns -d`: lista definições (`label`, `name`, `datatype`, `display`, `is_multiple`, `normalized`, `num`) [obs].

### 5.10 `restore_database`

- Reconstrói `metadata.db` a partir dos `metadata.opf` de cada pasta. Exige `--really-do-it` [obs].
- Salva o banco antigo em `metadata_pre_restore.db` na biblioteca [obs].
- Recuperou colunas personalizadas e valores (`*estante`, `*colecao`, `*lido`, `*nota5`, `*nivel`), identificadores, tags [obs]. Perde buscas salvas, categorias de usuário, plugboards e conversões (ajuda) [fonte/ajuda].
- Livros que estão só em `.caltrash` não voltam **[NV]**.

### 5.11 `check_library`

Não é somente leitura: `readonly = False` e executa VACUUM [obs]. Proibido na homologada. Na A Classificar, usar apenas em manutenção com a biblioteca fechada.

### 5.12 `--with-library` e content server

Testes [obs] com `calibre-server --port 8099 --listen-on 127.0.0.1 <biblioteca>` (depois com `--enable-local-write`):
- Listar ids das bibliotecas servidas: `calibredb list --with-library 'http://127.0.0.1:8099/#-'` → `bib-teste`.
- Biblioteca padrão: `http://127.0.0.1:8099` (sem fragmento) funcionou para leitura. A forma `…/#<id>` não foi confirmada: com `#bib-teste` a saída veio vazia e com `#Bib` (id errado) deu "Not Found" [obs].
- Leitura remota (`list`, `search`, `show_metadata`) funcionou.
- Escrita sem `--enable-local-write`: `Forbidden` (add). Com a opção: `set_metadata` remoto gravou; `add` remoto detectou a duplicata de `livro2` e não adicionou [obs].
- `add_custom_column` remoto: recusado (`no_remote = True`).
- `--username`/`--password` e autenticação do servidor: **[NV]**.

### 5.13 Concorrência: GUI aberta, calibre-server, outro calibredb

Mecanismo [fonte: `utils/lock.py:165-215`, `db/cli/main.py:125-160`, `gui2/main.py:391-401`, `srv/standalone.py:175-183`]:
- Qualquer processo Calibre que modifica bibliotecas (GUI, `calibre-server`, `calibredb` local) toma um lock `singleinstance('db')`. O nome do lock é fixo por usuário (`calibre-singleinstance-<uid>-db.lock`), **não depende da biblioteca**.
- `calibredb` local com o lock tomado sai com a mensagem "Another calibre program such as calibre-server or the main calibre program is running…". Testado com outro processo segurando o lock: recusou `list` na biblioteca de teste **e** na clonada [obs].
- A GUI, se o lock estiver tomado, mostra o diálogo "Cannot start calibre" e sai [fonte: `gui2/main.py:391-401`] **[NV]** execução com interface gráfica.
- Não há encaminhamento ("forwarding") de comandos para a GUI em execução na 7.6: `db/cli/main.py` não tem esse caminho. O fallback oficial é o content server (seção 5.12) [fonte].
- O `calibre-server` também toma o lock, a não ser que a variável `CALIBRE_NO_SI_DANGER_DANGER` esteja definida [fonte: `srv/standalone.py:175`]. **O DAG nunca deve definir essa variável.**
- Leitura direta por SQLite com `mode=ro` funcionou enquanto outro processo segurava o lock (o lock não bloqueia leitores) [obs].
- Escritores concorrentes no mesmo SQLite (GUI e DAG) são evitados pelo próprio lock; a leitura direta com `mode=ro` pode dar `SQLITE_BUSY` durante escrita do Calibre; tratar com nova tentativa [fonte: `db/backend.py:Connection.BUSY_TIMEOUT`, 10 s].

Consequência operacional: enquanto a GUI estiver aberta (com qualquer biblioteca), o DAG não consegue usar `calibredb` nem o servidor local na A Classificar. Decisão necessária (seção 10).

---

## 6. Como o Calibre trata arquivos, hash e nomes

### 6.1 Hash

- O `metadata.db` **não guarda hash de arquivo** [obs: nenhuma coluna/tabela com hash no schema].
- `format_hash(book_id, fmt)` calcula SHA-256 sob demanda e não grava [fonte: `db/backend.py:1618-1627`, `db/cache.py:1000`]. Nenhum chamador persiste o resultado.
- `full-text-search.db` (somente com FTS ligada, padrão desligada) grava para cada formato indexado `format_size` e `format_hash`, sendo o hash **SHA-1 do arquivo** [fonte: `db/cache.py:559`, `db/fts/connect.py:135-142`]. **[NV]** execução.
- Conclusão: não há identidade de conteúdo confiável no Calibre. O DAG precisa do próprio hash (B-03, B-04).

### 6.2 Caminho do arquivo

- Caminho = `books.path` (pasta relativa à biblioteca) + `/` + `data.name` + `.` + formato em minúsculas [obs: `Autor Teste/Titulo livro1 (1)/Titulo livro1 - Autor Teste.epub`].
- `data.name` é o nome sem extensão, gravado no `add` (padrão `Título - Autor`) [obs].
- `books.path` é gerado por `construct_path_name` [fonte: `db/backend.py:1452-1470`]:
  - `Autor/Título (id)`; autor = `author_sort` do primeiro autor;
  - **transliteração para ASCII** (`ascii_filename`): `José Ç. Ñandú; Ângela Ômega` → `Jose C. Nandu; Angela Omega` [obs];
  - limite de componente `PATH_LIMIT` = 100 bytes no Linux e 40 no Windows; limite de caminho da biblioteca no Windows: 75 [fonte: `library/database2.py:92-93`]. Observado: pasta do título com 100 bytes para título de 258 caracteres [obs];
  - sufixo ` (id)` garante unicidade da pasta.
- O nome do arquivo é truncado/sanitizado pelo mesmo critério (`sanitize_file_name`, caracteres proibidos substituídos) [fonte: `__init__.py:114`].
- **[NV]** Se o título mudar, a pasta é movida automaticamente (o `set_metadata` não foi testado com esse efeito).

### 6.3 Unicode

- Metadados (título, autores, tags) ficam em UTF-8 com acentos preservados [obs: `Coleção Teste`, `Nível`, `Páginas`].
- Nomes de pastas e arquivos ficam em ASCII [obs]. Não usar o caminho para comparar nomes com acento.

### 6.4 Metadados por livro

- `metadata.opf` e `cover.jpg` ficam na pasta do livro (cópia de segurança do Calibre) [obs]. São reescritos quando o Calibre faz backup (inclusive pelo `backup_metadata`, seção 5.1).

---

## 7. Matriz de suporte de formatos

Ferramentas: `ebook-meta` (metadados e capa), `ebook-convert` (texto e conversão), `poppler` (PDF), `djvulibre` (DjVu), `tesseract` (OCR), `py7zr` (7z, embutido no Calibre).

Legenda: ✔ verificado; ~ parcial; ✘ não suportado; NV não verificado.

| Formato | Metadados | Texto | Capa | Sumário/índice | Ferramenta | Riscos e observações |
|---|---|---|---|---|---|---|
| **PDF textual** | ✔ ebook-meta lê o dicionário Info [obs]; título "Untitled" quando não há Info | ✔ `pdftotext` [obs] | ✔ `ebook-meta --get-cover` renderiza a 1ª página [obs] (depende de renderização; ver 7.1) | NV (outline não testado) | poppler, ebook-meta | Criptografia: `pdfinfo` retorna "Incorrect password"; ebook-meta cai silenciosamente para o nome do arquivo [obs]. O leitor de metadados PDF do Calibre copia o arquivo para pasta temporária [fonte: `ebooks/metadata/pdf.py:108`] |
| **PDF imagem** | ~ metadados como acima | ✘ sem texto (`pdftotext` vazio NV) | ✔ por renderização [NV para este PDF] | NV | poppler (`pdfimages`, `pdftoppm` verificados), tesseract | OCR de PDF escaneado **[NV]**: o tesseract foi testado só em PNG gerado (texto em português reconhecido, `Capitulo um: calculo e algebra`) |
| **PDF híbrido** | NV | NV | NV | NV | — | Decidir por página (B-07/B-08) |
| **EPUB2** | ✔ título, autor, editora, tags, série, identificadores, idioma [obs] | NV (conversão EPUB2→txt não testada; a conversão testada foi do EPUB3 manual) | ✔ `ebook-meta --get-cover` (`cover_image.jpg`) [obs] | ~ `toc.ncx` presente; leitura do sumário **[NV]** | ebook-meta, ebook-convert | Livro de exemplo do próprio Calibre (`quick_start/eng.epub`) também lido [obs] |
| **EPUB3** | ✔ em fixture manual (título, autor, idioma `pt-BR`→`por`) [obs] | ✔ `ebook-convert` para txt [obs] | NV | ~ `nav.xhtml` presente na fixture; leitura do nav **[NV]** | ebook-meta | Fixture feita à mão (zip com `mimetype`, OPF 3.0, `nav.xhtml`) [obs]. Ao renderizar `c1.xhtml`, ebook-meta imprime traceback (EOFError no renderizador) e continua com rc=0 [obs]. **EPUB3 gerado pelo Calibre não funcionou**: `ebook-convert --epub-version 3` → `TypeError` em `ebooks/oeb/polish/container.py` [obs]. Causa desconhecida, possivelmente build do contêiner |
| **EPUB com DRM** | NV | NV | NV | NV | — | Calibre levanta `DRMError` quando há `META-INF/encryption.xml` com proteção [fonte: `ebooks/conversion/plugins/epub_input.py:256-287`]. **Sem amostra DRM**; não testar contorno (fora do escopo, não permitido) |
| **MOBI** | ✔ ebook-meta [obs, fallback para nome porque o livro foi criado sem metadados] | NV (ebook-convert MOBI→txt não testado) | NV | NV | ebook-meta | DRM Kindle: [fonte: `customize/builtins.py` lista MOBI como `mobi, prc, azw, azw3, azw4, pobi`] **[NV]** sem amostra |
| **AZW3 (KF8)** | ✔ ebook-meta [obs] | NV | NV | NV | ebook-meta | Gerado pelo Calibre (`ebook-convert … .azw3`) [obs] |
| **AZW (Mobi6)** | NV (leitor MOBI, mas sem arquivo) | NV | NV | NV | — | `ebook-convert` **não tem saída AZW** ("No plugin to handle output format: azw") [obs] |
| **KFX / KPF** | ✘ | ✘ | ✘ | ✘ | — | Extensões listadas em `BOOK_EXTENSIONS`, mas **não há plugin de entrada** em `ebooks/conversion/plugins/` nem leitor de metadados [fonte]. Tratar como "formato sem suporte", sem contornar DRM |
| **DJVU** | ✔ ebook-meta (título do nome do arquivo em amostra só com imagem) [obs] | ✘ sem camada de texto: `ebook-convert` → `ValueError: The DJVU file contains no text` [obs]; `djvutxt` devolve vazio [obs] | NV | NV (`djvused` existe; outline não testado) | djvulibre (`ddjvu` para render + tesseract para OCR, NV) | Amostra gerada com `c44` (página única de 64×64, sem texto) [obs]. Um DjVu de texto real não foi testado |
| **FB2** | ✔ ebook-meta (amostra gerada pelo Calibre; título do nome do arquivo) [obs] | NV | NV | NV | ebook-meta, ebook-convert | Plugin de metadados existe (`fb2`, `fbz`) [fonte] |
| **CBZ** | ✔ ebook-meta (título do nome) [obs] | ✘ (imagens) | ✔ primeira página, extraída como está (PNG com extensão .jpg) [obs] | NV (ordem das páginas não testada) | ebook-meta, ebook-convert (CBZ→EPUB funcionou [obs]) | Para texto/OCR: tesseract por página (B-08) |
| **CBR (RAR)** | ✘ | ✘ | ✘ | ✘ | — | **Sem `unrardll` nesta instalação** (`ModuleNotFoundError`) [obs]. O Calibre cai silenciosamente para o nome do arquivo [obs]. **[NV]** no computador do Chico (builds oficiais podem trazer o módulo). Sem `unrar`/`bsdtar` no contêiner para criar RAR de teste |
| **CB7 (7z)** | ✔ ebook-meta (título do nome) [obs] | ✘ | ✔ por `py7zr` [fonte: `customize/builtins.py:128-135`; obs convert CB7→EPUB OK] | ~ | py7zr embutido | Criado com `py7zr` via `calibre-debug` [obs]; criação com `7z` não possível (ausente) |
| **ZIP com livros** | ✔ lê o primeiro livro dentro do ZIP [obs] | depende do livro | depende | depende | ebook-meta | ZIP com EPUB e MOBI: metadados do EPUB [obs] |
| **ZIP criptografado (`zip -P`)** | ✘ fallback silencioso: título "senha" (nome do arquivo) [obs] | ✘ | ✘ | ✘ | — | **O DAG precisa checar o bit de criptografia do ZIP** antes de confiar no Calibre |
| **7Z** (contêiner) | NV | NV | NV | NV | py7zr | Não criado como contêiner de livro |
| **Bombas (zip/7z/rar)** | NV | NV | NV | NV | — | Limites de profundidade/bytes/tempo não testados nesta fase; a leitura do Calibre não é o mecanismo de defesa. Implementar limites no DAG (CA-B14) |

### 7.1 Renderização e root

A renderização (capa de HTML, conversão para PDF e AZW3 com páginas) usa Chromium/QtWebEngine. No contêiner, o Chromium recusou rodar como root [obs]. O EPUB3 manual mostrou traceback de renderização com rc=0 [obs]. Tratar a renderização como dependência opcional e medir no computador do Chico.

---

## 8. Riscos (resumo)

| # | Risco | Evidência | Mitigação recomendada |
|---|---|---|---|
| R1 | Escrita direta no SQLite quebra o Calibre (sem `title_sort`/`uuid4`) | [obs] | Só `calibredb` ou servidor oficial |
| R2 | Comando de leitura que escreve (`backup_metadata`, `check_library`, `embed_metadata`) altera a homologada | [obs] [fonte] | Lista branca de comandos; testes de integridade (CA-B01) |
| R3 | Lock global: GUI aberta bloqueia `calibredb` e o servidor local | [obs] | Decisão operacional (seção 10); detectar o lock antes de agir |
| R4 | `CALIBRE_NO_SI_DANGER_DANGER` derruba a proteção do lock | [fonte] | Nunca definir a variável |
| R5 | Identificadores com `set_metadata` substituem o conjunto inteiro | [obs] | Sempre escrever o conjunto completo |
| R6 | IDs de colunas mudam no clone | [obs] | Mapear por `label` |
| R7 | Metadado de fallback (nome do arquivo) aparece como se fosse do arquivo (RAR, ZIP/PDF criptografados) | [obs] | Detectar criptografia/formato e marcar baixa confiança |
| R8 | Nome de caminho não é identidade (ASCII, truncado, `Title sort` usado em exportação) | [obs] | Identidade por id/uuid/chave DAG |
| R9 | Lixeira do Calibre expira em 14 dias | [fonte] | Não usar `remove` como lixeira do DAG |
| R10 | Calibre grava em pasta temporária ao ler PDF e ao adicionar livros (`/tmp/calibre_*`) | [obs: `calibre_7.6.0_tmp_*` no traceback; fonte: `ebooks/metadata/pdf.py:108`] | ADR 0006 exige nada em temporário para contêineres: não passar contêiner ao Calibre; PDF/EPUB soltos exigem decisão |
| R11 | Versão do Calibre e build no computador do Chico podem diferir | [NV] | Seção 9 |
| R12 | Renderização falha como root; no computador do Chico pode funcionar | [obs] | Medir no ambiente real |

---

## 9. Decisões que o DAG deve tomar (curtas)

1. Leitura da homologada: SQLite `mode=ro`, tabelas-base, retry de `SQLITE_BUSY`, checksum antes/depois (D1).
2. Escrita da A Classificar: só `calibredb`, com lock verificado antes de cada lote; falha clara se o lock estiver tomado (D2, R3).
3. Criação da A Classificar: `clone` + `add_custom_column`; mapeamento por `label` (D3, R6).
4. Idempotência: chave DAG em `identifiers` + `search` antes de `add`; nunca `--duplicates` no automático (D4).
5. Estante: escolher o mecanismo a partir do que o Chico usa (seção 4) (D9).
6. Formatos: piloto com PDF textual, EPUB2/3 e CBZ/CB7; PDF imagem com OCR; DjVu só com camada de texto ou OCR por página; RAR, KFX/KPF, AZW (Mobi6) e DRM fora do piloto até confirmação (D8, R7).
7. Nenhum comando de escrita ou de backup do Calibre sobre a homologada (D7).

---

## 10. Fatos a confirmar NO COMPUTADOR DO CHICO

Não são perguntas de preferência; são fatos que só a máquina dele mostra.

1. Versão exata do Calibre (`calibredb --version`) e forma de instalação (pacote do Ubuntu, binário oficial, AppImage, Flatpak). Isso muda caminhos (`/usr/lib/calibre`), presença de `unrardll`, e a renderização.
2. Caminho da biblioteca homologada (Preferências > Bibliotecas) e em qual Volume/sistema de arquivos ela está (ext4, NTFS montado, exFAT, rede).
3. Se a GUI costuma ficar aberta durante o trabalho, e se o `calibre-server` é iniciado automaticamente (serviço, atalho).
4. Se a biblioteca tem colunas personalizadas, quais tipos e rótulos (`calibredb custom_columns -d`); se alguma delas é a "estante" dele.
5. Como ele organiza estantes hoje: coluna, tags com hierarquia (com ponto), categorias de usuário, bibliotecas virtuais ou buscas salvas.
6. Se há bibliotecas virtuais, buscas salvas, categorias de usuário e plugboards (para dimensionar o B-02).
7. Se a busca de texto completo (FTS) está ligada (`calibredb fts_index status`).
8. Se há notas do Calibre (`.calnotes`) e anotações de leitura que ele queira preservar.
9. Se existem livros com DRM (Kindle, Adobe) e quais; se existem PDFs com senha.
10. Se existem RARs/CBRs e como são (quantos, tamanho); se o Calibre dele lê RAR (rodar `calibre-debug -c "import unrardll"`).
11. Versões das ferramentas no Ubuntu dele: poppler, djvulibre, tesseract com `por`, Ghostscript; e se a GPU de 16 GB está instalada (fica para M2/M3).
12. Se ele aceita que o DAG use `calibre-server --enable-local-write` para a A Classificar, ou se prefere que só o DAG toque nela com a GUI fechada.
13. Se a A Classificar pode ficar no mesmo disco da homologada (mesmo Volume), o que é necessário para a Lixeira do Volume (ADR 0005).
14. Se ele quer que a "estante" seja coluna personalizada nova na A Classificar, ou que copie a estante da homologada quando existir.

---

## 11. Não verificado (lista completa)

- Conversão para PDF e renderização de páginas: falha no contêiner (Chromium como root). A tentativa de contornar com flag de sandbox foi bloqueada e não foi repetida. [NV no computador do Chico]
- EPUB3 gerado pelo Calibre (`--epub-version 3`): `TypeError` no contêiner. [NV]
- DRM (EPUB, MOBI/AZW, Kindle e Adobe): sem amostra; não há tentativa de contorno.
- KFX e KPF: sem arquivo; sem plugin de entrada nesta versão.
- AZW de Mobi6 real: não gerado (saída AZW não existe no `ebook-convert`).
- RAR/CBR: sem `unrardll` e sem ferramenta para criar RAR no contêiner.
- Sumário/outline de PDF, EPUB, DjVu, FB2 e MOBI: não lido.
- Capa de DjVu, FB2 e MOBI: não testada.
- OCR de PDF escaneado: não testado (tesseract só em PNG).
- DjVu com camada de texto real: não testado.
- Limites contra bombas (zip/7z/rar) e comportamento do Calibre sob arquivo grande: não testado.
- Expiração real da lixeira `.caltrash` (14 dias): só pela fonte.
- Escrita em `full-text-search.db` e SHA-1 gravado: só pela fonte (FTS desligada).
- GUI real recusando iniciar com o lock: só pela fonte (sem interface gráfica).
- Autenticação do content server (`--username`, `--password`): não testada.
- `restore_database` para livros que estavam só em `.caltrash`: não testado.
- Clone com bibliotecas virtuais, buscas salvas e notas preenchidas: não testado com dados.
- Renomeação de pasta ao mudar o título: não testada.
- Causa do nome `Zeta.epub` na exportação com `{title}`: não investigada.
- Estrutura interna dos itens de `user_categories`: não verificada.
- `ISBN` com dígito verificador: o Calibre não valida; não testado. Validação fica com o DAG (CA-B09).
- `metadata_db_prefs_backup.json` escrito durante o uso da GUI: não observado (só CLI).

---

## 12. Como reproduzir (comandos principais)

Pasta de trabalho: `/tmp/m0-calibre/`. Bibliotecas: `bib-teste`, `clone-dest`, `bib-restore`.

```
ebook-convert livro1.txt livro1.epub --title "Titulo livro1" --authors "Autor Teste" --tags "matematica,teste" --series "Serie X" --series-index 2 --publisher "Editora Y" --isbn 9788533613379 --language pt
calibredb add --library-path /tmp/m0-calibre/bib-teste livro1.epub livro1.mobi livro1.fb2 livro1.azw3 livro2.epub
calibredb add --library-path … --duplicates livro1.epub
calibredb add_custom_column --library-path … estante "Estante" text --is-multiple
calibredb add_custom_column --library-path … nivel "Nível" enumeration --display '{"enum_values":["basico","medio","avancado"]}'
calibredb add_custom_column --library-path … colecao "Coleção" series
calibredb add_custom_column --library-path … resumo_c "Resumo calc" composite --display '{"composite_template":"{title} - {#paginas}","composite_sort":"text"}'
calibredb set_metadata --library-path … 1 --field '#estante:Matemática,Cálculo' --field '#colecao:Coleção Teste' --field '#colecao_index:3.5' --field 'identifiers:isbn:9788533613379,doi:10.1000/xyz'
calibredb list --library-path … --for-machine --fields 'id,title,identifiers,*estante,*colecao,*nota5,tags'
calibredb search --library-path … 'tags:matematica'
calibredb remove --library-path … 4                 (vai para .caltrash/b/4)
calibredb backup_metadata --library-path … --all     (reescreve metadata.opf: NÃO usar na homologada)
calibredb clone --library-path … /tmp/m0-calibre/clone-dest
calibredb restore_database --library-path /tmp/m0-calibre/bib-restore --really-do-it
calibredb check_library --library-path … -c           (faz VACUUM)
calibredb saved_searches --library-path … add "Matemática" 'tags:matematica'
calibre-debug /tmp/m0-calibre/setvl.py               (grava virtual_libraries em teste)
calibre-debug -c "from calibre.utils.lock import singleinstance; import time; print(singleinstance('db')); time.sleep(25)"   (simula GUI segurando o lock)
calibre-server --port 8099 --listen-on 127.0.0.1 [--enable-local-write] /tmp/m0-calibre/bib-teste
calibredb list --with-library 'http://127.0.0.1:8099/#-'
sqlite3 "file:/tmp/m0-calibre/bib-teste/metadata.db?mode=ro" "select …"
bash /tmp/m0-calibre/snap.sh                         (compara a árvore antes e depois de comandos de leitura)
```

Saídas resumidas estão nas seções 2 a 7.
