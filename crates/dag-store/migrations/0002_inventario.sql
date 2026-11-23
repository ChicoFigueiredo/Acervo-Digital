-- Migração 2: inventário do Full Scan (Fase A, incremento 2).
-- Acrescenta: tipo de entrada em `files` (symlinks e especiais ficam em `files`, nunca em
-- `folders`: pasta é só diretório real, e links nunca são seguidos), alvo bruto do symlink,
-- contagem de entradas especiais nos agregados, pasta de origem dos erros (para marcar
-- agregados parciais), totais por volume e índices de paginação/alias.

-- Tipo da entrada. 'arquivo' = arquivo regular. Symlink guarda o alvo exatamente como o
-- readlink devolveu (bytes), sem resolver nem abrir.
ALTER TABLE files ADD COLUMN kind TEXT NOT NULL DEFAULT 'arquivo'
    CHECK (kind IN ('arquivo','symlink','fifo','socket','dispositivo_bloco','dispositivo_char','outro'));
ALTER TABLE files ADD COLUMN link_target_raw BLOB;
ALTER TABLE files ADD COLUMN link_target_display TEXT;

-- Paginação por tamanho dentro de uma pasta (keyset).
CREATE INDEX idx_files_folder_size ON files(folder_id, logical_bytes DESC, id);
-- Grupos de hard link (A-06).
CREATE INDEX idx_files_hardlink ON files(volume_id, hardlink_group) WHERE hardlink_group IS NOT NULL;
-- Detecção de diretório já visitado por outro caminho (bind mount/alias) por (dev, inode).
CREATE INDEX idx_folders_physical ON folders(volume_id, native_dev, native_file_id);

-- Symlinks, FIFOs, sockets e dispositivos acumulados na subárvore (não entram em
-- recursive_file_count, que conta só arquivos regulares).
ALTER TABLE folder_stats ADD COLUMN recursive_special_count INTEGER NOT NULL DEFAULT 0
    CHECK (recursive_special_count >= 0);

-- Pasta em que o problema ocorreu (a que estava sendo lida, ou a que contém a entrada).
-- Serve para marcar o agregado como 'parcial'. severity 'info' = item ignorado de propósito
-- (exclusão, pseudo-filesystem, limite de volume, alias): não torna o agregado parcial.
ALTER TABLE scan_errors ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL;
CREATE INDEX idx_scan_errors_folder ON scan_errors(folder_id);
-- Idempotência na retomada: o mesmo problema no mesmo caminho é registrado uma vez por execução.
CREATE UNIQUE INDEX ux_scan_errors_dedup ON scan_errors(run_id, phase, error_code, path_raw)
    WHERE path_raw IS NOT NULL;

-- Totais por volume da última agregação.
CREATE TABLE volume_stats (
    volume_id             INTEGER PRIMARY KEY REFERENCES volumes(id) ON DELETE CASCADE,
    run_id                INTEGER REFERENCES scan_runs(id) ON DELETE SET NULL,
    folder_count          INTEGER NOT NULL DEFAULT 0 CHECK (folder_count >= 0),
    file_count            INTEGER NOT NULL DEFAULT 0 CHECK (file_count >= 0),
    special_count         INTEGER NOT NULL DEFAULT 0 CHECK (special_count >= 0),
    logical_bytes         INTEGER NOT NULL DEFAULT 0 CHECK (logical_bytes >= 0),
    allocated_entry_bytes INTEGER CHECK (allocated_entry_bytes IS NULL OR allocated_entry_bytes >= 0),
    physical_unique_bytes INTEGER CHECK (physical_unique_bytes IS NULL OR physical_unique_bytes >= 0),
    issue_count           INTEGER NOT NULL DEFAULT 0 CHECK (issue_count >= 0),
    aggregate_state       TEXT    NOT NULL DEFAULT 'pendente'
                          CHECK (aggregate_state IN ('pendente','parcial','completo','desatualizado')),
    updated_at            INTEGER NOT NULL
) STRICT;
