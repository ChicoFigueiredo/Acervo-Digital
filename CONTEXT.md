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

**Dispositivo Físico**:
Disco ou mídia física que hospeda um ou mais Volumes. Dois Volumes no mesmo Dispositivo Físico falham juntos.
_Avoid_: HD, disco (quando se quer dizer o Volume)

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

**Execução Interrompida**:
Varredura ou etapa de hash que um processo morto deixou pela metade (queda de energia, travamento, encerramento do programa). Continua de onde parou, sem refazer o que já está gravado.
_Avoid_: varredura travada, scan órfão, execução cancelada (cancelar é decisão do usuário)

**Ponto de Confiança**:
Situação de uma Raiz em que o Catálogo foi conferido contra o disco sem achar nenhuma mudança e, a partir daí, só o monitoramento a mantém em dia. Qualquer falha do monitoramento ou queda do programa tira a Raiz dele.
_Avoid_: sincronizado, atualizado, sincronia

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
Condição de um arquivo que tem ao menos uma cópia com Marcação de Backup em outro Dispositivo Físico.
_Avoid_: protegido, redundante

**Marcação para Deleção**:
Primeiro nível da Lixeira: decisão registrada no Catálogo de que uma cópia é candidata a descarte. Não apaga nem move nada.
_Avoid_: excluir, lixo, lixeira lógica

**Lixeira do Volume**:
Pasta na raiz de cada Volume para onde o DAG move cópias com Marcação para Deleção. O DAG só move para ela e de volta; esvaziá-la é sempre ato do usuário, fora do DAG.
_Avoid_: lixeira do HD, lixeira do sistema, trash

**Na Lixeira**:
Segundo nível da Lixeira: cópia com Marcação para Deleção que já foi movida para a Lixeira do Volume em que estava. O Catálogo guarda o caminho de origem para restaurá-la.
_Avoid_: deletado, excluído, descartado

**Descartado**:
Registro histórico de uma cópia que estava Na Lixeira e deixou de existir porque o usuário esvaziou a Lixeira do Volume. Guarda o que era e quando sumiu; não faz mais parte do acervo.
_Avoid_: deletado, perdido, removido (remoção comum não é descarte)

**Única Cópia**:
Arquivo cujo conteúdo não existe em nenhum outro arquivo físico do Catálogo.
_Avoid_: original, exclusivo

**Marcação Pendente**:
Marcação de Backup ou para Deleção cujo arquivo mudou de conteúdo depois de marcado. Sai de todas as contas até o usuário confirmá-la de novo.
_Avoid_: marcação inválida, expirada

### Contêineres

**Contêiner**:
Arquivo do Catálogo cujo conteúdo o DAG lê por dentro, em fluxo e na memória, sem nunca extrair nada em disco: um Compactado ou uma Imagem de Disco.
_Avoid_: pacote, arquivo-mãe

**Compactado**:
Contêiner de formato de arquivamento ou compressão: zip, 7z, rar, tar e tar comprimido (gz, bz2, xz, zst).
_Avoid_: zipado, arquivo zip (para qualquer formato)

**Imagem de Disco**:
Contêiner que guarda um disco ou partição inteiros (VHDX, qcow2, img bruto, ISO). O DAG lê a tabela de partições e o sistema de arquivos com leitor próprio, sem montar.
_Avoid_: disco virtual, VM

**Membro**:
Entrada dentro de um Contêiner, identificada pelo Contêiner e pelo caminho interno. Recebe metadados e Hash Completo; um Membro que também é Contêiner é lido por dentro (aninhado).
_Avoid_: item, entrada interna, arquivo extraído

**Senha do Contêiner**:
Senha que abre um Contêiner cifrado, guardada cifrada no Catálogo e ligada ao Hash Completo do Contêiner, de modo que cópias idênticas abrem com ela.
_Avoid_: senha do arquivo, credencial

**Cofre de Senhas**:
Lista de senhas que o usuário mais usa, guardada cifrada, que o DAG tenta sozinho em todo Contêiner cifrado antes de pedir ajuda.
_Avoid_: dicionário, chaveiro (o chaveiro é o do sistema)

**Fila de Serviço**:
Lista de pendências que só o usuário resolve, mostrada na GUI sem popups e servida pelo agente a qualquer cliente. O trabalho nunca para esperando por ela.
_Avoid_: notificações, alertas, popup

**Pedido de Serviço**:
Um item da Fila de Serviço, com tipo, objeto, motivo e estado (aberto, resolvido, dispensado). Na primeira versão os tipos são Pedido de Senha e Falha de Leitura.
_Avoid_: tarefa, ticket

### Erros da varredura

**Erro Esperado**:
Problema registrado numa varredura que o DAG reconhece como benigno por regra embutida: arquivo vivo (log, cache, `-wal`) que mudou ou sumiu durante o hash e continuou assim no reteste, symlink de NTFS de alvo ilegível, limite de Volume. Não torna a execução `concluida_com_erros`.
_Avoid_: erro ignorado, falso positivo

**Erro Aceito**:
Problema que casa uma regra criada pelo usuário (glob de caminho, código de erro ou qualquer, motivo). Conta como aviso, nunca como erro real.
_Avoid_: erro silenciado, exceção

**Reteste**:
Releitura única, no fim da etapa de Hash Completo, dos arquivos que mudaram ou sumiram durante a leitura. Estável: ganha hash e o problema some. Ainda mudando ou ausente: vira Erro Esperado.
_Avoid_: segunda tentativa, rehash

### Biblioteca

**Biblioteca Homologada**:
Biblioteca Calibre oficial do usuário, já curada. É a referência de autores, tags, séries e estantes; o DAG só a lê, e qualquer mudança nela exige aprovação explícita do usuário.
_Avoid_: biblioteca oficial, biblioteca principal, Calibre (sozinho)

**A Classificar**:
Biblioteca Calibre separada, criada pelo DAG, onde entram os livros candidatos para revisão antes da homologação. É área de trabalho: estar nela não autoriza descartar nada.
_Avoid_: pré-catálogo, staging, caixa de entrada

## Relationships

- Um **Grupo de Duplicados** contém dois ou mais arquivos físicos; caminhos do **Mesmo Arquivo Físico** contam como um só.
- Qualquer arquivo pode ter **Marcação para Deleção**; se não houver outra cópia, ele recebe alerta de **Única Cópia**.
- Só uma cópia com **Marcação para Deleção** pode ir para a **Lixeira do Volume**, e sempre para a do próprio **Volume**, nunca de outro.
- Uma **Única Cópia** pode ir para a **Lixeira do Volume**, mas só com confirmação extra que diga que é a última cópia.
- Uma cópia **Na Lixeira** não conta como outra cópia: não evita o alerta de **Única Cópia** nem dá **Cobertura**.
- O usuário também pode mover à mão: o que ele põe na **Lixeira do Volume** passa a ter **Marcação para Deleção** e ficar **Na Lixeira**; o que ele tira volta a ser cópia comum, sem marcação.
- Só vira **Descartado** o que estava **Na Lixeira** e sumiu; arquivo que some de outro lugar é remoção comum.
- Uma cópia pode ter **Marcação de Backup** ou **Marcação para Deleção**, nunca as duas.
- **Cobertura** só existe quando a cópia marcada está em outro **Dispositivo Físico**; outra partição do mesmo disco não conta.
- Um **Dispositivo Físico** hospeda um ou mais **Volumes**.
- Uma **Execução Interrompida** continua sozinha ao abrir o programa; depois dela, o **Fast Scan** é repetido na **Raiz** até uma passada sem mudanças, e só então a **Raiz** entra no **Ponto de Confiança**.
- Ao abrir, toda **Raiz** passa por um **Fast Scan** (a menos que o usuário escolha o contrário); só arquivos novos ou alterados recebem **Hash Completo**. Um **Full Scan** com hash de tudo só acontece por pedido explícito.
- Um **Membro** pertence a um **Contêiner** e conta como cópia: entra em **Grupo de Duplicados**, evita o alerta de **Única Cópia** e dá **Cobertura** quando o **Contêiner** tem **Marcação de Backup** em outro **Dispositivo Físico**. Um **Membro** nunca soma espaço recuperável.
- Um **Membro** não recebe marcação própria; marcações valem para o **Contêiner** inteiro.
- Uma execução termina `concluida` (nenhum problema), `concluida_com_avisos` (só avisos, **Erros Esperados** ou **Erros Aceitos**) ou `concluida_com_erros` (algum erro real).
- Um **Contêiner** cifrado é tentado primeiro com a **Senha do Contêiner** já guardada, depois com o **Cofre de Senhas**; só se tudo falhar vira **Pedido de Serviço** de senha, e os **Membros** ficam não lidos até a resposta.
- Um livro só é sugerido para a **Lixeira do Volume** se tiver **Cobertura** com hash conferido; o fluxo bibliográfico não cria outro conceito de backup.
- A **Biblioteca Homologada** e **A Classificar** são bibliotecas Calibre independentes; nada da **A Classificar** entra na **Biblioteca Homologada** sem aprovação do usuário.

## Flagged ambiguities

- "backup" foi usado para a cópia preservada e para a Fase C (backup gerenciado). Aqui, a cópia preservada é **Marcação de Backup**; a Fase C não está neste escopo.
- "compactado" às vezes quer dizer qualquer coisa que se abre por dentro, inclusive imagens de disco. Aqui, o termo geral é **Contêiner**; **Compactado** e **Imagem de Disco** são os dois tipos.
- "backup verificado" na spec da Fase B é a **Cobertura**; "lixeira" é a **Lixeira do Volume**.
