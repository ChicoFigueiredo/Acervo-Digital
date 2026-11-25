-- Migração 8: Lixeira em dois níveis (ADR 0005). Aditiva: só acrescenta colunas e uma tabela.
-- * `files.na_lixeira`: a cópia está na `DAG-Lixeira` do próprio Volume (segundo nível). A linha
--   passa a apontar o caminho dentro da Lixeira; `lixeira_origem_*` guarda de onde veio (para
--   restaurar) e `lixeira_movido_em` quando (segundos Unix).
-- * `descartados`: histórico do que estava Na Lixeira e sumiu (o usuário esvaziou a Lixeira).
--   Só de acréscimo na prática; sem FK, sobrevive à linha de `files`.

ALTER TABLE files ADD COLUMN na_lixeira INTEGER NOT NULL DEFAULT 0 CHECK (na_lixeira IN (0,1));
ALTER TABLE files ADD COLUMN lixeira_origem_raw BLOB;
ALTER TABLE files ADD COLUMN lixeira_origem_display TEXT;
ALTER TABLE files ADD COLUMN lixeira_movido_em INTEGER;
CREATE INDEX idx_files_na_lixeira ON files(volume_id, id) WHERE na_lixeira = 1;

CREATE TABLE descartados (
    id              INTEGER PRIMARY KEY,
    file_id         INTEGER,
    volume_id       INTEGER,
    caminho_display TEXT    NOT NULL,   -- onde estava na Lixeira quando sumiu
    origem_display  TEXT,               -- de onde veio
    hash            BLOB CHECK (hash IS NULL OR length(hash) = 32),
    bytes           INTEGER NOT NULL CHECK (bytes >= 0),
    mtime_ns        INTEGER,
    movido_em       INTEGER,
    descartado_em   INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_descartados_em ON descartados(descartado_em DESC, id DESC);
