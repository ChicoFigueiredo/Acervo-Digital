# Servidor MCP do DAG

`dag mcp` é um servidor [Model Context Protocol](https://modelcontextprotocol.io) (versão `2025-06-18`) por **stdio**: JSON-RPC 2.0, uma mensagem por linha. O stdout carrega só quadros do protocolo; logs vão para o stderr. O MCP não toca no Catálogo: repassa cada operação ao agente (único escritor, [ADR 0004](adr/0004-agente-escritor-unico-e-assinatura-de-eventos.md)), identificando o autor como `mcp` nas Marcações.

Decisão: [ADR 0002](adr/0002-mcp-com-todas-as-operacoes.md). Requisitos: A-35 e CA-A28.

## Como registrar

Claude Code:

```sh
claude mcp add dag -- dag mcp
```

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "dag": { "command": "dag", "args": ["mcp"] }
  }
}
```

Para usar outro Catálogo, defina `DAG_CATALOGO` no ambiente do servidor (`"env": {"DAG_CATALOGO": "/caminho/catalogo.sqlite"}`) ou passe `--catalogo` antes do subcomando.

O MCP conversa com o agente pelo socket local e abre uma conexão por chamada. Se não há agente, a primeira ferramenta de escrita o lança destacado (systemd de usuário se instalado, senão `dag` ao lado do executável ou no PATH; erro claro se não achar). Escritas funcionam com a GUI aberta, porque a GUI também é só cliente. `dag --sem-agente mcp` (ou `DAG_SEM_AGENTE=1`) volta ao modo antigo, com sessão própria.

Ferramenta `jobs`: lista todos os trabalhos do agente (qualquer origem, com progresso). `pausar`, `retomar` e `cancelar` aceitam `job` (id do job) além de `execucao`.

## Garantias

- **Nada é apagado nem alterado no acervo.** `marcar_delecao` apenas registra uma candidata a descarte no Catálogo. A única exceção é a Lixeira em dois níveis (ADR 0005): `mover_para_lixeira` e `restaurar_da_lixeira` fazem `rename` (sem sobrescrever, no mesmo Volume) de e para a pasta `DAG-Lixeira` do Volume. `mover_para_lixeira` exige `confirmacao_1` e `confirmacao_2` verdadeiros, depois de uma chamada com `simular: true` mostrada ao usuário. Esvaziar a Lixeira é ato do usuário, fora do DAG.
- **Senhas nunca passam pelo MCP.** `servico` pode abrir o diálogo do sistema (Polkit), onde o usuário digita a senha; o MCP só recebe o resultado.

## Ferramentas

| Ferramenta | O que faz | Argumentos |
|---|---|---|
| `listar_volumes` | Volumes conhecidos | — |
| `scan_full` | Full Scan (inventário, hash, duplicados, reavaliação) | `raizes` (obrigatório), `sem_hash`, `releitura_total` (lê tudo de novo), `threads_por_disco`, `max_discos` |
| `scan_fast` | Fast Scan (reconcilia) | `raizes`, `sem_hash`, `threads_por_disco`, `max_discos` |
| `retomar` / `pausar` / `cancelar` | Controle de execução (`retomar` também aceita `threads_por_disco`, `max_discos` para execução gravada) | `execucao` |
| `status` | Progresso (com "interrompida, retomando"), totais, saúde do monitor e Ponto de Confiança por Raiz | `execucao` |
| `explorar` | Filhos de uma pasta, do maior para o menor | `pasta`, `limite`, `cursor` |
| `listar_duplicados` | Grupos de duplicados paginados | `volume`, `tamanho_minimo`, `ordem`, `limite`, `cursor` |
| `detalhar_grupo` | Cópias, Marcações e Cobertura de um grupo | `grupo` |
| `definir_copia_principal` | Troca (ou volta ao automático) a Cópia Principal | `grupo`, `arquivo` |
| `marcar_backup` / `marcar_delecao` / `desmarcar` | Marcações (só no Catálogo) | `alvo` `{tipo, valor}` |
| `mover_para_lixeira` | Move cópias com Marcação para Deleção para a `DAG-Lixeira` do Volume (dupla confirmação) | `alvos`, `simular`, `confirmacao_1`, `confirmacao_2`, `confirmacao_unica_copia`, `marcar_tudo` |
| `restaurar_da_lixeira` | Devolve Na Lixeira ao caminho de origem (sufixo se ocupado) | `alvos` |
| `listar_lixeira` | O que está Na Lixeira e os Descartados | `limite`, `cursor` |
| `confirmar_marcacao` | Reconfirma Marcação Pendente | `marcacao` |
| `definir_prioridade` | Prioridade de Volume | `ordem` |
| `relatorio` | Cobertura, sem cobertura, espaço, marcações | `tipo`, `nivel`, `saida`, `limite`, `cursor` |
| `verificar_integridade` | Verificação de Integridade | `raizes`, `threads_por_disco`, `max_discos` |
| `config_hash_obter` | Paralelismo padrão do Hash Completo persistido (padrão 1 thread por disco, 16 discos) | — |
| `config_hash_definir` | Grava o paralelismo padrão (vale na próxima execução ou retomada) | `threads_por_disco` (1..8), `max_discos` (1..16) |
| `erros_da_varredura` | Erros da varredura classificados (esperado, aceito, aviso, erro real), por código e por pasta, com amostras | `execucao`, `agrupar`, `codigo`, `pasta`, `amostras` |
| `erros_regras` / `erros_aceitar` / `erros_remover` | Regras embutidas, regras de erros aceitos do usuário (glob + código) | `padrao`, `codigo`, `motivo`, `id` |
| `excluir` / `exclusao_remover` | Exclusões do escopo das próximas varreduras (glob) | `padrao`, `motivo`, `id` |
| `eventos` / `saude_monitor` | Eventos persistidos e saúde do monitor | `limite` |
| `servico` | `instalar`, `remover`, `iniciar`, `parar`, `autostart` | `acao`, `ativo` |
| `backup_catalogo` | Backup consistente do Catálogo | `destino` |

Paralelismo do hash: valor do pedido > flag do agente (`dag agente --threads-por-disco/--max-discos`) > valor persistido (`config_hash_definir`) > padrão (1 por disco, 16 discos). Fora da faixa dá erro. HDD: 1 é o melhor; SSD/NVMe podem ganhar com 2 a 4. Uma execução em andamento mantém o valor com que começou; a mudança vale na próxima execução ou retomada.

`alvo.tipo` é `caminho`, `arquivo` (id), `grupo` (id), `volume` ou `pasta`. O monitoramento contínuo (`dag monitorar`) fica só na CLI, pois roda em primeiro plano até Ctrl-C.

Cada resposta traz `content` (resumo em português) e `structuredContent` (a `Resposta` completa em JSON). Falhas voltam com `isError: true`. Em varreduras, se o cliente enviar `_meta.progressToken`, o servidor emite `notifications/progress`.

## Privacidade (ADR 0002)

Clientes MCP como o Claude processam as respostas na nuvem. Por isso a regra "nunca enviar o inventário à nuvem" tem uma **exceção deliberada**: caminhos e metadados do Catálogo podem ir ao cliente MCP que **você mesmo configurou**. Nenhuma outra via envia o inventário para fora da máquina. Se não quiser isso, não registre o `dag mcp` no seu cliente.
