# Acervo-Digital — DAG

**DAG (Digital Asset Governance)** é um projeto Rust para descobrir, catalogar, acompanhar e futuramente proteger e organizar um acervo digital.

## Estado atual

**Planejamento da Fase A consolidado; ainda não há implementação neste repositório.**

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
