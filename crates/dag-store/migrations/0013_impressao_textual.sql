-- Curadoria Bibliográfica (B-14), aditiva: impressão digital textual (MinHash de shingles) por
-- item (arquivo + Membro). `assinatura` é o BLOB de `dag_biblio::impressao`; `impressoes_bandas`
-- guarda uma chave LSH por banda para pré-filtrar candidatos por índice (nunca comparação
-- quadrática). É só evidência de hipótese de mesma Edição: nunca autoriza descarte.
CREATE TABLE impressoes_texto (
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    membro     TEXT    NOT NULL DEFAULT '',
    assinatura BLOB    NOT NULL,
    shingles   INTEGER NOT NULL,
    criado_em  INTEGER NOT NULL,
    PRIMARY KEY (file_id, membro)
) STRICT;

CREATE TABLE impressoes_bandas (
    chave   INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    membro  TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (chave, file_id, membro),
    FOREIGN KEY (file_id, membro) REFERENCES impressoes_texto(file_id, membro) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_impressoes_bandas_arquivo ON impressoes_bandas(file_id, membro);
