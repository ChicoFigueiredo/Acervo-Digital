# DAG — Digital Asset Governance

Plataforma local que cataloga o acervo digital (volumes, pastas e arquivos), mede o espaço consumido, identifica conteúdo idêntico e registra quais cópias são preservação e quais são candidatas a descarte. Este arquivo é só o glossário; decisões ficam em `docs/adr/`.

## Language

### Inventário

**Catálogo**:
Banco local que guarda o retrato do acervo: volumes, pastas, arquivos, hashes, marcações e histórico.
_Avoid_: inventário (como substantivo do banco), snapshot, índice

**Volume**:
Sistema de arquivos identificado de forma estável, conectado ou desconectado.
_Avoid_: disco, HD, drive (quando se quer dizer o sistema de arquivos)

**Raiz**:
Volume ou pasta escolhido pelo usuário como escopo de uma varredura.
_Avoid_: root, ponto de partida

**Full Scan**:
Varredura que visita todo o escopo, registra metadados de cada entrada e calcula o Hash Completo de todos os arquivos.
_Avoid_: varredura completa, scan inicial

**Fast Scan**:
Varredura de reconciliação que revisita o escopo e reaproveita metadados e hash das entradas comprovadamente inalteradas, atualizando só o que foi adicionado, alterado, renomeado ou removido.
_Avoid_: quick scan, varredura incremental

**Verificação de Integridade**:
Releitura agendada do conteúdo de arquivos cujo hash foi reaproveitado, para detectar corrupção silenciosa que metadados inalterados não revelam.
_Avoid_: rehash, auditoria

### Conteúdo

**Hash Completo**:
Impressão digital de 100% do conteúdo de um arquivo. Todo arquivo do escopo recebe um.
_Avoid_: full hash, checksum

**Mesmo Arquivo Físico**:
Dois ou mais caminhos que apontam para o mesmo conteúdo físico no mesmo sistema de arquivos (hard links). Não é duplicado e não gera espaço recuperável.
_Avoid_: duplicado, cópia

**Grupo de Duplicados**:
Conjunto de dois ou mais arquivos físicos distintos com o mesmo Hash Completo e o mesmo tamanho.
_Avoid_: duplicata, clone

**Cópia Principal**:
A cópia de um Grupo de Duplicados escolhida como referência, por Prioridade de Volume ou por escolha manual do usuário.
_Avoid_: original, master

**Prioridade de Volume**:
Ordem definida pelo usuário entre Volumes e pastas que decide automaticamente a Cópia Principal de cada Grupo de Duplicados.
_Avoid_: preferência, ranking

### Marcações

**Marcação de Backup**:
Decisão registrada no Catálogo de que uma cópia dentro de um Grupo de Duplicados é intencional e deve ser preservada. Tem dois efeitos: a cópia sai do espaço recuperável e o arquivo correspondente passa a ter Cobertura.
_Avoid_: backup (sozinho), cópia de segurança

**Cobertura**:
Condição de um arquivo que tem ao menos uma cópia com Marcação de Backup em outro Volume.
_Avoid_: protegido, redundante

**Marcação para Deleção**:
Decisão registrada no Catálogo de que uma cópia é candidata a descarte. Não apaga nem move nada.
_Avoid_: excluir, lixo

## Relationships

- Um **Grupo de Duplicados** contém dois ou mais arquivos físicos; caminhos do **Mesmo Arquivo Físico** contam como um só.
- Uma cópia pode ter **Marcação de Backup** ou **Marcação para Deleção**, nunca as duas.
- **Cobertura** só existe quando a cópia marcada está em outro **Volume**.

## Flagged ambiguities

- "backup" foi usado para a cópia preservada e para a Fase C (backup gerenciado). Aqui, a cópia preservada é **Marcação de Backup**; a Fase C não está neste escopo.
