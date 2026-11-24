-- Erros esperados e aceitos. Só acrescenta.
--  * scan_errors.classe: 'esperado' | 'aceito' | 'erro' (NULL em linhas antigas: o leitor classifica
--    na hora pelas regras); scan_errors.regra: quem decidiu (`embutida:<id>`, `usuario:<id>`).
--  * erros_aceitos: regras "Erros aceitos" do usuário (glob de caminho + código ou '*').
--  * scan_runs.status ganha 'concluida_com_avisos' (feito em código, em `lib.rs`).
--  * Exclusões do usuário reaproveitam `scan_exclusions` (matcher = glob).
ALTER TABLE scan_errors ADD COLUMN classe TEXT CHECK (classe IS NULL OR classe IN ('esperado','aceito','erro'));
ALTER TABLE scan_errors ADD COLUMN regra TEXT;
CREATE INDEX idx_scan_errors_classe ON scan_errors(run_id, classe);

CREATE TABLE erros_aceitos (
    id        INTEGER PRIMARY KEY,
    padrao    TEXT    NOT NULL,
    codigo    TEXT    NOT NULL DEFAULT '*',
    motivo    TEXT    NOT NULL DEFAULT '',
    criada_em INTEGER NOT NULL,
    UNIQUE (padrao, codigo)
) STRICT;
