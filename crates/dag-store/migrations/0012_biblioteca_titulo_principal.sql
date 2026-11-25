-- Curadoria Bibliográfica, 2ª passada (aditiva): título principal normalizado do espelho
-- (sem artigo inicial, subtítulo nem marcador de volume) para pré-filtrar candidatos de
-- identidade bibliográfica por índice, sem nunca comparar todos contra todos.
-- A coluna é preenchida pelo código de migração (retroativo) e a cada sincronização.
ALTER TABLE livros_homologados ADD COLUMN titulo_principal TEXT;
CREATE INDEX idx_livros_homologados_principal ON livros_homologados(titulo_principal)
    WHERE titulo_principal IS NOT NULL;
