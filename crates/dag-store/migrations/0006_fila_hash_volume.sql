-- Desempenho da fila de hash em catálogos grandes (dezenas de milhões de arquivos).
-- `volume_id` é copiado de `files` ao enfileirar (a fila é reconstruída a cada etapa, então não
-- há retroativo a fazer: itens antigos com NULL só valem para a reserva sem filtro de volume).
-- O índice atende a reserva por volume em ordem de prioridade e id sem varrer a fila inteira.
ALTER TABLE fila_hash ADD COLUMN volume_id INTEGER;
CREATE INDEX idx_fila_hash_volume ON fila_hash(tipo, state, volume_id, priority DESC);
