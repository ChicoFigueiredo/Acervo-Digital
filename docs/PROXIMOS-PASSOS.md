# Próximos passos (depois da Fase A)

Pendências registradas ao fechar o PR #1. Este arquivo é o ponto de partida do PR #2.

- **Memória do agente:** confirmar em campo, pelas linhas `agente: memória após <etapa>` do journal, qual etapa cresce; o pico de 28,7 GB não foi reproduzido em teste.
- **Clones de pasta (A-32):** decisão de escopo ainda em aberto.
- **Redesenho da UX da Bancada:** plano proposto, aguardando escolha.
- **Lixeira com Polkit:** auxiliar para volumes sem permissão de escrita do usuário.
- **Imagens de disco reais:** testar VHDX, NTFS e UDF com arquivos reais.
- **Ordenação das tabelas paginadas:** hoje ordena só a página carregada; levar a ordenação para a consulta.
- **Validação local:** sem CI; antes de cada merge rodar `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` e `cargo clean`.
