# Critérios para surpreender o cliente — Fase B

São diferenciais de experiência; não substituem segurança ou critérios de aceite. Cada um tem demonstração verificável.

| ID | Experiência desejada | Demonstração / prova |
|---|---|---|
| WOW-01 | Ao baixar um livro já homologado, alerta em segundos após arquivo estabilizar: "Você já tem este livro", mostrando capa, título, edição, formato, hash e localizações. | Demonstração com download novo, cópia exata e arquivo alterado; sem alerta falso de duplicata exata. |
| WOW-02 | "Economize espaço com segurança": painel ordenado por bytes recuperáveis, com prévia do que será mantido, onde está o backup e por que cada item é redundante. | Simulação sem mover nada; validação manual dos dez maiores grupos. |
| WOW-03 | "Bibliotecário que aprende comigo": ao corrigir autor/tag/estante, próximas sugestões seguem o padrão pessoal, e o usuário vê exemplos de decisões anteriores usadas. | Antes/depois com conjunto cego e histórico de explicações. |
| WOW-04 | Livro sem nome útil, escaneado: DAG encontra capa, índice e ficha, sugere título/autor/edição com página-fonte e pede revisão quando incerto. | Demonstração com PDFs digitalizados e baixa qualidade. |
| WOW-05 | "Uma obra, muitos formatos": exibe PDF, EPUB, MOBI e outras versões por edição, sem perder traduções nem volumes. | Demonstração com obra de múltiplas edições e formatos. |
| WOW-06 | "Estante automática": sugere a estante exata existente na biblioteca do usuário, além de tags finas para cálculo, álgebra, xadrez, HQ etc. | Comparação com exemplos homologados; nenhuma nova estante silenciosa. |
| WOW-07 | "Trabalho noturno": processa fila com consumo de recursos ajustável, pausa e retoma após queda, mostra progresso e economia potencial ao voltar. | Encerrar agente durante OCR e retomar sem repetir trabalho concluído. |
| WOW-08 | "Por que você classificou assim?": painel de proveniência com trechos do índice, páginas, metadados e regras, permitindo corrigir sem abrir vários aplicativos. | Revisar cinco livros apenas pela interface do DAG. |
| WOW-09 | "Meu acervo está protegido?": indicador de cobertura de backup e teste de restauração antes de qualquer sugestão executável de descarte. | Simular disco desconectado e backup ausente, observando bloqueio. |
| WOW-10 | "Novidades da biblioteca": resumo diário opcional de livros novos, duplicatas, itens classificados e revisões pendentes, sem notificações excessivas. | Gerar resumo a partir de eventos reais, com opção de silenciar. |

## Condição de sucesso
No piloto, demonstrar ao menos WOW-01, WOW-03, WOW-04, WOW-05 e WOW-08, além de todos os portões de segurança aplicáveis. Os demais podem ser entregues incrementalmente com registro explícito do estado.
