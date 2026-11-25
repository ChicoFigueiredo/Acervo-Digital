# Erros esperados e aceitos: a varredura só é "com erros" quando há erro real

Decidido em 24/11/2026, a pedido do usuário, depois de ver toda varredura terminar `concluida_com_erros`.

**Problema.** No Catálogo real (execução 7, Fast Scan de 6 Raízes) havia 7.320 linhas em `scan_errors`, quase todas benignas: 5.390 `Permission denied` no hash (`/swapfile`, `/etc/shadow`, bancos do SQL Server), 1.325 symlinks de NTFS/WSL (`node_modules/.bin`, `venv/bin`) cujo `readlink` devolve `EINVAL`, 346 arquivos vivos que mudaram durante o hash (`/var/log`, `~/.cache`, SQLite `-wal`) e 154 arquivos temporários que sumiram. Qualquer linha não informativa fazia a execução terminar `concluida_com_erros`, então o estado não dizia nada. A execução 1 (Full Scan) chegou a 238 mil erros de hash, a maioria em `~/.bun` e `~/.cache`.

**Decisão.** Cada problema registrado tem um desfecho:

- **esperado**: benigno conhecido, decidido por regras **embutidas** (documentadas em `dag erros regras`, `dag_core::erros::REGRAS_EMBUTIDAS`);
- **aceito**: casa uma regra do usuário ("Erros aceitos": glob de caminho, código ou `*`, motivo), guardada no Catálogo (`erros_aceitos`);
- **erro**: o resto. Severidade `aviso` (p. ex. permissão negada) continua aviso: aparece agrupado, com a dica `dag scan --elevar` ou "aceite esta pasta".

Regras embutidas: (1) arquivo que mudou ou sumiu durante o hash (`hash_alterado_*`, `hash_ausente`, `nao_encontrado`) em caminho volátil (`~/.cache`, `~/.bun`, `~/.npm`, `~/.cargo/registry`, `/var/log`, `/var/cache`, `/var/tmp`, `*-wal`, `*-shm`, `*.lock`, caches de perfis de navegador); (2) **reteste**: no fim da etapa de hash, cada arquivo que mudou ou sumiu, em qualquer pasta, é relido **uma única vez**. Estável: o hash é gravado (a linha do inventário ganha o tamanho e as datas de agora) e o problema é apagado, sem erro nenhum. Ainda mudando ou ausente: fica como esperado (`regra = embutida:reteste`); (3) symlink de NTFS com `EINVAL` no `readlink` é gravado como symlink com alvo "alvo ilegível (reparse point NTFS)" e uma linha `info` (`symlink_alvo_ilegivel`); (4) limites de volume, volume pseudo e pastas excluídas (já eram `info`).

**Estado da execução** (valores novos só acrescentam; execuções antigas mantêm o estado gravado):

- `concluida`: nenhum problema além de informativos;
- `concluida_com_avisos`: só avisos, esperados ou aceitos;
- `concluida_com_erros`: ao menos um erro real (severidade erro ou impeditivo, sem regra que o aceite).

A migração 0010 acrescenta `scan_errors.classe`/`regra`, a tabela `erros_aceitos` e o valor ao `CHECK` de `scan_runs.status`. O SQLite não altera `CHECK`; como só se acrescenta um valor à lista `IN`, a migração usa o procedimento documentado de `writable_schema` em vez de recriar `scan_runs` (recriar apagaria, por `ON DELETE`, o que aponta para ela). Linhas antigas ficam com `classe` nula e são classificadas na leitura.

**Exclusões.** `dag excluir <glob>` reaproveita `scan_exclusions` (A-19, justificativa registrada) e o `ConfigScan.exclusoes` do scanner, com uma nova forma de casamento por glob. Vale nas próximas varreduras e no monitor; o que casa não é varrido nem hasheado.

**Visibilidade.** `dag erros [--execucao N] [--agrupar codigo|pasta]`, `dag erros regras|aceitar|remover`, `dag excluir`, MCP (`erros_da_varredura`, `erros_regras`, `erros_aceitar`, `erros_remover`, `excluir`, `exclusao_remover`) e a seção "Erros da varredura" da página Varredura, com os botões "Aceitar este tipo nesta pasta" e "Excluir pasta".

**Consequências.** Regras do usuário valem também para execuções já gravadas na exibição (a classificação é refeita na leitura), mas não reescrevem o estado dessas execuções. O reteste lê cada arquivo problemático uma segunda vez, em série; o custo é proporcional ao número de arquivos vivos, não ao Catálogo. Alternativas rejeitadas: ignorar esses códigos sem registrá-los (some a evidência), classificar só pelo caminho (esconderia mudança real em arquivo importante), recriar `scan_runs` para ampliar o `CHECK`.
