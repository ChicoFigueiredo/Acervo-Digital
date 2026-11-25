-- Curadoria Bibliográfica (Fase B), migração aditiva: nada das tabelas da Fase A é alterado.
-- Convenções: STRICT; instantes em segundos Unix passados pelo chamador; hashes BLAKE3 em BLOB(32);
-- enums em TEXT com CHECK (valores iguais aos `serde(rename_all = "snake_case")` de `dag-biblio`).
-- Contadores mantidos por gatilhos na tabela `contadores` (chaves `biblio_*`), para a GUI/CLI/MCP
-- nunca fazerem COUNT(*) sobre dezenas de milhões de linhas.

-- ---------------------------------------------------------------------------------------------
-- Bibliotecas Calibre, perfil e instantâneos
-- ---------------------------------------------------------------------------------------------

-- Um caminho tem um papel só (UNIQUE caminho). Pode haver várias Homologadas (o Chico pode ter
-- mais de uma biblioteca de referência); a "A Classificar" ativa é uma só (índice parcial), pois
-- é o destino único dos cadastros criados pelo DAG e não pode ser ambíguo.
CREATE TABLE bibliotecas_calibre (
    id               INTEGER PRIMARY KEY,
    caminho          TEXT    NOT NULL UNIQUE,
    papel            TEXT    NOT NULL CHECK (papel IN ('homologada','a_classificar')),
    nome             TEXT    NOT NULL DEFAULT '',
    library_uuid     TEXT,
    versao_calibre   TEXT,
    estado           TEXT    NOT NULL DEFAULT 'ativa' CHECK (estado IN ('ativa','inacessivel','arquivada')),
    perfil_json      TEXT,
    -- Geração do espelho `livros_homologados`: sobe a cada sincronização completa; o que não foi
    -- visto na geração corrente foi removido da biblioteca.
    geracao_espelho  INTEGER NOT NULL DEFAULT 0,
    criado_em        INTEGER NOT NULL,
    atualizado_em    INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX idx_bibliotecas_uma_a_classificar
    ON bibliotecas_calibre(papel) WHERE papel = 'a_classificar' AND estado = 'ativa';

-- Cada leitura do metadata.db (somente leitura) e o resultado da comparação com a anterior.
CREATE TABLE instantaneos_biblioteca (
    id                  INTEGER PRIMARY KEY,
    biblioteca_id       INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    quando              INTEGER NOT NULL,
    hash_metadata_db    BLOB    CHECK (hash_metadata_db IS NULL OR length(hash_metadata_db) = 32),
    tamanho_metadata_db INTEGER,
    livros              INTEGER NOT NULL DEFAULT 0,
    formatos            INTEGER NOT NULL DEFAULT 0,
    manifesto_json      TEXT,
    resultado           TEXT    NOT NULL CHECK (resultado IN ('primeiro','igual','alterado','incompativel')),
    comparacao_json     TEXT
) STRICT;
CREATE INDEX idx_instantaneos_biblioteca ON instantaneos_biblioteca(biblioteca_id, quando DESC, id DESC);

-- Campos (colunas padrão e personalizadas) do Calibre e se o DAG pode clonar cada um.
CREATE TABLE campos_calibre (
    biblioteca_id INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    rotulo        TEXT    NOT NULL,
    nome          TEXT    NOT NULL,
    datatype      TEXT    NOT NULL,
    multiplo      INTEGER NOT NULL DEFAULT 0 CHECK (multiplo IN (0,1)),
    clonavel      INTEGER NOT NULL DEFAULT 1 CHECK (clonavel IN (0,1)),
    motivo        TEXT,
    PRIMARY KEY (biblioteca_id, rotulo)
) STRICT, WITHOUT ROWID;

-- ---------------------------------------------------------------------------------------------
-- Espelho de leitura da Biblioteca Homologada
-- ---------------------------------------------------------------------------------------------

CREATE TABLE livros_homologados (
    id            INTEGER PRIMARY KEY,
    biblioteca_id INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    calibre_id    INTEGER NOT NULL,
    uuid          TEXT,
    titulo        TEXT    NOT NULL,
    titulo_norm   TEXT    NOT NULL,
    autores_json  TEXT    NOT NULL DEFAULT '[]',
    isbn13        TEXT,
    idioma        TEXT,
    serie         TEXT,
    indice        REAL,
    editora       TEXT,
    tags_json     TEXT    NOT NULL DEFAULT '[]',
    estante       TEXT,
    caminho       TEXT    NOT NULL DEFAULT '',
    geracao       INTEGER NOT NULL DEFAULT 0,
    atualizado_em INTEGER NOT NULL,
    UNIQUE (biblioteca_id, calibre_id)
) STRICT;
CREATE INDEX idx_livros_homologados_isbn    ON livros_homologados(isbn13) WHERE isbn13 IS NOT NULL;
CREATE INDEX idx_livros_homologados_titulo  ON livros_homologados(titulo_norm);
CREATE INDEX idx_livros_homologados_uuid    ON livros_homologados(uuid) WHERE uuid IS NOT NULL;
CREATE INDEX idx_livros_homologados_geracao ON livros_homologados(biblioteca_id, geracao);

-- Um arquivo de formato de um livro homologado. O hash é calculado pelo DAG (nunca confiado de
-- fora) e só vale enquanto tamanho e mtime não mudam. `file_id` liga ao Catálogo quando o arquivo
-- está num Volume inventariado.
CREATE TABLE formatos_homologados (
    id            INTEGER PRIMARY KEY,
    livro_id      INTEGER NOT NULL REFERENCES livros_homologados(id) ON DELETE CASCADE,
    biblioteca_id INTEGER NOT NULL,
    formato       TEXT    NOT NULL,
    caminho       TEXT    NOT NULL,
    tamanho       INTEGER NOT NULL CHECK (tamanho >= 0),
    mtime_ns      INTEGER,
    hash          BLOB    CHECK (hash IS NULL OR length(hash) = 32),
    hash_em       INTEGER,
    file_id       INTEGER REFERENCES files(id) ON DELETE SET NULL,
    UNIQUE (livro_id, formato)
) STRICT;
-- Alerta de duplicata exata (CA-B04) em O(log n): hash, depois tamanho.
CREATE INDEX idx_formatos_homologados_hash ON formatos_homologados(hash, tamanho) WHERE hash IS NOT NULL;
CREATE INDEX idx_formatos_homologados_sem_hash ON formatos_homologados(biblioteca_id, id) WHERE hash IS NULL;
CREATE INDEX idx_formatos_homologados_file ON formatos_homologados(file_id) WHERE file_id IS NOT NULL;

-- ---------------------------------------------------------------------------------------------
-- Obra > Edição > Formato, autores
-- ---------------------------------------------------------------------------------------------

-- `chave` é a identidade canônica decidida por quem classifica (p. ex. `isbn:<13>` ou
-- `t:<titulo_norm>|a:<autor_norm>`); UNIQUE torna o reprocessamento idempotente (CA-B11).
CREATE TABLE obras (
    id               INTEGER PRIMARY KEY,
    chave            TEXT    NOT NULL UNIQUE,
    titulo           TEXT    NOT NULL,
    titulo_norm      TEXT    NOT NULL,
    idioma_original  TEXT,
    criado_em        INTEGER NOT NULL,
    atualizado_em    INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_obras_titulo ON obras(titulo_norm);

CREATE TABLE edicoes (
    id                    INTEGER PRIMARY KEY,
    obra_id               INTEGER NOT NULL REFERENCES obras(id) ON DELETE CASCADE,
    chave                 TEXT    NOT NULL UNIQUE,
    titulo                TEXT    NOT NULL,
    editora               TEXT,
    edicao                TEXT,
    idioma                TEXT,
    ano                   INTEGER,
    isbn13                TEXT,
    serie                 TEXT,
    indice_serie          REAL,
    livro_homologado_id   INTEGER REFERENCES livros_homologados(id) ON DELETE SET NULL,
    criado_em             INTEGER NOT NULL,
    atualizado_em         INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_edicoes_obra ON edicoes(obra_id);
CREATE INDEX idx_edicoes_isbn ON edicoes(isbn13) WHERE isbn13 IS NOT NULL;

-- "O EPUB da edição X". O arquivo físico continua sendo uma linha de `files`.
CREATE TABLE formatos_bibliograficos (
    id            INTEGER PRIMARY KEY,
    edicao_id     INTEGER NOT NULL REFERENCES edicoes(id) ON DELETE CASCADE,
    formato       TEXT    NOT NULL,
    file_id       INTEGER REFERENCES files(id) ON DELETE SET NULL,
    hash          BLOB    NOT NULL CHECK (length(hash) = 32),
    tamanho       INTEGER NOT NULL CHECK (tamanho >= 0),
    estado        TEXT    NOT NULL DEFAULT 'candidato'
        CHECK (estado IN ('candidato','a_classificar','homologado','duplicado','descartado','falha')),
    criado_em     INTEGER NOT NULL,
    atualizado_em INTEGER NOT NULL,
    UNIQUE (edicao_id, formato, hash)
) STRICT;
CREATE INDEX idx_formatos_biblio_hash ON formatos_bibliograficos(hash);
CREATE INDEX idx_formatos_biblio_file ON formatos_bibliograficos(file_id) WHERE file_id IS NOT NULL;

CREATE TABLE autores_biblio (
    id             INTEGER PRIMARY KEY,
    nome           TEXT    NOT NULL,
    nome_norm      TEXT    NOT NULL,
    -- Desambigua homônimos: dois autores com o mesmo nome só coexistem com marcas diferentes.
    desambiguacao  TEXT    NOT NULL DEFAULT '',
    ordenacao      TEXT,
    variantes_json TEXT    NOT NULL DEFAULT '[]',
    criado_em      INTEGER NOT NULL,
    UNIQUE (nome_norm, desambiguacao)
) STRICT;

CREATE TABLE vinculo_autoria (
    edicao_id INTEGER NOT NULL REFERENCES edicoes(id) ON DELETE CASCADE,
    autor_id  INTEGER NOT NULL REFERENCES autores_biblio(id) ON DELETE CASCADE,
    papel     TEXT    NOT NULL DEFAULT 'autor'
        CHECK (papel IN ('autor','tradutor','organizador','ilustrador','prefaciador','outro')),
    ordem     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (edicao_id, autor_id, papel)
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_vinculo_autoria_autor ON vinculo_autoria(autor_id);

-- ---------------------------------------------------------------------------------------------
-- Fila persistente de jobs bibliográficos
-- ---------------------------------------------------------------------------------------------

-- `ordem` = prioridade * 3600 - enfileirado_em: a prioridade é calculada pelo chamador e cada
-- hora de espera vale um ponto (aging, sem starvation). Como o valor não depende do "agora", o
-- índice (estado, ordem, id), percorrido de trás para frente, entrega o próximo job em O(log n).
CREATE TABLE jobs_bibliograficos (
    id              INTEGER PRIMARY KEY,
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    motivo          TEXT    NOT NULL CHECK (motivo IN ('novo','alterado','reprocessar')),
    prioridade      INTEGER NOT NULL DEFAULT 0,
    enfileirado_em  INTEGER NOT NULL,
    ordem           INTEGER GENERATED ALWAYS AS (prioridade * 3600 - enfileirado_em) VIRTUAL,
    estado          TEXT    NOT NULL DEFAULT 'pendente'
        CHECK (estado IN ('pendente','em_andamento','concluido','falhou','excecao')),
    etapa           TEXT,
    checkpoint_json TEXT,
    tentativas      INTEGER NOT NULL DEFAULT 0,
    erro            TEXT,
    versao_regras   TEXT,
    iniciado_em     INTEGER,
    atualizado_em   INTEGER NOT NULL,
    concluido_em    INTEGER
) STRICT;
-- No máximo um job ativo por arquivo.
CREATE UNIQUE INDEX idx_jobs_biblio_ativo ON jobs_bibliograficos(file_id)
    WHERE estado IN ('pendente','em_andamento');
CREATE INDEX idx_jobs_biblio_fila ON jobs_bibliograficos(estado, ordem, id);
CREATE INDEX idx_jobs_biblio_arquivo ON jobs_bibliograficos(file_id, id DESC);

-- ---------------------------------------------------------------------------------------------
-- Extração, sugestões, decisões humanas, inferências
-- ---------------------------------------------------------------------------------------------

-- `resultado` guarda o `ResultadoExtracao` serializado em JSON, puro ou comprimido
-- (`compressao`). Uma linha por (arquivo, versão das regras): reprocessar substitui.
CREATE TABLE extracoes (
    id             INTEGER PRIMARY KEY,
    job_id         INTEGER REFERENCES jobs_bibliograficos(id) ON DELETE SET NULL,
    file_id        INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    formato        TEXT    NOT NULL,
    estado         TEXT    NOT NULL CHECK (estado IN
        ('lido','drm','senha','corrompido','nao_suportado','limite_excedido')),
    resultado      BLOB,
    compressao     TEXT    NOT NULL DEFAULT 'nenhuma' CHECK (compressao IN ('nenhuma','zstd','gzip')),
    paginas_total  INTEGER,
    paginas_ocr    INTEGER NOT NULL DEFAULT 0,
    tempo_ms       INTEGER NOT NULL DEFAULT 0,
    hash_arquivo   BLOB    CHECK (hash_arquivo IS NULL OR length(hash_arquivo) = 32),
    versao_regras  TEXT    NOT NULL DEFAULT '',
    criado_em      INTEGER NOT NULL,
    UNIQUE (file_id, versao_regras)
) STRICT;

CREATE TABLE sugestoes (
    id              INTEGER PRIMARY KEY,
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    edicao_id       INTEGER REFERENCES edicoes(id) ON DELETE SET NULL,
    campo           TEXT    NOT NULL,
    valor           TEXT,
    lista_json      TEXT    NOT NULL DEFAULT '[]',
    confianca       REAL    NOT NULL CHECK (confianca >= 0.0 AND confianca <= 1.0),
    evidencias_json TEXT    NOT NULL DEFAULT '[]',
    origem          TEXT    NOT NULL,
    modelo          TEXT    NOT NULL DEFAULT '',
    versao_regras   TEXT    NOT NULL DEFAULT '',
    estado          TEXT    NOT NULL DEFAULT 'pendente'
        CHECK (estado IN ('pendente','aceita','corrigida','rejeitada')),
    criado_em       INTEGER NOT NULL,
    atualizado_em   INTEGER NOT NULL,
    UNIQUE (file_id, campo, origem, modelo, versao_regras)
) STRICT;
CREATE INDEX idx_sugestoes_pendentes ON sugestoes(file_id) WHERE estado = 'pendente';
CREATE INDEX idx_sugestoes_arquivo ON sugestoes(file_id, campo);

-- Histórico de decisões: nunca apagado; desfazer só preenche `desfeita_em`.
CREATE TABLE decisoes_humanas (
    id             INTEGER PRIMARY KEY,
    lote_id        TEXT,
    alvo_tipo      TEXT    NOT NULL CHECK (alvo_tipo IN ('arquivo','edicao','obra')),
    alvo_id        INTEGER NOT NULL,
    sugestao_id    INTEGER REFERENCES sugestoes(id) ON DELETE SET NULL,
    campo          TEXT    NOT NULL,
    acao           TEXT    NOT NULL CHECK (acao IN ('aceitar','corrigir','rejeitar')),
    valor_anterior TEXT,
    valor_novo     TEXT,
    autor          TEXT    NOT NULL CHECK (autor IN ('gui','cli','mcp')),
    quando         INTEGER NOT NULL,
    desfeita_em    INTEGER
) STRICT;
CREATE INDEX idx_decisoes_lote ON decisoes_humanas(lote_id) WHERE lote_id IS NOT NULL;
CREATE INDEX idx_decisoes_alvo ON decisoes_humanas(alvo_tipo, alvo_id, id DESC);
CREATE INDEX idx_decisoes_sugestao ON decisoes_humanas(sugestao_id) WHERE sugestao_id IS NOT NULL;

CREATE TABLE inferencias (
    id             INTEGER PRIMARY KEY,
    job_id         INTEGER REFERENCES jobs_bibliograficos(id) ON DELETE SET NULL,
    file_id        INTEGER REFERENCES files(id) ON DELETE CASCADE,
    modelo         TEXT    NOT NULL,
    versao_modelo  TEXT    NOT NULL DEFAULT '',
    hash_prompt    BLOB    NOT NULL CHECK (length(hash_prompt) = 32),
    tokens_entrada INTEGER NOT NULL DEFAULT 0,
    tokens_saida   INTEGER NOT NULL DEFAULT 0,
    tempo_ms       INTEGER NOT NULL DEFAULT 0,
    bruto_json     TEXT,
    criado_em      INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_inferencias_prompt ON inferencias(hash_prompt, modelo, versao_modelo);
CREATE INDEX idx_inferencias_arquivo ON inferencias(file_id) WHERE file_id IS NOT NULL;

-- ---------------------------------------------------------------------------------------------
-- Duplicidade e alertas
-- ---------------------------------------------------------------------------------------------

-- `bytes` é o tamanho do arquivo candidato (só importa para `exata`): ordena "recuperável por
-- bytes" por índice. Só `exata` pode gerar espaço recuperável; as demais nunca.
CREATE TABLE candidatos_duplicidade (
    id              INTEGER PRIMARY KEY,
    file_id         INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    alvo_tipo       TEXT    NOT NULL CHECK (alvo_tipo IN ('formato_homologado','arquivo','edicao')),
    alvo_id         INTEGER NOT NULL,
    tipo            TEXT    NOT NULL CHECK (tipo IN ('exata','mesma_edicao','mesma_obra','incerta')),
    evidencias_json TEXT    NOT NULL DEFAULT '[]',
    risco           TEXT    NOT NULL DEFAULT 'medio' CHECK (risco IN ('baixo','medio','alto')),
    estado          TEXT    NOT NULL DEFAULT 'aberto'
        CHECK (estado IN ('aberto','confirmado','descartado_como_falso')),
    bytes           INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    criado_em       INTEGER NOT NULL,
    decidido_em     INTEGER,
    UNIQUE (file_id, alvo_tipo, alvo_id, tipo)
) STRICT;
CREATE INDEX idx_candidatos_dup_fila ON candidatos_duplicidade(estado, tipo, bytes DESC, id);
CREATE INDEX idx_candidatos_dup_alvo ON candidatos_duplicidade(alvo_tipo, alvo_id);

-- `chave` (opcional, UNIQUE) torna a emissão idempotente: o mesmo fato não gera dois alertas.
CREATE TABLE alertas_biblio (
    id          INTEGER PRIMARY KEY,
    chave       TEXT    UNIQUE,
    tipo        TEXT    NOT NULL CHECK (tipo IN
        ('ja_tem_este_livro','duplicata_exata','mesma_edicao_outro_formato','possivel_mesma_obra',
         'extracao_falhou','biblioteca_alterada','resumo_diario','outro')),
    file_id     INTEGER REFERENCES files(id) ON DELETE CASCADE,
    dados_json  TEXT    NOT NULL DEFAULT '{}',
    criado_em   INTEGER NOT NULL,
    visto_em    INTEGER,
    silenciado  INTEGER NOT NULL DEFAULT 0 CHECK (silenciado IN (0,1))
) STRICT;
CREATE INDEX idx_alertas_nao_vistos ON alertas_biblio(id DESC) WHERE visto_em IS NULL AND silenciado = 0;
CREATE INDEX idx_alertas_periodo ON alertas_biblio(criado_em, tipo);

-- ---------------------------------------------------------------------------------------------
-- A Classificar, homologação, backups
-- ---------------------------------------------------------------------------------------------

CREATE TABLE cadastros_a_classificar (
    id                 INTEGER PRIMARY KEY,
    file_id            INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    edicao_id          INTEGER REFERENCES edicoes(id) ON DELETE SET NULL,
    biblioteca_id      INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    calibre_id         INTEGER,
    chave_idempotencia TEXT    NOT NULL,
    estado             TEXT    NOT NULL DEFAULT 'pendente'
        CHECK (estado IN ('pendente','cadastrando','cadastrado','falhou','removido')),
    erro               TEXT,
    criado_em          INTEGER NOT NULL,
    atualizado_em      INTEGER NOT NULL,
    UNIQUE (biblioteca_id, chave_idempotencia)
) STRICT;
CREATE INDEX idx_cadastros_estado ON cadastros_a_classificar(estado, id);
CREATE INDEX idx_cadastros_arquivo ON cadastros_a_classificar(file_id);

-- Aprovação explícita de levar um cadastro à Biblioteca Homologada. O DAG só registra; quem
-- escreve (pela interface oficial do Calibre) é outra camada, com este registro de antes/depois
-- e o `rollback_id` para desfazer.
CREATE TABLE homologacoes (
    id                    INTEGER PRIMARY KEY,
    cadastro_id           INTEGER NOT NULL REFERENCES cadastros_a_classificar(id) ON DELETE CASCADE,
    biblioteca_destino_id INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    aprovado_por          TEXT,
    aprovado_em           INTEGER,
    autor                 TEXT    CHECK (autor IS NULL OR autor IN ('gui','cli','mcp')),
    antes_json            TEXT,
    depois_json           TEXT,
    calibre_id            INTEGER,
    rollback_id           TEXT,
    estado                TEXT    NOT NULL DEFAULT 'proposta'
        CHECK (estado IN ('proposta','aprovada','aplicada','falhou','revertida')),
    erro                  TEXT,
    criado_em             INTEGER NOT NULL,
    atualizado_em         INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_homologacoes_cadastro ON homologacoes(cadastro_id, id DESC);
CREATE INDEX idx_homologacoes_estado ON homologacoes(estado, id);

-- Mínimo: a verificação e a Cobertura vêm da Fase A (cópia com Marcação de Backup em outro
-- Dispositivo Físico); aqui só o manifesto e o registro da verificação/restauração amostral.
CREATE TABLE backups_biblioteca (
    id                 INTEGER PRIMARY KEY,
    biblioteca_id      INTEGER NOT NULL REFERENCES bibliotecas_calibre(id) ON DELETE CASCADE,
    manifesto_json     TEXT    NOT NULL,
    criado_em          INTEGER NOT NULL,
    verificacao_estado TEXT    NOT NULL DEFAULT 'pendente'
        CHECK (verificacao_estado IN ('pendente','ok','falhou')),
    verificado_em      INTEGER,
    restauracao_json   TEXT,
    restauracao_em     INTEGER,
    cobertura_json     TEXT
) STRICT;
CREATE INDEX idx_backups_biblioteca ON backups_biblioteca(biblioteca_id, criado_em DESC);

-- ---------------------------------------------------------------------------------------------
-- Contadores mantidos (tabela `contadores`, migração 0007). Os gatilhos também valem para
-- ON DELETE CASCADE vindo de `files`.
-- ---------------------------------------------------------------------------------------------

-- biblio_jobs:<estado>
CREATE TRIGGER trg_biblio_jobs_ins AFTER INSERT ON jobs_bibliograficos
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_jobs:' || NEW.estado, 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_jobs_del AFTER DELETE ON jobs_bibliograficos
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_jobs:' || OLD.estado;
END;
CREATE TRIGGER trg_biblio_jobs_upd AFTER UPDATE OF estado ON jobs_bibliograficos
WHEN OLD.estado <> NEW.estado
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_jobs:' || OLD.estado;
    INSERT INTO contadores (chave, valor) VALUES ('biblio_jobs:' || NEW.estado, 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;

-- biblio_livros:<biblioteca_id> e biblio_formatos:<biblioteca_id>
CREATE TRIGGER trg_biblio_livros_ins AFTER INSERT ON livros_homologados
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_livros:' || NEW.biblioteca_id, 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_livros_del AFTER DELETE ON livros_homologados
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_livros:' || OLD.biblioteca_id;
END;
CREATE TRIGGER trg_biblio_formatos_ins AFTER INSERT ON formatos_homologados
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_formatos:' || NEW.biblioteca_id, 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_formatos_del AFTER DELETE ON formatos_homologados
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_formatos:' || OLD.biblioteca_id;
END;

-- biblio_sugestoes_pendentes
CREATE TRIGGER trg_biblio_sugestoes_ins AFTER INSERT ON sugestoes WHEN NEW.estado = 'pendente'
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_sugestoes_pendentes', 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_sugestoes_del AFTER DELETE ON sugestoes WHEN OLD.estado = 'pendente'
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_sugestoes_pendentes';
END;
CREATE TRIGGER trg_biblio_sugestoes_upd AFTER UPDATE OF estado ON sugestoes
WHEN (OLD.estado = 'pendente') <> (NEW.estado = 'pendente')
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_sugestoes_pendentes',
        CASE WHEN NEW.estado = 'pendente' THEN 1 ELSE 0 END)
        ON CONFLICT(chave) DO UPDATE SET
            valor = valor + CASE WHEN NEW.estado = 'pendente' THEN 1 ELSE -1 END;
END;

-- biblio_candidatos_abertos
CREATE TRIGGER trg_biblio_cand_ins AFTER INSERT ON candidatos_duplicidade WHEN NEW.estado = 'aberto'
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_candidatos_abertos', 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_cand_del AFTER DELETE ON candidatos_duplicidade WHEN OLD.estado = 'aberto'
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_candidatos_abertos';
END;
CREATE TRIGGER trg_biblio_cand_upd AFTER UPDATE OF estado ON candidatos_duplicidade
WHEN (OLD.estado = 'aberto') <> (NEW.estado = 'aberto')
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_candidatos_abertos',
        CASE WHEN NEW.estado = 'aberto' THEN 1 ELSE 0 END)
        ON CONFLICT(chave) DO UPDATE SET
            valor = valor + CASE WHEN NEW.estado = 'aberto' THEN 1 ELSE -1 END;
END;

-- biblio_alertas_nao_vistos (não visto e não silenciado)
CREATE TRIGGER trg_biblio_alertas_ins AFTER INSERT ON alertas_biblio
WHEN NEW.visto_em IS NULL AND NEW.silenciado = 0
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_alertas_nao_vistos', 1)
        ON CONFLICT(chave) DO UPDATE SET valor = valor + 1;
END;
CREATE TRIGGER trg_biblio_alertas_del AFTER DELETE ON alertas_biblio
WHEN OLD.visto_em IS NULL AND OLD.silenciado = 0
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'biblio_alertas_nao_vistos';
END;
CREATE TRIGGER trg_biblio_alertas_upd AFTER UPDATE OF visto_em, silenciado ON alertas_biblio
WHEN (OLD.visto_em IS NULL AND OLD.silenciado = 0) <> (NEW.visto_em IS NULL AND NEW.silenciado = 0)
BEGIN
    INSERT INTO contadores (chave, valor) VALUES ('biblio_alertas_nao_vistos',
        CASE WHEN NEW.visto_em IS NULL AND NEW.silenciado = 0 THEN 1 ELSE 0 END)
        ON CONFLICT(chave) DO UPDATE SET
            valor = valor + CASE WHEN NEW.visto_em IS NULL AND NEW.silenciado = 0 THEN 1 ELSE -1 END;
END;
