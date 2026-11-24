# Acervo-Digital — DAG

**DAG (Digital Asset Governance)** é um projeto Rust para descobrir, catalogar, acompanhar e futuramente proteger e organizar um acervo digital.

## Estado atual

**Fase A (Linux) implementada nos seis incrementos**, sempre em modo **somente leitura sobre os arquivos catalogados**: o DAG só escreve no Catálogo (SQLite), no arquivo `<catálogo>.lock` ao lado dele, no socket do agente e em relatórios/backups que você pedir. Nenhum comando da CLI, da GUI ou do MCP apaga, move, renomeia ou altera arquivo; as Marcações (Backup, Deleção) vivem só no Catálogo. O estado de cada critério de aceite, com os testes que o provam e o que continua manual, está em [docs/ACEITE-FASE-A.md](docs/ACEITE-FASE-A.md).

### O que já existe

- **Inventário (Full Scan):** volumes e dispositivos físicos, pastas e arquivos em tabelas separadas, tamanho lógico e alocado (NULL quando o sistema não informa), datas, symlinks (nunca seguidos), hard links, bind mounts sem laço, exclusões justificadas, tipo/MIME por extensão e finalidade (sistema, aplicativo, usuário, compartilhado, misto, desconhecido) com regra e confiança. Pausa, cancelamento e retomada após queda.
- **Hash Completo BLAKE3** de todo arquivo regular, em etapa própria com fila persistente, limite de I/O e retomada sem refazer o que já foi lido; hard links são lidos uma vez.
- **Duplicados:** Grupos por (tamanho, hash) entre pastas, volumes e dispositivos (inclusive volumes desconectados), Cópia Principal por Prioridade de Volume ou escolha manual, Marcação de Backup e para Deleção (por arquivo, grupo, pasta ou Volume), Cobertura em outro Dispositivo Físico, alerta de Única Cópia, Marcação Pendente quando o conteúdo muda, histórico com autor (GUI, CLI ou MCP) e espaço recuperável por grupo, pasta, volume e computador. Pastas clones (A-32) continuam **pendentes de decisão**.
- **Fast Scan** que reaproveita metadados e hash do que não mudou, trata renomeações e remoções sem confundir com volume desconectado, e **monitoramento** por inotify com lacunas (overflow) registradas e reconciliadas. **Verificação de Integridade** relê o conteúdo e aponta divergência sem alterar o arquivo.
- **Interfaces sobre o mesmo núcleo:** CLI `dag` (saída legível e `--json`), GUI `dag-app` (Bancada: Início, Discos, Varredura, Explorador, Análise com Tree View e Treemap, Duplicados, Monitoramento, Configurações) e servidor **MCP** (`dag mcp`, [docs/MCP.md](docs/MCP.md)). Um único escritor por Catálogo: com o agente/serviço ativo, GUI, CLI e MCP falam com ele pelo socket local.
- **Serviço opcional** (`dag servico instalar` recomendado: habilita e inicia, entrega o Catálogo ao serviço sem interromper o hash em curso, `--iniciar-no-boot` liga o linger; `dag servico ...`, systemd de usuário, três modos: não instalado, sem autostart, com autostart) e **elevação sob demanda** (`dag scan --elevar`: Polkit + `dag-helper`, que lê só metadados; a senha só no diálogo do sistema). Veja [packaging/README.md](packaging/README.md).
- **Backup consistente do Catálogo** (`dag catalogo backup`) com verificação de integridade; esquema versionado por migrações ([crates/dag-store/ESQUEMA.md](crates/dag-store/ESQUEMA.md), versão 3).
- **Escala:** ensaio de 1 milhão de entradas com hash e duplicados em modo release (números em [docs/ACEITE-FASE-A.md](docs/ACEITE-FASE-A.md)).
- **Fronteira de plataforma:** a trait `Plataforma` (`dag-core`) isola o que depende do SO; só `PlataformaLinux` existe. Windows e macOS são apenas contrato.

### Início rápido

```sh
cargo build --release                    # gera target/release/dag (CLI) e target/release/dag-app (GUI)
export DAG_CATALOGO=~/dag/catalogo.sqlite  # opcional; padrão ~/.local/share/dag/catalogo.sqlite (0700/0600)

dag volumes                              # volumes reais (físicos, removíveis, rede); oculta snap/loop/zram
dag volumes --todos                      # inclui imagem/virtuais, com a coluna CLASSE
dag scan --full ~/Documentos /mnt/hd2    # Full Scan: inventário + Hash Completo + duplicados
dag duplicados --limite 20               # Grupos de Duplicados, maior espaço recuperável primeiro
dag grupo 7                              # cópias, Marcações e Cobertura do grupo 7
dag prioridade SSD HD-Externo            # Cópia Principal: SSD vence o HD-Externo
dag marcar backup --volume HD-Externo    # regra: o HD-Externo é backup (só no Catálogo)
dag marcar delecao --grupo 7             # candidatas a descarte (a principal é poupada; nada é apagado)
dag relatorio espaco --nivel volume      # espaço recuperável; também cobertura, sem-cobertura, marcacoes
dag relatorio marcacoes --saida marcacoes.csv   # exporta (.csv ou .json), sem sobrescrever
dag scan --fast                          # reconcilia o que mudou (todas as raízes já varridas)
dag verificar                            # Verificação de Integridade (releitura e comparação de hash)
dag explorar ~/Documentos --limite 20    # filhos por tamanho (lógico e alocado)
dag status                               # execuções e totais; na etapa de hash mostra arquivos/bytes feitos e pendentes por disco
                                         # dag pausar | retomar | cancelar
dag catalogo backup ~/dag-backup.sqlite  # cópia consistente do Catálogo
dag --json duplicados                    # qualquer comando em JSON
dag-app                                  # GUI
```

Segundo plano e integrações:

```sh
dag servico instalar && dag servico iniciar     # opt-in; `dag servico autostart on`, `linger on`, `remover`
dag agente                                       # o agente em primeiro plano (a unidade systemd usa isto)
dag monitorar ~/Documentos                       # inotify + reconciliação, até Ctrl-C
dag scan --full --elevar /mnt/dados             # pastas sem permissão: pede o Polkit (metadados apenas)
claude mcp add dag -- dag mcp                    # registra o MCP no Claude Code (docs/MCP.md)
```

O Catálogo fica em `--catalogo <arquivo>`, senão em `$DAG_CATALOGO`, senão em `~/.local/share/dag/catalogo.sqlite`. O escritor único é o agente: CLI, GUI e MCP o lançam sozinhos se não houver um (`--sem-agente` ou `DAG_SEM_AGENTE=1` mantém o modo antigo em processo; o agente lançado fica fora do escopo da janela: unidade systemd instalada, senão `systemd-run --user`, senão `setsid`; fechar a GUI nunca para os trabalhos, e para tê-lo no ar desde o login use `dag servico instalar && dag servico autostart on`, veja [packaging/README.md](packaging/README.md)) e qualquer cliente vê e controla os trabalhos dos outros: `dag jobs`, `dag status --seguir` (ou `dag acompanhar`), `dag pausar|retomar|cancelar --job N` ([ADR 0004](docs/adr/0004-agente-escritor-unico-e-assinatura-de-eventos.md)). Ao atualizar, feche a GUI antiga antes (ela segura a trava); o agente retoma o hash interrompido. Só um processo escreve por vez; um segundo recebe "outro processo do DAG já é o escritor deste catálogo". Leituras (`explorar`, `status`, `duplicados`, a GUI) funcionam durante uma varredura.

O Hash Completo lê **um disco físico por thread, todos os discos ao mesmo tempo** (volumes do mesmo disco ficam em sequência). Se o processo morrer ou a execução for cancelada no meio do hash, `dag scan --retomar <id>` continua só o que falta: a fila é refeita a partir dos hashes já gravados, sem reler nada e sem mudar o Catálogo.

**Paralelismo do hash (configurável):** por padrão 1 leitor por disco físico (o melhor para HDD) e até 16 discos ao mesmo tempo. Defina o padrão persistido no Catálogo com `dag config hash --threads-por-disco N --max-discos M` (sem flags, só mostra); vale para todo Hash Completo e Verificação de Integridade do agente, do serviço e da GUI (Configurações, "Threads de hash por disco" 1–8 e "Discos em paralelo" 1–16; SSD/NVMe podem ganhar com 2–4). Sobrescreva por execução com `dag scan ... --threads-por-disco N --max-discos M` (também em `dag verificar` e `dag scan --retomar`) ou por agente com `dag agente --threads-por-disco N --max-discos M`. Precedência: pedido > flag do agente > persistido > padrão; fora da faixa (1..=8, 1..=16) dá erro. A mudança vale a partir da próxima execução ou retomada; uma execução em andamento mantém o valor com que começou (pause/cancele e retome para aplicar).

**Queda no meio da varredura (falta de energia, travamento):** ao abrir, o agente (`dag agente`) e a GUI detectam a execução deixada pela metade, marcam-na `interrompida` (`dag status` mostra "interrompida, retomando") e a retomam de onde parou, sem reler o que já foi hasheado. Depois repetem o Fast Scan nas mesmas raízes (até 3 passadas) para pegar arquivos que chegaram enquanto o programa estava fora; com o monitor ativo, a raiz que estabiliza entra no **Ponto de Confiança** (`dag status` lista), a partir do qual só o monitoramento a mantém em dia (lacuna do monitor ou queda do processo a tira). Ao abrir, toda raiz também é conferida por Fast Scan e só o que é novo ou mudou recebe hash; o Full Scan que relê tudo é `dag scan --full`. A abertura é uma execução como as outras: a GUI mostra "Retomando execução #N (hash, X%)" na Varredura e na barra de status e Pausar/Cancelar funcionam (a ordem é: 1) retomar o hash pendente; 2) passadas de verificação). Execução que terminou com o hash pela metade (fase `hash`) é detectada mesmo sem fila persistente e a fila é refeita; só cancelamento deliberado do hash impede a retomada automática. Opções do agente: `--sem-retomar-automatico`, `--verificar-ao-abrir sempre|nao-confiaveis|nunca`, `--max-passadas N`. Decisão: [ADR 0003](docs/adr/0003-retomada-automatica-e-ponto-de-confianca.md).

### Limitações conhecidas

Assinatura de conteúdo (magic bytes) existe em `dag-core` mas ainda não é usada pelo scan (tipo só por extensão); pastas clones (A-32) pendentes de decisão; Windows/macOS só como contrato; janela real da GUI, systemd real, diálogo Polkit real e NFS real não são exercitados pelos testes automáticos (ver o documento de aceite).

**Leia a especificação principal:** [Fase A — Especificação de produto e engenharia](docs/FASE-A-ESPECIFICACAO.md).

## Fase A — Linux primeiro

- GUI nativa em Rust com Ribbon da [Bancada](https://github.com/ChicoFigueiredo/bancada) (egui/eframe).
- SQLite local, arquivos e pastas em tabelas separadas; navegação sem árvore completa na memória.
- Full Scan por etapas e Fast Scan de reconciliação.
- Tamanho lógico versus alocado, tipos MIME, classificações de sistema/aplicativos/usuário/compartilhados/mistos.
- Tree View e Treemap com análise por HD ou computador.
- Monitoramento contínuo enquanto aplicativo aberto; serviço independente **opcional** instalado e configurado pela GUI.
- Escalonamento de permissões apenas quando necessário, no Linux via Polkit e helper restrito.
- **Somente leitura dos arquivos catalogados. Sem exclusões ou movimentações.**
- Arquitetura preparada para Windows/macOS.

## Próximas fases

B: (absorvida pela Fase A, ADR 0001); C: backup local/nuvem com restauração; D: acervo inteligente, PDF/OCR, fotos e [Calibre](https://github.com/ChicoFigueiredo/calibre-port); E: integrações ampliadas.

## Referências

- [ScanFileProGo](https://github.com/ChicoFigueiredo/ScanFileProGo) — inspiração funcional (não alterar).
- [Bancada](https://github.com/ChicoFigueiredo/bancada) — interface e SQLite.
- [calibre-port](https://github.com/ChicoFigueiredo/calibre-port) — integração futura, sem escrita direta no banco do Calibre.

> O repositório é público: não commitar credenciais, caminhos privados do acervo, segredos nem dados biométricos reais.
