-- Retomada automática e Ponto de Confiança (ADR 0003). Só acrescenta: nada existente é alterado.
--
-- `parada_deliberada`: a etapa de hash foi cancelada de propósito (não é queda): não retoma sozinha.
-- `retomada_automatica`: flag "continuar de onde parou": gravada ao detectar a Execução
-- Interrompida com a retomada automática ligada; zerada quando a execução termina ou é parada.
ALTER TABLE scan_runs ADD COLUMN parada_deliberada INTEGER NOT NULL DEFAULT 0
    CHECK (parada_deliberada IN (0, 1));
ALTER TABLE scan_runs ADD COLUMN retomada_automatica INTEGER NOT NULL DEFAULT 0
    CHECK (retomada_automatica IN (0, 1));

-- Ponto de Confiança por Raiz: `confiavel_em` não nulo = só o monitoramento a mantém em dia.
-- Lacuna do monitor ou encerramento não limpo grava `perdido_em` e o motivo e zera `confiavel_em`.
CREATE TABLE ponto_confianca (
    volume_id     INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
    path_raw      BLOB    NOT NULL,
    path_display  TEXT    NOT NULL,
    confiavel_em  INTEGER,
    perdido_em    INTEGER,
    motivo_perda  TEXT,
    PRIMARY KEY (volume_id, path_raw)
) STRICT;
