-- Migração 1: esquema inicial do Catálogo (Fase A, rev. 2). Ver ESQUEMA.md.
-- Convenções: tempos em INTEGER (segundos Unix para eventos; nanossegundos Unix para
-- mtime/ctime/birthtime de arquivos); caminhos brutos em BLOB (*_raw) mais TEXT de exibição
-- (*_display); NULL significa "indisponível", nunca zero inventado.

-- ---------------------------------------------------------------- hardware e volumes
CREATE TABLE dispositivos (
    id            INTEGER PRIMARY KEY,
    identificador TEXT    NOT NULL UNIQUE,   -- serial/WWN/UUID do disco físico
    rotulo        TEXT,
    modelo        TEXT,
    tipo          TEXT    NOT NULL DEFAULT 'desconhecido'
                  CHECK (tipo IN ('desconhecido','hdd','ssd','nvme','usb','rede','virtual','outro')),
    criado_em     INTEGER NOT NULL,
    visto_em      INTEGER
) STRICT;

CREATE TABLE volumes (
    id             INTEGER PRIMARY KEY,
    platform_id    TEXT    NOT NULL UNIQUE,  -- UUID do filesystem
    device_id      INTEGER REFERENCES dispositivos(id) ON DELETE SET NULL,
    rotulo         TEXT,
    fs_type        TEXT,
    capacity_bytes INTEGER CHECK (capacity_bytes IS NULL OR capacity_bytes >= 0),
    free_bytes     INTEGER CHECK (free_bytes IS NULL OR free_bytes >= 0),
    mounted        INTEGER NOT NULL DEFAULT 0 CHECK (mounted IN (0,1)),
    last_seen_at   INTEGER,
    flags          TEXT    NOT NULL DEFAULT '{}'
) STRICT;
CREATE INDEX idx_volumes_device ON volumes(device_id);

CREATE TABLE mount_points (
    id            INTEGER PRIMARY KEY,
    volume_id     INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
    path_raw      BLOB    NOT NULL,
    path_display  TEXT    NOT NULL,
    mounted_at    INTEGER,
    scope_policy  TEXT    NOT NULL DEFAULT 'completo'
                  CHECK (scope_policy IN ('completo','restrito','ignorado')),
    UNIQUE (volume_id, path_raw)
) STRICT;

-- ---------------------------------------------------------------- varreduras
CREATE TABLE scan_runs (
    id             INTEGER PRIMARY KEY,
    mode           TEXT    NOT NULL CHECK (mode IN ('full','fast','verificacao','monitor')),
    roots          TEXT    NOT NULL DEFAULT '[]',   -- JSON
    started_at     INTEGER NOT NULL,
    ended_at       INTEGER,
    status         TEXT    NOT NULL DEFAULT 'em_andamento'
                   CHECK (status IN ('em_andamento','pausada','concluida','concluida_com_erros','cancelada','falhou','interrompida')),
    phase          TEXT    NOT NULL DEFAULT 'inventario'
                   CHECK (phase IN ('inventario','estatisticas','hash','verificacao','concluida')),
    counters       TEXT    NOT NULL DEFAULT '{}',   -- JSON
    checkpoint     TEXT,
    config_version INTEGER NOT NULL DEFAULT 1,
    CHECK (ended_at IS NULL OR ended_at >= started_at)
) STRICT;

-- ---------------------------------------------------------------- classificação
CREATE TABLE file_types (
    id                INTEGER PRIMARY KEY,
    extension         TEXT,
    mime              TEXT,
    human_description TEXT,
    broad_category    TEXT,
    evidence_source   TEXT NOT NULL DEFAULT 'extensao'
                      CHECK (evidence_source IN ('extensao','magic','mime','usuario')),
    confidence        REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
    UNIQUE (extension, mime)
) STRICT;

CREATE TABLE classifications (
    id            INTEGER PRIMARY KEY,
    category      TEXT    NOT NULL,
    scope         TEXT    NOT NULL DEFAULT 'global',
    rule_id       TEXT,
    confidence    REAL CHECK (confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
    classified_at INTEGER NOT NULL
) STRICT;

-- ---------------------------------------------------------------- pastas e arquivos (tabelas separadas, CA-A03)
CREATE TABLE folders (
    id                        INTEGER PRIMARY KEY,
    volume_id                 INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
    parent_folder_id          INTEGER REFERENCES folders(id) ON DELETE CASCADE,  -- NULL = raiz do volume
    path_raw                  BLOB    NOT NULL,
    path_display              TEXT    NOT NULL,
    name_raw                  BLOB    NOT NULL,
    name_display              TEXT    NOT NULL,
    native_dev                INTEGER,
    native_file_id            INTEGER,           -- inode
    class_id                  INTEGER REFERENCES classifications(id) ON DELETE SET NULL,
    classification_confidence REAL CHECK (classification_confidence IS NULL OR (classification_confidence >= 0 AND classification_confidence <= 1)),
    attrs                     TEXT    NOT NULL DEFAULT '{}',
    mtime_ns                  INTEGER,
    seen_run_id               INTEGER REFERENCES scan_runs(id) ON DELETE SET NULL,
    state                     TEXT    NOT NULL DEFAULT 'ativo'
                              CHECK (state IN ('ativo','ausente','removido','indisponivel')),
    UNIQUE (volume_id, path_raw),
    CHECK (parent_folder_id IS NULL OR parent_folder_id <> id)
) STRICT;
CREATE INDEX idx_folders_parent ON folders(parent_folder_id);
CREATE INDEX idx_folders_seen_run ON folders(seen_run_id);
CREATE INDEX idx_folders_state ON folders(volume_id, state);

CREATE TABLE files (
    id               INTEGER PRIMARY KEY,
    volume_id        INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
    folder_id        INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    path_raw         BLOB    NOT NULL,
    path_display     TEXT    NOT NULL,
    name_raw         BLOB    NOT NULL,
    name_display     TEXT    NOT NULL,
    native_dev       INTEGER,
    native_file_id   INTEGER,                     -- inode; pode ser reutilizado, não é identidade única
    hardlink_group   INTEGER,                     -- NULL = nlink 1 / desconhecido
    nlink            INTEGER CHECK (nlink IS NULL OR nlink >= 1),
    logical_bytes    INTEGER NOT NULL CHECK (logical_bytes >= 0),
    allocated_bytes  INTEGER CHECK (allocated_bytes IS NULL OR allocated_bytes >= 0),  -- NULL = indisponível
    mtime_ns         INTEGER,
    ctime_ns         INTEGER,
    birthtime_ns     INTEGER,                     -- NULL = indisponível
    attrs            TEXT    NOT NULL DEFAULT '{}',
    mime_id          INTEGER REFERENCES file_types(id) ON DELETE SET NULL,
    class_id         INTEGER REFERENCES classifications(id) ON DELETE SET NULL,
    seen_run_id      INTEGER REFERENCES scan_runs(id) ON DELETE SET NULL,
    state            TEXT    NOT NULL DEFAULT 'ativo'
                     CHECK (state IN ('ativo','ausente','removido','indisponivel')),
    UNIQUE (volume_id, path_raw)
) STRICT;
CREATE INDEX idx_files_folder ON files(folder_id);
CREATE INDEX idx_files_seen_run ON files(seen_run_id);
CREATE INDEX idx_files_state ON files(volume_id, state);
CREATE INDEX idx_files_physical ON files(volume_id, native_dev, native_file_id);
CREATE INDEX idx_files_size ON files(logical_bytes);

CREATE TABLE folder_stats (
    folder_id              INTEGER PRIMARY KEY REFERENCES folders(id) ON DELETE CASCADE,
    child_count            INTEGER NOT NULL DEFAULT 0 CHECK (child_count >= 0),
    recursive_file_count   INTEGER NOT NULL DEFAULT 0 CHECK (recursive_file_count >= 0),
    recursive_folder_count INTEGER NOT NULL DEFAULT 0 CHECK (recursive_folder_count >= 0),
    logical_bytes          INTEGER NOT NULL DEFAULT 0 CHECK (logical_bytes >= 0),
    allocated_entry_bytes  INTEGER CHECK (allocated_entry_bytes IS NULL OR allocated_entry_bytes >= 0),
    physical_unique_bytes  INTEGER CHECK (physical_unique_bytes IS NULL OR physical_unique_bytes >= 0),
    aggregate_state        TEXT    NOT NULL DEFAULT 'pendente'
                           CHECK (aggregate_state IN ('pendente','parcial','completo','desatualizado')),
    updated_at             INTEGER NOT NULL
) STRICT;

-- ---------------------------------------------------------------- fila, eventos, erros, exclusões
CREATE TABLE scan_queue (
    id            INTEGER PRIMARY KEY,
    run_id        INTEGER NOT NULL REFERENCES scan_runs(id) ON DELETE CASCADE,
    directory_id  INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    path_raw      BLOB    NOT NULL,
    phase         TEXT    NOT NULL DEFAULT 'inventario'
                  CHECK (phase IN ('inventario','estatisticas','hash','verificacao')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    lease_state   TEXT    NOT NULL DEFAULT 'livre'
                  CHECK (lease_state IN ('livre','reservada','concluida','falhou')),
    last_error    TEXT,
    scheduled_at  INTEGER NOT NULL,
    UNIQUE (run_id, phase, path_raw)
) STRICT;
CREATE INDEX idx_scan_queue_estado ON scan_queue(run_id, lease_state);

CREATE TABLE scan_events (
    id                 INTEGER PRIMARY KEY,
    volume_id          INTEGER REFERENCES volumes(id) ON DELETE CASCADE,
    event_type         TEXT    NOT NULL
                       CHECK (event_type IN ('criado','alterado','removido','renomeado','overflow','montado','desmontado')),
    source             TEXT    NOT NULL DEFAULT 'inotify',
    path_raw           BLOB,
    file_id            INTEGER REFERENCES files(id) ON DELETE SET NULL,
    observed_at        INTEGER NOT NULL,
    processed_at       INTEGER,
    reconcile_required INTEGER NOT NULL DEFAULT 0 CHECK (reconcile_required IN (0,1))
) STRICT;
CREATE INDEX idx_scan_events_pendentes ON scan_events(processed_at) WHERE processed_at IS NULL;

CREATE TABLE scan_errors (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER REFERENCES scan_runs(id) ON DELETE CASCADE,
    path_raw    BLOB,
    path_display TEXT,
    phase       TEXT    NOT NULL,
    error_code  TEXT    NOT NULL,
    reason      TEXT,
    observed_at INTEGER NOT NULL,
    severity    TEXT    NOT NULL DEFAULT 'aviso'
                CHECK (severity IN ('info','aviso','erro','impeditivo'))
) STRICT;
CREATE INDEX idx_scan_errors_run ON scan_errors(run_id);

CREATE TABLE scan_exclusions (
    id      INTEGER PRIMARY KEY,
    scope   TEXT    NOT NULL DEFAULT 'global',
    matcher TEXT    NOT NULL,
    reason  TEXT,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1))
) STRICT;

CREATE TABLE service_state (
    id             INTEGER PRIMARY KEY CHECK (id = 1),   -- linha única
    desired_mode   TEXT    NOT NULL DEFAULT 'parado'
                   CHECK (desired_mode IN ('parado','sob_demanda','continuo')),
    current_mode   TEXT    NOT NULL DEFAULT 'parado'
                   CHECK (current_mode IN ('parado','sob_demanda','continuo')),
    last_heartbeat INTEGER,
    last_reconcile INTEGER,
    error          TEXT
) STRICT;
INSERT INTO service_state (id) VALUES (1);

CREATE TABLE catalog_snapshots (
    id               INTEGER PRIMARY KEY,
    created_at       INTEGER NOT NULL,
    backup_path      TEXT    NOT NULL,
    integrity_result TEXT    NOT NULL,
    scope            TEXT    NOT NULL DEFAULT 'completo'
) STRICT;

-- ---------------------------------------------------------------- hash completo (A-23, A-24, A-33)
-- Ausência de linha = arquivo ainda sem Hash Completo (nunca hash zerado).
CREATE TABLE hashes (
    file_id        INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    algoritmo      TEXT    NOT NULL DEFAULT 'blake3' CHECK (algoritmo = 'blake3'),
    hash           BLOB    NOT NULL CHECK (length(hash) = 32),
    hashed_at      INTEGER NOT NULL,
    -- Evidências do arquivo no momento do hash: reuso só se todas continuarem iguais.
    size_bytes     INTEGER NOT NULL CHECK (size_bytes >= 0),
    mtime_ns       INTEGER,
    ctime_ns       INTEGER,
    native_file_id INTEGER,
    -- Verificação de Integridade (A-33).
    verified_at        INTEGER,
    verification_state TEXT CHECK (verification_state IS NULL OR verification_state IN ('ok','divergente')),
    CHECK ((verified_at IS NULL) = (verification_state IS NULL))
) STRICT;
CREATE INDEX idx_hashes_dup ON hashes(size_bytes, hash);

CREATE TABLE fila_hash (
    id            INTEGER PRIMARY KEY,
    file_id       INTEGER NOT NULL UNIQUE REFERENCES files(id) ON DELETE CASCADE,
    tipo          TEXT    NOT NULL DEFAULT 'hash' CHECK (tipo IN ('hash','verificacao')),
    state         TEXT    NOT NULL DEFAULT 'pendente'
                  CHECK (state IN ('pendente','em_andamento','pausado','concluido','erro')),
    priority      INTEGER NOT NULL DEFAULT 0,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    bytes_lidos   INTEGER NOT NULL DEFAULT 0 CHECK (bytes_lidos >= 0),
    last_error    TEXT,
    enqueued_at   INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_fila_hash_state ON fila_hash(state, priority DESC);

-- ---------------------------------------------------------------- duplicados e Cópia Principal (A-25, A-26)
CREATE TABLE grupos_duplicados (
    id                      INTEGER PRIMARY KEY,
    logical_bytes           INTEGER NOT NULL CHECK (logical_bytes > 0),   -- arquivos vazios ficam fora
    hash                    BLOB    NOT NULL CHECK (length(hash) = 32),
    copia_principal_file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
    copia_principal_modo    TEXT    NOT NULL DEFAULT 'auto' CHECK (copia_principal_modo IN ('auto','manual')),
    atualizado_em           INTEGER NOT NULL,
    UNIQUE (logical_bytes, hash),
    CHECK (copia_principal_modo = 'auto' OR copia_principal_file_id IS NOT NULL)
) STRICT;

CREATE TABLE grupo_membros (
    file_id  INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,  -- um arquivo, um grupo
    grupo_id INTEGER NOT NULL REFERENCES grupos_duplicados(id) ON DELETE CASCADE
) STRICT;
CREATE INDEX idx_grupo_membros_grupo ON grupo_membros(grupo_id);

-- Prioridade de Volume/pasta: menor ordem = maior prioridade. folder_id NULL = volume inteiro.
CREATE TABLE prioridades (
    id        INTEGER PRIMARY KEY,
    volume_id INTEGER NOT NULL REFERENCES volumes(id) ON DELETE CASCADE,
    folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    ordem     INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX ux_prioridades_volume ON prioridades(volume_id) WHERE folder_id IS NULL;
CREATE UNIQUE INDEX ux_prioridades_pasta ON prioridades(folder_id) WHERE folder_id IS NOT NULL;

-- ---------------------------------------------------------------- marcações (A-27..A-30)
CREATE TABLE marcacoes (
    id                INTEGER PRIMARY KEY,
    tipo              TEXT    NOT NULL CHECK (tipo IN ('backup','delecao')),
    escopo            TEXT    NOT NULL
                      CHECK (escopo IN ('regra_volume','regra_pasta','manual_arquivo','manual_grupo')),
    volume_id         INTEGER REFERENCES volumes(id) ON DELETE CASCADE,
    folder_id         INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    file_id           INTEGER REFERENCES files(id) ON DELETE CASCADE,
    grupo_id          INTEGER REFERENCES grupos_duplicados(id) ON DELETE CASCADE,
    estado            TEXT    NOT NULL DEFAULT 'ativa' CHECK (estado IN ('ativa','pendente')),
    hash_na_marcacao  BLOB CHECK (hash_na_marcacao IS NULL OR length(hash_na_marcacao) = 32),
    autor             TEXT    NOT NULL CHECK (autor IN ('gui','cli','mcp')),
    criada_em         INTEGER NOT NULL,
    atualizada_em     INTEGER NOT NULL,
    CHECK (
        (escopo = 'regra_volume'   AND volume_id IS NOT NULL AND folder_id IS NULL     AND file_id IS NULL     AND grupo_id IS NULL) OR
        (escopo = 'regra_pasta'    AND folder_id IS NOT NULL AND file_id IS NULL       AND grupo_id IS NULL) OR
        (escopo = 'manual_arquivo' AND file_id IS NOT NULL   AND volume_id IS NULL     AND folder_id IS NULL   AND grupo_id IS NULL) OR
        (escopo = 'manual_grupo'   AND file_id IS NOT NULL   AND grupo_id IS NOT NULL  AND volume_id IS NULL   AND folder_id IS NULL)
    ),
    -- Marcação manual guarda o hash do momento; ela vira pendente se o hash do arquivo mudar.
    CHECK (escopo IN ('regra_volume','regra_pasta') OR hash_na_marcacao IS NOT NULL)
) STRICT;
-- Uma cópia tem no máximo uma marcação manual; cada alvo de regra tem no máximo uma regra.
CREATE UNIQUE INDEX ux_marcacoes_arquivo ON marcacoes(file_id) WHERE file_id IS NOT NULL;
CREATE UNIQUE INDEX ux_marcacoes_regra_volume ON marcacoes(volume_id) WHERE escopo = 'regra_volume';
CREATE UNIQUE INDEX ux_marcacoes_regra_pasta ON marcacoes(folder_id) WHERE escopo = 'regra_pasta';
CREATE INDEX idx_marcacoes_estado ON marcacoes(tipo, estado);
CREATE INDEX idx_marcacoes_grupo ON marcacoes(grupo_id);

-- Histórico só de acréscimo. Sem FK de propósito: sobrevive à remoção da marcação e do arquivo.
CREATE TABLE marcacoes_historico (
    id              INTEGER PRIMARY KEY,
    marcacao_id     INTEGER,
    evento          TEXT    NOT NULL
                    CHECK (evento IN ('criada','confirmada','pendente','alterada','removida')),
    tipo            TEXT    NOT NULL CHECK (tipo IN ('backup','delecao')),
    escopo          TEXT    NOT NULL
                    CHECK (escopo IN ('regra_volume','regra_pasta','manual_arquivo','manual_grupo')),
    estado_anterior TEXT    CHECK (estado_anterior IS NULL OR estado_anterior IN ('ativa','pendente')),
    estado_novo     TEXT    CHECK (estado_novo IS NULL OR estado_novo IN ('ativa','pendente')),
    volume_id       INTEGER,
    folder_id       INTEGER,
    file_id         INTEGER,
    grupo_id        INTEGER,
    caminho_display TEXT,
    hash            BLOB CHECK (hash IS NULL OR length(hash) = 32),
    autor           TEXT    NOT NULL CHECK (autor IN ('gui','cli','mcp')),
    em              INTEGER NOT NULL,
    detalhe         TEXT
) STRICT;
CREATE INDEX idx_marcacoes_historico_marcacao ON marcacoes_historico(marcacao_id);
CREATE INDEX idx_marcacoes_historico_arquivo ON marcacoes_historico(file_id);

-- Triggers no mesmo formato de bancada_armazem::instalar_somente_acrescimo (esta_protegida os reconhece).
CREATE TRIGGER IF NOT EXISTS "marcacoes_historico_recusa_update" BEFORE UPDATE ON "marcacoes_historico"
BEGIN
    SELECT RAISE(ABORT, 'a tabela marcacoes_historico é somente de acréscimo: UPDATE recusado');
END;
CREATE TRIGGER IF NOT EXISTS "marcacoes_historico_recusa_delete" BEFORE DELETE ON "marcacoes_historico"
BEGIN
    SELECT RAISE(ABORT, 'a tabela marcacoes_historico é somente de acréscimo: DELETE recusado');
END;
