# Servidor MCP do DAG

`dag mcp` é um servidor [Model Context Protocol](https://modelcontextprotocol.io) (versão `2025-06-18`) por **stdio**: JSON-RPC 2.0, uma mensagem por linha. O stdout carrega só quadros do protocolo; logs vão para o stderr. O MCP não toca no Catálogo: repassa cada operação ao worker (único escritor), identificando o autor como `mcp` nas Marcações.

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

Com o serviço (daemon) ativo, o MCP conversa com ele pelo socket local. Sem daemon, o MCP abre uma sessão própria como escritor do Catálogo; com GUI e MCP abertos ao mesmo tempo existe um único escritor.

## Garantias

- **Somente leitura sobre o acervo.** Nenhuma ferramenta apaga, move ou altera arquivos. `marcar_delecao` apenas registra uma candidata a descarte no Catálogo.
- **Senhas nunca passam pelo MCP.** `servico` pode abrir o diálogo do sistema (Polkit), onde o usuário digita a senha; o MCP só recebe o resultado.

## Ferramentas

| Ferramenta | O que faz | Argumentos |
|---|---|---|
| `listar_volumes` | Volumes conhecidos | — |
| `scan_full` | Full Scan (inventário, hash, duplicados, reavaliação) | `raizes` (obrigatório), `sem_hash` |
| `scan_fast` | Fast Scan (reconcilia) | `raizes`, `sem_hash` |
| `retomar` / `pausar` / `cancelar` | Controle de execução | `execucao` |
| `status` | Progresso, totais e saúde do monitor | `execucao` |
| `explorar` | Filhos de uma pasta, do maior para o menor | `pasta`, `limite`, `cursor` |
| `listar_duplicados` | Grupos de duplicados paginados | `volume`, `tamanho_minimo`, `ordem`, `limite`, `cursor` |
| `detalhar_grupo` | Cópias, Marcações e Cobertura de um grupo | `grupo` |
| `definir_copia_principal` | Troca (ou volta ao automático) a Cópia Principal | `grupo`, `arquivo` |
| `marcar_backup` / `marcar_delecao` / `desmarcar` | Marcações (só no Catálogo) | `alvo` `{tipo, valor}` |
| `confirmar_marcacao` | Reconfirma Marcação Pendente | `marcacao` |
| `definir_prioridade` | Prioridade de Volume | `ordem` |
| `relatorio` | Cobertura, sem cobertura, espaço, marcações | `tipo`, `nivel`, `saida`, `limite`, `cursor` |
| `verificar_integridade` | Verificação de Integridade | `raizes` |
| `eventos` / `saude_monitor` | Eventos persistidos e saúde do monitor | `limite` |
| `servico` | `instalar`, `remover`, `iniciar`, `parar`, `autostart` | `acao`, `ativo` |
| `backup_catalogo` | Backup consistente do Catálogo | `destino` |

`alvo.tipo` é `caminho`, `arquivo` (id), `grupo` (id), `volume` ou `pasta`. O monitoramento contínuo (`dag monitorar`) fica só na CLI, pois roda em primeiro plano até Ctrl-C.

Cada resposta traz `content` (resumo em português) e `structuredContent` (a `Resposta` completa em JSON). Falhas voltam com `isError: true`. Em varreduras, se o cliente enviar `_meta.progressToken`, o servidor emite `notifications/progress`.

## Privacidade (ADR 0002)

Clientes MCP como o Claude processam as respostas na nuvem. Por isso a regra "nunca enviar o inventário à nuvem" tem uma **exceção deliberada**: caminhos e metadados do Catálogo podem ir ao cliente MCP que **você mesmo configurou**. Nenhuma outra via envia o inventário para fora da máquina. Se não quiser isso, não registre o `dag mcp` no seu cliente.
