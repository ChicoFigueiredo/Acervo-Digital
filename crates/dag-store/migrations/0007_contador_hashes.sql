-- Contador mantido do total de arquivos com hash (`dag status` e a barra da GUI).
-- `SELECT COUNT(*) FROM hashes` varre o índice inteiro (dezenas de milhões de linhas); aqui a
-- contagem é uma leitura de uma linha. Os gatilhos mantêm o valor em qualquer caminho de escrita
-- (inclusive ON DELETE CASCADE de `files`). `INSERT OR REPLACE` NÃO dispara o gatilho de DELETE
-- (recursive_triggers desligado): a gravação do hash usa upsert (ON CONFLICT DO UPDATE).
-- A única varredura completa é a inicialização, uma vez, nesta migração.
CREATE TABLE contadores (
    chave TEXT PRIMARY KEY,
    valor INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

INSERT INTO contadores (chave, valor) VALUES ('hashes', (SELECT COUNT(*) FROM hashes));

CREATE TRIGGER trg_hashes_contador_ins AFTER INSERT ON hashes
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes';
END;

CREATE TRIGGER trg_hashes_contador_del AFTER DELETE ON hashes
BEGIN
    UPDATE contadores SET valor = valor - 1 WHERE chave = 'hashes';
END;

-- Versão dos hashes para os Grupos de Duplicados: `hashes_mudancas` sobe a cada inserção,
-- remoção ou troca de hash; `grupos_em` guarda o valor visto pela última reconstrução completa
-- dos grupos (-1 = nunca construídos). Se diferem, os grupos estão desatualizados e a
-- reconstrução é agendada (abertura do agente, fim da etapa de hash, `dag duplicados reconstruir`).
-- Catálogo que já tem grupos não é tratado como desatualizado pela migração.
INSERT INTO contadores (chave, valor) VALUES ('hashes_mudancas', (SELECT COUNT(*) FROM hashes));
INSERT INTO contadores (chave, valor) VALUES ('grupos_em',
    CASE WHEN EXISTS (SELECT 1 FROM grupos_duplicados) THEN (SELECT COUNT(*) FROM hashes) ELSE -1 END);

CREATE TRIGGER trg_hashes_mudancas_ins AFTER INSERT ON hashes
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes_mudancas';
END;
CREATE TRIGGER trg_hashes_mudancas_del AFTER DELETE ON hashes
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes_mudancas';
END;
CREATE TRIGGER trg_hashes_mudancas_upd AFTER UPDATE OF hash, size_bytes ON hashes
BEGIN
    UPDATE contadores SET valor = valor + 1 WHERE chave = 'hashes_mudancas';
END;
