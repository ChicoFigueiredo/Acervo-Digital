# Acervo-Digital — DAG

**DAG (Digital Asset Governance)** é um projeto Rust para descobrir, catalogar, acompanhar e futuramente proteger e organizar um acervo digital.

## Estado atual

**Incrementos 1 e 2 da Fase A (Linux) em andamento.** A CLI `dag` já inventaria o acervo e o explora, sempre em modo somente leitura sobre os arquivos catalogados. O DAG só escreve no Catálogo (SQLite) e no arquivo `<catálogo>.lock` ao lado dele.

O Catálogo fica em `--catalogo <arquivo>`, senão em `$DAG_CATALOGO`, senão em `~/.local/share/dag/catalogo.sqlite` (diretório 0700, banco 0600). Só um processo escreve por vez; um segundo recebe "outro processo do DAG já é o escritor deste catálogo". Leituras (`explorar`, `status`) funcionam durante uma varredura.

```sh
dag volumes                              # volumes montados (e os conhecidos, offline)
dag scan --full ~/Documentos /mnt/hd2    # Full Scan: inventário, tipo por extensão e finalidade por caminho
dag scan --retomar 3                     # retoma a execução 3 (Ctrl-C cancela com limpeza e ela fica retomável)
dag explorar ~/Documentos --limite 20    # filhos por tamanho (lógico e alocado); sem pasta lista as Raízes
dag status                               # últimas varreduras e totais do Catálogo
dag catalogo backup ~/dag-backup.sqlite  # cópia consistente do Catálogo, com verificação de integridade
dag --json explorar ~/Documentos         # qualquer comando em JSON (--json)
```

Ainda não implementados (saem com código 2): Fast Scan, hash e duplicados, marcações, relatório, verificação de integridade, serviço, MCP e pausar/cancelar entre processos. O Full Scan ainda não calcula o Hash Completo nem lê o conteúdo dos arquivos.

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

B: hashes e duplicados; C: backup local/nuvem com restauração; D: acervo inteligente, PDF/OCR, fotos e [Calibre](https://github.com/ChicoFigueiredo/calibre-port); E: integrações ampliadas.

## Referências

- [ScanFileProGo](https://github.com/ChicoFigueiredo/ScanFileProGo) — inspiração funcional (não alterar).
- [Bancada](https://github.com/ChicoFigueiredo/bancada) — interface e SQLite.
- [calibre-port](https://github.com/ChicoFigueiredo/calibre-port) — integração futura, sem escrita direta no banco do Calibre.

> O repositório é público: não commitar credenciais, caminhos privados do acervo, segredos nem dados biométricos reais.
