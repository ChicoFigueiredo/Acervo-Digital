# Regras para agentes neste repositório

- **Limpeza após cada build:** ao terminar um build, teste ou clippy, rode `cargo clean` (ou apague `target/`). O diretório `target/` cresce vários GiB e consome espaço demais. Isso vale também para worktrees e `CARGO_TARGET_DIR` alternativos.
- O DAG é somente leitura para os arquivos catalogados: nenhum código pode apagar, mover, renomear ou alterar arquivos varridos.
- Vocabulário do domínio em `CONTEXT.md`; decisões em `docs/adr/`; requisitos e critérios de aceite em `docs/FASE-A-REQUISITOS.md`.
- Antes de concluir: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, e então `cargo clean`.
