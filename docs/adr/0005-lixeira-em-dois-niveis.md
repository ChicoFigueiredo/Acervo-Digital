# Lixeira em dois níveis: o DAG passa a mover, mas nunca apaga

Decidido em 24/11/2026, a pedido do usuário. Substitui a regra "o DAG é somente leitura para os arquivos catalogados" (CLAUDE.md, A-29, CA-A15, MCP.md) por uma exceção única e estreita.

**Problema.** A Marcação para Deleção só fica no Catálogo; o usuário ainda precisa achar e apagar cada cópia à mão, fora do DAG. Faltava um passo intermediário reversível entre "marcado" e "apagado".

**Decisão.** A Lixeira tem dois níveis:

1. **Marcação para Deleção** (já existente): só no Catálogo, não toca o disco.
2. **Na Lixeira**: a cópia marcada é movida para a **Lixeira do Volume**, a pasta visível `DAG-Lixeira` na raiz do sistema de arquivos onde ela já está. O Catálogo guarda o caminho de origem.

A única escrita que o DAG faz nos arquivos catalogados é esse rename: da origem para a `DAG-Lixeira` do mesmo Volume e de volta (restaurar). O DAG nunca apaga, sobrescreve nem altera conteúdo, e nunca move entre Volumes. Esvaziar a Lixeira é sempre ato do usuário, fora do DAG.

**Regras.**
- Uma Lixeira por Volume, não por Dispositivo Físico: dentro do mesmo sistema de arquivos mover é `rename(2)`, instantâneo, sem copiar dados nem exigir espaço livre. Entre Volumes seria copiar e apagar, o que esta ADR proíbe.
- Só cópia com Marcação para Deleção vai para a Lixeira. Única Cópia pode ir, mas exige confirmação extra dizendo que é a última cópia.
- Cópia Na Lixeira não conta como outra cópia: não evita o alerta de Única Cópia nem dá Cobertura.
- Restaurar com o caminho de origem ocupado volta com nome novo (sufixo), nunca sobrescreve. Pasta de origem ausente é recriada.
- Scans visitam a `DAG-Lixeira` e reconhecem o que está lá. O que o usuário põe na Lixeira à mão passa a ter Marcação para Deleção e ficar Na Lixeira; o que ele tira volta a ser cópia comum, sem marcação.
- O que estava Na Lixeira e some (o usuário esvaziou) vira registro **Descartado**: caminho, hash e data ficam no histórico.
- Sem permissão para o rename, o helper Polkit faz só essa operação, com senha no diálogo do sistema.
- GUI, CLI e MCP podem mover e restaurar (segue a ADR 0002). Só o agente, escritor único (ADR 0004), executa o rename.

**Consequências.** O DAG deixa de ser somente leitura; CA-A15 e A-29 precisam ser reescritos para "nada é apagado nem alterado, só movido para a Lixeira do próprio Volume e de volta". Os testes de segurança de dados passam a verificar que nenhum caminho de código chama unlink, write ou rename fora da Lixeira. Alternativas rejeitadas: deixar o nível 2 só no Catálogo (não resolve o trabalho manual), deixar o DAG esvaziar a Lixeira (único passo irreversível sai da mão do usuário), uma Lixeira por Dispositivo Físico (obrigaria copiar e apagar entre partições).
