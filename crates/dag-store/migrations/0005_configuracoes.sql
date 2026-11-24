-- Configurações do usuário persistidas no Catálogo (chave/valor). Só acrescenta.
-- Hoje: `hash.threads_por_disco` e `hash.max_discos` (paralelismo do Hash Completo).
CREATE TABLE configuracoes (
    chave TEXT PRIMARY KEY,
    valor TEXT NOT NULL
) STRICT;
