-- Contêineres (ADR 0006), migração aditiva: conteúdo de compactados em tabela própria, fila
-- persistente da etapa "Conteúdo de contêineres", senhas cifradas, Cofre de Senhas, Fila de
-- Serviço e Membros nos Grupos de Duplicados.
--
-- Nada aqui altera as tabelas de inventário: Membros não entram em `files`, `folders` nem em
-- `folder_stats` (a árvore e o tamanho das pastas do Volume continuam só com arquivos reais).

-- Um Contêiner por arquivo candidato. Esta tabela é também a fila da etapa (estado `pendente`
-- = na fila; `lendo` volta para `pendente` na abertura): persistente, pausável, retomável.
-- `hash_lido` é o Hash Completo do arquivo na leitura que gerou os Membros: o Contêiner só é
-- relido se o hash do arquivo mudar, e uma cópia idêntica reaproveita os Membros de quem já foi
-- lido.
CREATE TABLE conteineres (
    file_id       INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    volume_id     INTEGER NOT NULL,
    estado        TEXT    NOT NULL CHECK (estado IN
        ('pendente','lendo','lido','aguardando_senha','aguardando_chaveiro','falha','nao_conteiner')),
    hash_lido     BLOB    CHECK (hash_lido IS NULL OR length(hash_lido) = 32),
    formato       TEXT,
    membros       INTEGER NOT NULL DEFAULT 0,
    bytes_total   INTEGER NOT NULL DEFAULT 0,
    bytes_lidos   INTEGER NOT NULL DEFAULT 0,
    com_senha     INTEGER NOT NULL DEFAULT 0 CHECK (com_senha IN (0,1)),
    motivo        TEXT,
    enfileirado_em INTEGER NOT NULL,
    atualizado_em INTEGER NOT NULL,
    -- Cursor de retomada (só leitores retomáveis, as Imagens de Disco): os primeiros
    -- `retomada_indice` Membros do nível raiz já estão em `membros`, gravados na mesma
    -- transação deste cursor. `retomada_hash` é o Hash Completo do arquivo na leitura que o
    -- gerou; com outro hash o cursor vale nada e a leitura recomeça do zero.
    retomada_indice INTEGER,
    retomada_ordem  INTEGER,
    retomada_bytes  INTEGER,
    retomada_hash   BLOB CHECK (retomada_hash IS NULL OR length(retomada_hash) = 32)
) STRICT;
CREATE INDEX idx_conteineres_estado ON conteineres(estado, volume_id);
CREATE INDEX idx_conteineres_hash ON conteineres(hash_lido) WHERE estado = 'lido';

-- Membros. `pai_ordem` aponta o Membro pai (Contêiner aninhado) dentro do mesmo Contêiner, pela
-- `ordem` (e não pelo id), para que copiar a lista de Membros para uma cópia idêntica seja um
-- INSERT ... SELECT sem remapear ids.
CREATE TABLE membros (
    id                INTEGER PRIMARY KEY,
    conteiner_file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    ordem             INTEGER NOT NULL,
    pai_ordem         INTEGER,
    caminho_raw       BLOB    NOT NULL,
    caminho_display   TEXT    NOT NULL,
    tipo              TEXT    NOT NULL CHECK (tipo IN ('arquivo','pasta','link','outro')),
    tamanho           INTEGER,
    mtime_ns          INTEGER,
    cifrado           INTEGER NOT NULL DEFAULT 0 CHECK (cifrado IN (0,1)),
    hash              BLOB    CHECK (hash IS NULL OR length(hash) = 32),
    estado            TEXT    NOT NULL CHECK (estado IN
        ('lido','cifrado','nao_suportado','aninhado_grande','erro')),
    motivo            TEXT,
    formato_interno   TEXT,
    UNIQUE (conteiner_file_id, ordem)
) STRICT;
CREATE INDEX idx_membros_hash ON membros(tamanho, hash) WHERE hash IS NOT NULL;
CREATE INDEX idx_membros_pai ON membros(conteiner_file_id, pai_ordem, caminho_raw);

-- Senha do Contêiner: cifrada (XChaCha20-Poly1305, nonce + texto cifrado) e ligada ao Hash
-- Completo do Contêiner. A chave de cifra fica no keyring do sistema, nunca no Catálogo.
CREATE TABLE senhas_conteiner (
    hash      BLOB PRIMARY KEY CHECK (length(hash) = 32),
    cifrada   BLOB    NOT NULL,
    criada_em INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

-- Cofre de Senhas: as senhas que o usuário mais usa, também cifradas.
CREATE TABLE cofre_senhas (
    id        INTEGER PRIMARY KEY,
    rotulo    TEXT    NOT NULL,
    cifrada   BLOB    NOT NULL,
    criada_em INTEGER NOT NULL
) STRICT;

-- Fila de Serviço: pendências que só o usuário resolve.
CREATE TABLE pedidos_servico (
    id          INTEGER PRIMARY KEY,
    tipo        TEXT    NOT NULL CHECK (tipo IN ('pedido_senha','falha_leitura')),
    file_id     INTEGER REFERENCES files(id) ON DELETE CASCADE,
    motivo      TEXT    NOT NULL,
    estado      TEXT    NOT NULL DEFAULT 'aberto' CHECK (estado IN ('aberto','resolvido','dispensado')),
    criado_em   INTEGER NOT NULL,
    resolvido_em INTEGER
) STRICT;
-- No máximo um pedido aberto de cada tipo por objeto.
CREATE UNIQUE INDEX ux_pedidos_abertos ON pedidos_servico(tipo, file_id) WHERE estado = 'aberto';
CREATE INDEX idx_pedidos_estado ON pedidos_servico(estado, id);

-- Membros que entram em Grupos de Duplicados (o grupo é o mesmo de `grupo_membros`; um Membro
-- conta como cópia mas nunca como espaço recuperável).
CREATE TABLE grupo_membros_conteiner (
    membro_id INTEGER PRIMARY KEY REFERENCES membros(id) ON DELETE CASCADE,
    grupo_id  INTEGER NOT NULL REFERENCES grupos_duplicados(id) ON DELETE CASCADE
) STRICT;
CREATE INDEX idx_gmc_grupo ON grupo_membros_conteiner(grupo_id);

-- Membros novos ou removidos desatualizam os Grupos de Duplicados, como os hashes.
CREATE TRIGGER trg_membros_mudancas_ins AFTER INSERT ON membros WHEN NEW.hash IS NOT NULL
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes_mudancas';
END;
CREATE TRIGGER trg_membros_mudancas_del AFTER DELETE ON membros WHEN OLD.hash IS NOT NULL
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes_mudancas';
END;
