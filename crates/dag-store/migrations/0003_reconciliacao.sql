-- Migração 3: Fast Scan, monitor de eventos e volumes offline (Fase A, incremento 5).
-- * `first_run_id`: execução que criou a linha (detecção de renomeação e de itens novos).
-- * `scan_events` ganha caminho anterior (renomeação), execução de origem e detalhe livre.
-- * `monitor_saude`: linha única com a saúde do monitor (visível a outros processos).

ALTER TABLE files ADD COLUMN first_run_id INTEGER;
ALTER TABLE folders ADD COLUMN first_run_id INTEGER;
CREATE INDEX idx_files_first_run ON files(first_run_id) WHERE first_run_id IS NOT NULL;
CREATE INDEX idx_folders_first_run ON folders(first_run_id) WHERE first_run_id IS NOT NULL;

ALTER TABLE scan_events ADD COLUMN old_path_raw BLOB;
ALTER TABLE scan_events ADD COLUMN run_id INTEGER;
ALTER TABLE scan_events ADD COLUMN detail TEXT;
CREATE INDEX idx_scan_events_observado ON scan_events(observed_at DESC, id DESC);
-- Lacunas (overflow, limite de watches, volume que sumiu) ainda sem correção.
CREATE INDEX idx_scan_events_lacunas ON scan_events(volume_id)
    WHERE reconcile_required = 1 AND processed_at IS NULL;

CREATE TABLE monitor_saude (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),   -- linha única
    run_id                INTEGER,
    estado                TEXT    NOT NULL DEFAULT 'parado'
                          CHECK (estado IN ('parado','iniciando','ativo','reconciliando','encerrando')),
    iniciado_em           INTEGER,
    heartbeat             INTEGER,
    backlog               INTEGER NOT NULL DEFAULT 0 CHECK (backlog >= 0),
    ultimo_evento_em      INTEGER,
    ultima_reconciliacao  INTEGER,
    watches               INTEGER NOT NULL DEFAULT 0 CHECK (watches >= 0),
    watches_limite        INTEGER NOT NULL DEFAULT 0 CHECK (watches_limite >= 0),
    overflows             INTEGER NOT NULL DEFAULT 0 CHECK (overflows >= 0),
    eventos_processados   INTEGER NOT NULL DEFAULT 0 CHECK (eventos_processados >= 0),
    raizes_parciais       INTEGER NOT NULL DEFAULT 0 CHECK (raizes_parciais >= 0),
    raizes                TEXT    NOT NULL DEFAULT '[]',
    erro                  TEXT
) STRICT;
INSERT INTO monitor_saude (id) VALUES (1);

-- Volume desconectado nunca apaga nem reescreve linhas (A-18): a indisponibilidade é derivada de
-- `volumes.mounted`. Estas visões expõem o estado efetivo ('indisponivel' quando o volume está
-- desconectado e a linha está 'ativo'); reconectar o volume restaura sem tocar nas linhas.
CREATE VIEW v_arquivos AS
    SELECT f.*, v.mounted AS volume_conectado,
           CASE WHEN f.state = 'ativo' AND v.mounted = 0 THEN 'indisponivel' ELSE f.state END
               AS estado_efetivo
    FROM files f JOIN volumes v ON v.id = f.volume_id;
CREATE VIEW v_pastas AS
    SELECT f.*, v.mounted AS volume_conectado,
           CASE WHEN f.state = 'ativo' AND v.mounted = 0 THEN 'indisponivel' ELSE f.state END
               AS estado_efetivo
    FROM folders f JOIN volumes v ON v.id = f.volume_id;
