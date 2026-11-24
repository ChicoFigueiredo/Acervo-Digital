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
- **Serviço opcional** (`dag servico ...`, systemd de usuário, três modos: não instalado, sem autostart, com autostart) e **elevação sob demanda** (`dag scan --elevar`: Polkit + `dag-helper`, que lê só metadados; a senha só no diálogo do sistema). Veja [packaging/README.md](packaging/README.md).
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
dag status                               # execuções e totais; dag pausar | retomar | cancelar
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

O Catálogo fica em `--catalogo <arquivo>`, senão em `$DAG_CATALOGO`, senão em `~/.local/share/dag/catalogo.sqlite`. Só um processo escreve por vez; um segundo recebe "outro processo do DAG já é o escritor deste catálogo". Leituras (`explorar`, `status`, `duplicados`, a GUI) funcionam durante uma varredura.

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
