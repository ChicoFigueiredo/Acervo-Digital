# Guia de operação da Curadoria Bibliográfica

Este guia reúne a operação por tela e por terminal. Esta primeira seção é a do **terminal
(`dag biblioteca ...` e `dag modelos ...`)**; as outras frentes acrescentam as suas abaixo.

## CLI

### Antes de começar

- Tudo que o DAG sabe sobre os livros vem do Catálogo. Para as duplicatas aparecerem, as pastas
  precisam ter sido varridas com hash: `dag scan --full /mnt/e /mnt/e2 /mnt/d` (uma vez; o monitor
  mantém o resto em dia).
- Com o agente em segundo plano (`dag servico instalar`) os trabalhos longos continuam se você
  fechar o terminal, e `dag jobs` mostra o andamento. Sem ele, o comando roda em processo e
  Ctrl-C pausa.
- Dois princípios valem para todos os comandos abaixo:
  1. A **Biblioteca Homologada** (a sua, em `/mnt/e/Livros`) só é lida, nunca escrita, até o
     passo 9, e mesmo ele só escreve depois de duas confirmações suas.
  2. **Nada é apagado nem movido**. As "decisões sugeridas" das duplicatas são só sugestões; liberar
     espaço continua sendo o fluxo da Lixeira (`dag lixeira mover`), com dupla confirmação.
- Qualquer comando aceita `--json` (saída para scripts) logo depois de `dag`:
  `dag --json biblioteca fila`.

### Passo a passo

**1. Registrar a Biblioteca Homologada** (sua biblioteca Calibre, só leitura):

```
dag biblioteca registrar /mnt/e/Livros --papel homologada --nome "Livros"
```

Mostra o id da biblioteca (use-o nos próximos comandos) e o próximo passo.
Para ver o que há dentro (colunas do Calibre, a coluna de estantes, tags, o que não é clonado):

```
dag biblioteca perfil 1
```

**2. Sincronizar** (lê o `metadata.db` em modo somente leitura, liga cada formato ao arquivo do
Catálogo e importa os hashes que o DAG já calculou):

```
dag biblioteca sincronizar 1
```

Ao final aparece a **prova de não alteração**: `PROVA DE NÃO ALTERAÇÃO: OK` quer dizer que o
instantâneo da biblioteca antes e depois da leitura é idêntico. Se aparecer `ATENÇÃO`, o Calibre
provavelmente estava gravando: feche-o e rode de novo. "Formatos sem arquivo no Catálogo" pede um
`dag scan` da pasta; "formatos sem hash" pede o Full Scan.

**3. Ver as duplicatas: "você já tem este livro"**

```
dag biblioteca duplicatas
dag biblioteca espaco
```

`duplicatas` lista, por livro da Homologada, as cópias exatas que estão fora dela: formato, bytes,
onde estão, se têm Cobertura (backup em outro disco) e a decisão `sugerir` ou `bloqueado` com as
razões. O total de bytes é uma **estimativa**. `espaco` resume por Volume: espaço lógico, alocado,
estimado, incerto e efetivamente recuperado. Páginas longas: use o `--cursor` que o comando imprime
(`--limite N` muda o tamanho; `--raiz PASTA` restringe). Os alertas "você já tem este livro" também
aparecem em `dag biblioteca alertas`.

**4. Criar a A Classificar** (a biblioteca Calibre separada onde entram os livros novos, criada
pelo DAG com as mesmas colunas da sua):

```
dag biblioteca registrar /mnt/e2/Livros_A_Classificar --papel a-classificar
```

Se a pasta não for uma biblioteca, o DAG a cria (via `calibredb`) e avisa "criada agora". É a
única biblioteca em que o DAG escreve por conta própria.

**5. Enfileirar os livros a classificar** (barato: não lê o conteúdo):

```
dag biblioteca enfileirar /mnt/e/_classifica
dag biblioteca fila
```

`fila --estado falhou` mostra o que deu erro e por quê.

**6. Processar, com a IA local**

Antes, confira a IA: `dag modelos listar` mostra os perfis configurados e se o Ollama responde
(se não responder: `ollama serve` e `ollama pull gemma3:12b`). Ligue e processe:

```
dag biblioteca config definir ia.ativa=true
dag biblioteca processar --ia
```

Mostra o progresso (itens, páginas, OCR, duplicatas exatas, espaço potencial) e, no fim, o painel
`dag jobs`. **Ctrl-C pausa** com segurança: rode o mesmo comando para continuar de onde parou
(`--limite 50` processa uma rodada pequena para testar). Duplicata exata é reconhecida antes de
qualquer leitura; livros já extraídos e sem mudança são reaproveitados.

**7. Revisar** (extraído × sugestão × homologado parecido, com as evidências e as páginas):

```
dag biblioteca revisar --limite 10
```

`*` marca o campo de baixa confiança. Decida um campo por vez:

```
dag biblioteca decidir --alvo arquivo:123 --campo titulo --aceitar
dag biblioteca decidir --alvo arquivo:123 --campo tags --corrigir "xadrez|finais"
dag biblioteca decidir --alvo arquivo:123 --campo estante --rejeitar
```

ou em lote, por um arquivo JSON (`--lote nome` dá nome ao lote):

```json
{ "lote": "piloto-1",
  "itens": [
    {"alvo": {"tipo": "arquivo", "id": 123}, "campo": "titulo", "acao": "aceitar"},
    {"alvo": {"tipo": "arquivo", "id": 124}, "campo": "tags", "acao": "corrigir", "valor": "xadrez|finais"}
  ] }
```

```
dag biblioteca decidir --de-arquivo decisoes.json
dag biblioteca desfazer piloto-1
```

Decidir só grava no Catálogo; nada é escrito em biblioteca alguma. `desfazer` desfaz o lote inteiro.

**8. Cadastrar na A Classificar**

```
dag biblioteca cadastrar 123 124
```

O resultado mostra o número do cadastro de cada livro (use-o no passo 9). Se o Calibre estiver aberto,
o DAG mostra `calibre_ocupado` com a orientação: **feche o Calibre** e rode o mesmo comando
de novo; o que sobrou ficou pendente e nada é duplicado (o cadastro é idempotente).

**9. Homologar** (o único passo que escreve na sua Homologada):

```
dag biblioteca homologar 12 --aprovado-por "Chico" --motivo "conferi capa e ficha"
```

O comando primeiro só **propõe**, depois mostra **o que vai ser escrito** (biblioteca de destino, o modo
`calibredb`/servidor, quem aprova) e pede duas confirmações:

1. `Escrever este livro na Biblioteca Homologada? [s/N]`: responda `s`.
2. Digite o título do livro (passe-o com `--titulo "Título"`) ou `HOMOLOGAR`.

Qualquer outra resposta cancela e nada é escrito. **Sem terminal** (script, cron) o comando recusa,
a menos que você passe `--sim-eu-aprovo`, que vale como a sua aprovação e dispensa as perguntas.
Feche o Calibre antes (ou configure o modo servidor; veja `dag biblioteca config`). A aprovação e o
que foi escrito ficam registrados no Catálogo.

### Rotina e consulta

| Quero... | Comando |
|---|---|
| Ver o que mudou hoje | `dag biblioteca resumo` (ou `--desde 7d`, `--desde 2026-11-01`) |
| Ver alertas novos | `dag biblioteca alertas` (`--todos` inclui os vistos) |
| Marcar alertas como vistos | `dag biblioteca visto 3 4 5` |
| Listar as bibliotecas | `dag biblioteca listar` |
| Ver/mudar a configuração | `dag biblioteca config` / `dag biblioteca config definir ocr.dpi=200 ia.ativa=true` |
| Reprocessar depois de mudar regras | `dag biblioteca enfileirar PASTA --motivo reprocessar` |

`config definir` aceita chaves aninhadas com ponto (`ia.modelo_primario=gemma3:12b`) e valores
JSON (`true`, `300`, `["por","eng"]`). O endereço da IA precisa ser local (127.0.0.1): o texto dos
livros nunca sai do computador.

### Escolher o modelo de IA: benchmark e conjunto ouro (piloto M5)

```
dag modelos listar
dag biblioteca ouro exportar --n 150 --saida /mnt/e2/ouro --com-texto
```

A pasta de `--saida` precisa existir (`mkdir /mnt/e2/ouro`) e arquivos existentes nunca são
sobrescritos. `exportar` monta uma **amostra estratificada** dos itens em revisão
(`--estratos matematica,xadrez,ficcao,quadrinhos,pdf_textual,pdf_imagem,epub,outros_formatos,duplicata_exata,titulo_coincidente`;
padrão: todos) e grava um JSON por livro (`ouro-000123.json`). Rode **antes** de decidir a
revisão. `--com-texto` lê cada livro (só leitura) e inclui as páginas, para o benchmark enxergar o
texto; sem ele o caso leva só os campos extraídos.

Abra cada JSON e preencha `gabarito` com o que você sabe estar certo
(`"titulo": "..."`, `"autores": ["A", "B"]`, `"tags": [...]`, `"estante": "..."`). `null` quer dizer
"deve ficar desconhecido" (a IA não pode inventar); campo ausente = não conferido; casos com
`gabarito` vazio são ignorados. Em `gabarito_extra`: `duplicata_exata` (`true`/`false`) e, com ids de
arquivo de outros casos, `mesma_edicao_que`, `mesma_obra_que`, `obra_diferente_de`.

```
dag biblioteca ouro avaliar --casos /mnt/e2/ouro
dag modelos benchmark --casos /mnt/e2/ouro --modelo gemma3:12b qwen3:14b --saida benchmark.md
```

`ouro avaliar` compara o gabarito com as sugestões gravadas e mostra, por campo, precisão,
cobertura e abstenções, mais o veredito das metas da especificação: título e autor em alta
confiança >= 95%, tags >= 90% (micro), vínculo obra/edição >= 98%, duplicata exata 100%.
`modelos benchmark` roda os mesmos casos contra os modelos pedidos no Ollama/llama-server local
(tempo, tokens por segundo, VRAM e precisão por campo) e grava o relatório (arquivo novo).

### Erros comuns

| Mensagem | O que fazer |
|---|---|
| `calibre_ocupado` | Feche o Calibre (ou pare o `calibre-server`) e repita o comando; o que faltou ficou pendente. |
| `calibredb_ausente` | Instale o Calibre (`sudo apt install calibre`). |
| `sem_homologada` / `sem_a_classificar` | Registre a biblioteca que falta (passos 1 e 4). |
| `cadastro_nao_pronto` | Rode `dag biblioteca cadastrar` antes de homologar. |
| `ja_existe_na_homologada` | O livro já está na sua biblioteca; veja `dag biblioteca duplicatas`. |
| `aprovacao_humana_exigida` | A homologação só vale com a aprovação de uma pessoa (GUI ou `homologar --aprovado-por`). |
| `config_invalida` | Valor recusado (por exemplo IA fora do loopback); veja `dag biblioteca config`. |
