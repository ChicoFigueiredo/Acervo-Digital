# Regras para agentes neste repositório

- **Limpeza após cada build:** ao terminar um build, teste ou clippy, rode `cargo clean` (ou apague `target/`). O diretório `target/` cresce vários GiB e consome espaço demais. Isso vale também para worktrees e `CARGO_TARGET_DIR` alternativos.
- O DAG nunca apaga nem altera arquivos catalogados. A única escrita permitida é mover uma cópia com Marcação para Deleção para a `DAG-Lixeira` do próprio Volume e de volta (ADR 0005).
- Contêineres (compactados e imagens de disco) são lidos em fluxo e na memória: nada é extraído, montado ou gravado em disco, nem em temporário (ADR 0006). O MCP nunca recebe senha de volta (só fornece).
- Vocabulário do domínio em `CONTEXT.md`; decisões em `docs/adr/`; requisitos e critérios de aceite em `docs/FASE-A-REQUISITOS.md`.
- Antes de concluir: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, e então `cargo clean`.
