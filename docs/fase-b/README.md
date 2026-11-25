# Fase B — Curadoria Bibliográfica Inteligente
Status: especificação para implementação; não é declaração de funcionalidade entregue.

## Objetivo
Transformar o inventário existente do DAG em biblioteca digital inteligente, preservando a biblioteca homologada do Calibre e criando biblioteca independente **A Classificar**. Priorizar arquivos grandes, duplicidades e ganho seguro de espaço, sem perder edições, traduções, anotações, metadados ou obras únicas.

## Regras inegociáveis
1. Biblioteca homologada é fonte de referência, somente leitura para o DAG até aprovação explícita; jamais escrever diretamente em metadata.db.
2. Ler o código e documentação do Calibre upstream (https://github.com/kovidgoyal/calibre), CLI calibredb/ebook-meta, esquema e ferramentas, e comparar com https://github.com/ChicoFigueiredo/calibre-port. Verificar comportamento da versão instalada antes de escolher APIs.
3. Inspecionar **a biblioteca real do usuário**, com permissão e acesso local, antes de fixar taxonomia: autores, autor_sort, título, title_sort, tags, séries, editoras, identificadores, idiomas, formatos, comentários, capas, campos personalizados, bibliotecas virtuais e 'estantes' conforme sua representação efetiva. Não presumir que estante equivale a tag.
4. Sem exclusão automática. Lixeira só com confirmação explícita, verificação de cópia íntegra e recuperação; nunca tratar coincidência de título como duplicidade descartável.
5. Operar no Linux primeiro, respeitar arquitetura multiplataforma; preservar agente escritor único, SQLite, CLI, GUI, MCP, filas, checkpoints e monitoramento.
6. Novos arquivos bibliográficos detectados pelo watcher entram automaticamente na fila; alerta imediato de hash idêntico a exemplar homologado, sem necessidade de OCR.
7. Distinguir arquivo físico, formato, manifestação/edição, obra e série; preservar traduções, revisões, volumes e edições.

## Entregáveis documentais
- [Requisitos e histórias](REQUISITOS.md)
- [Aceite e testes](CRITERIOS-ACEITE.md)
- [Critérios para surpreender o cliente](CRITERIOS-SURPRESA.md)
- [Arquitetura e execução](ARQUITETURA-PLANO.md)

## Fora de escopo inicial
Treinar modelos do zero; remover DRM; mover ou excluir sem autorização; alterar diretamente o banco do Calibre; backup remoto como pré-requisito para catalogar. Backup verificado é pré-requisito para descarte definitivo conforme política aprovada.
