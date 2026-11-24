# Agente como escritor único; clientes assinam eventos

Decidido em 24/11/2026, a pedido do usuário ("a interação dos serviços em background com a GUI, temos muito que melhorar").

**Problema.** Quem abria o Catálogo primeiro virava o escritor dentro do próprio processo (worker da GUI, CLI ou `dag mcp` com `Conexao::abrir`). A GUI não via nem controlava trabalhos iniciados pelo agente, pela CLI ou pela rotina de abertura, e o contrário também.

**Decisão.**
1. O agente (`dag agente`, socket Unix, NDJSON) é o único escritor. GUI, CLI e MCP são clientes. Se não há agente, o cliente o lança destacado (`dag` ao lado do executável atual ou no PATH; `systemctl --user start dag-agente` se a unidade estiver instalada; log em `<catálogo>.agente.log`). A corrida entre dois clientes é decidida por flock (`<catálogo>.lock.lancar` para quem lança, `<catálogo>.lock` para quem escreve): sobra um agente. Sem `dag` encontrável o erro é claro e aponta `--sem-agente`.
2. `--sem-agente` (ou `DAG_SEM_AGENTE=1`) mantém o modo antigo, em processo. Também é o que a GUI usa com volumes fixos de teste.
3. **Trabalhos (jobs).** Varreduras, retomadas, verificação de integridade, a abertura e o monitor são jobs com id (por execução do agente), tipo, origem (gui/cli/mcp/abertura/monitor), raízes, estado (enfileirado/executando/pausado/cancelando) e progresso. Comandos `ListarJobs`, `PausarJob`, `RetomarJob`, `CancelarJob` valem para qualquer job, inclusive os enfileirados, a partir de qualquer cliente.
4. **Assinatura.** `Assinar` transforma a conexão em fluxo: um instantâneo primeiro (assinante tardio), depois eventos (job enfileirado/iniciado/atualizado/progresso/finalizado, estado do monitor e lacunas, mudanças de Ponto de Confiança) e uma batida a cada 3 s. Assinante que não acompanha (fila de 4096) é descartado e reassina. O cliente `Observador` reconecta com recuo e a GUI mostra "Agente desconectado, reconectando…".
5. Abertura e monitor só rodam no agente (exceto `--sem-agente`). Leituras podem usar uma sessão somente leitura sem lançar nada.

**Nota (ordem de lançamento).** Para que fechar a GUI jamais pare jobs (inclusive quando o KDE a põe num escopo `app-*.scope` ou ela veio de um terminal), o cliente lança nesta ordem: unidade `dag-agente.service` instalada; `systemd-run --user --unit=dag-agente-<hash do catálogo> --collect` com saída `append:` no log (systemd >= 240; unidade já ativa conta como sucesso); `setsid`; processo em grupo próprio. `DAG_SEM_SYSTEMD=1` pula os dois primeiros.

**Adendo (serviço como dono normal do Catálogo).** `dag servico instalar` passa a escrever a unidade (`ExecStart=dag --catalogo <caminho> agente`), recarregar e executar `enable --now`; `--iniciar-no-boot` liga o linger (falha de Polkit vira aviso), `--sem-iniciar` mantém o comportamento antigo. Entrega: se outro agente (GUI/setsid/systemd-run) segura a trava de escritor, o instalador envia SIGTERM ao pid gravado na trava (somente se `/proc/<pid>/cmdline` for `dag ... agente`), espera a parada limpa e inicia o serviço, que retoma a execução `interrompida`; nunca SIGKILL. Em vez de um comando `Encerrar` no protocolo, usou-se o tratamento de SIGTERM que o agente já tem. A GUI mostra o serviço em Configurações e um aviso dispensável no Início.

**Consequências.** O worker do agente continua serial: pausar um job trava a fila e escritas rápidas esperam uma varredura longa. `dag monitorar` com agente responde que o agente já monitora (o primeiro plano exige `--sem-agente`). A vigia do agente consulta monitor e pontos de confiança a cada 1 s, só com assinantes.

**Atualização de uma máquina em que a GUI é a escritora com hash rodando.** Feche a GUI antiga (ela segura a trava; enquanto aberta o novo agente não sobe e o erro cita o pid). A execução fica `em_andamento` no Catálogo; o novo agente, ao abrir, a marca `interrompida` e a retoma sem reler o já hasheado (ADR 0003). Nada é perdido.

**Alternativas rejeitadas.** Manter o escritor em processo com um canal entre GUIs (não alcança CLI/MCP); polling do status em vez de eventos (atraso, sem progresso fino); exigir a unidade systemd (nem toda máquina a instala).
