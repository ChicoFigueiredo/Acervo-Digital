# DAG — Digital Asset Governance
## Especificação de produto e engenharia — Fase A (Linux)

**Estado:** decisões de produto consolidadas, implementação ainda não iniciada neste repositório  
**Data da consolidação:** 23/11/2026  
**Versão da especificação:** 1.0  
**Plataforma inicial:** Linux desktop; arquitetura preparada para Windows e macOS  
**Stack definida:** Rust, Bancada (bancada-casca/egui/eframe), SQLite por bancada-armazem

> Documento-fonte das decisões desta fase. Não constitui declaração de implementação, de aprovação dos testes ou de equivalência com projetos de referência. Não autoriza exclusão ou alteração de arquivos do usuário.

### 1. Contexto, problema e objetivo

O DAG será uma plataforma local de inventário, governança e organização do acervo digital: discos, pastas, arquivos, fotos, imagens, livros, PDFs, gibis, documentos e, futuramente, armazenamento em nuvem. O problema imediato é descobrir onde o espaço dos HDs/SSDs está sendo consumido e manter esse retrato atualizado com segurança.

A Fase A entregará **um analisador de espaço em disco realmente utilizável**, com catálogo persistente em SQLite, Full Scan por etapas, Fast Scan de reconciliação, monitoramento de alterações em execução, serviço de segundo plano **opcional**, classificação de arquivos e pastas e interface Ribbon com árvore e Treemap.

**Princípios inegociáveis:** inventário somente para leitura; nenhuma alteração em arquivos originais; nenhuma exclusão/movimentação; progresso e erros visíveis; milhões de registros sem árvore inteira na RAM; bancos locais íntegros; elevação administrativa mínima, específica e auditável.

### 2. Fontes de inspiração e limites

- **ScanFileProGo:** https://github.com/ChicoFigueiredo/ScanFileProGo — referência de produto para varredura, análise de tamanho lógico/alocado, exclusões justificadas, árvore, Treemap, monitoramento, Quick Scan e, nas fases futuras, filtros de tamanho/pré-hash/hash. **Não modificar nem portar cegamente o código Go.**
- **Bancada:** https://github.com/ChicoFigueiredo/bancada — base Rust existente: **bancada-casca** (Ribbon, egui/eframe, navegação, dock, paleta, temas) e **bancada-armazem** (SQLite, migrações, transações, backup do banco); avaliar APIs reais antes de implementar adaptadores.
- **calibre-port:** https://github.com/ChicoFigueiredo/calibre-port — referência de integração futura. Respeitar acesso coordenado à biblioteca; nunca escrever diretamente no metadata.db do Calibre enquanto outro processo o utiliza.

O aplicativo novo não deverá modificar nenhum dos três repositórios de referência por padrão.

### 3. Delimitação das fases

| Fase | Entrega | Status |
| --- | --- | --- |
| **A — Inventário e observabilidade** | Aplicação Linux Rust/Bancada, SQLite, descoberta, tamanhos, classificação, Full/Fast Scan, worker, serviço opcional, árvore, Treemap e privilégios sob demanda | **Escopo definido neste documento** |
| **B — Integridade e duplicados** | Filtros de tamanho, pré-hash, BLAKE3/SHA-256, arquivos idênticos, pastas clones, relatórios de espaço recuperável | Posterior |
| **C — Backup e proteção** | Backup local/nuvem gerenciado, políticas, criptografia, versões, retenção, reconciliação, restauração testada e sugestões da IA | Posterior |
| **D — Acervo inteligente e Calibre** | Fotos/EXIF, agrupamento facial supervisionado, PDF texto ou digitalizado, OCR das 10–15 páginas iniciais, metadados, catálogo de pré-curadoria e importação aprovada no Calibre | Posterior |
| **E — Expansão** | Fontes remotas e outras nuvens, catálogos independentes, monitoramentos adicionais, capacidades mais amplas de IA | Posterior |

Preparar interfaces de extensão para B–E, mas **não apresentar seus recursos como entregues na Fase A**.

### 4. Requisitos funcionais da Fase A

| ID | Prioridade | Requisito e comportamento obrigatório |
| --- | --- | --- |
| A-01 | Obrigatório | Detectar volumes, partições lógicas, sistemas de arquivos, capacidades, espaço livre, pontos de montagem e dispositivos removíveis quando disponíveis. |
| A-02 | Obrigatório | Escolher todos os volumes locais ou subconjunto de raízes, com inclusões e exclusões explícitas. |
| A-03 | Obrigatório | Full Scan por etapas com estados persistidos: descoberta de volumes/diretórios, inventário de arquivos e metadados, classificação e agregação. |
| A-04 | Obrigatório | Armazenar pastas e arquivos em **tabelas separadas**, com identidade e vínculo hierárquico; não depender de árvore total na RAM. |
| A-05 | Obrigatório | Registrar tamanho lógico, tamanho alocado quando consultável, datas, atributos, dispositivo, tipo de entrada e fonte dos dados; valor indisponível não deve ser inventado. |
| A-06 | Obrigatório | Tratar links simbólicos, junções, bind mounts, pontos de montagem, hard links, laços, permissões e pseudo-sistemas sem ciclos nem contagem indevida. |
| A-07 | Obrigatório | Identificar extensão, MIME padronizado, nome descritivo e categoria geral do formato; distinguir tipo inferido por extensão de tipo validado por assinatura. |
| A-08 | Obrigatório | Classificar **arquivos e pastas** por finalidade: sistema, aplicativo, usuário, compartilhado, misto e desconhecido; registrar regra e confiança. |
| A-09 | Obrigatório | Calcular totais hierárquicos de itens, bytes lógicos e bytes alocados por pasta/volume e por categoria; indicar estimativas ou somas potencialmente compartilhadas. |
| A-10 | Obrigatório | Tree View com barras proporcionais ao consumo; navegar, ordenar, filtrar e expandir com consultas paginadas. |
| A-11 | Obrigatório | Treemap interativo com drill-down e cores por tipo, finalidade, idade ou profundidade; percentuais por raiz/volume/computador. |
| A-12 | Obrigatório | Fast Scan para reconciliar adição, alteração, renomeação e remoção sem recalcular metadados de itens comprovadamente inalterados. |
| A-13 | Obrigatório | Worker Rust persistente e monitor de eventos para atualizar SQLite enquanto o aplicativo estiver aberto; operação independente da GUI quando serviço opcional estiver ativo. |
| A-14 | Obrigatório | Serviço Linux **opt-in** configurável na própria Bancada: não instalado; instalado sem inicialização automática; instalado com inicialização automática. |
| A-15 | Obrigatório | Progresso, filas, velocidade, itens processados, erros, itens ignorados, volumes conectados/desconectados, saúde do monitor e estado do serviço. |
| A-16 | Obrigatório | Acesso administrativo **sob demanda** no Linux via Polkit e helper restrito; a GUI e o banco não rodam como root. |
| A-17 | Obrigatório | Pausar, cancelar e retomar scans de forma consistente, com checkpoints, idempotência e recuperação após encerramento abrupto. |
| A-18 | Obrigatório | Detectar desconexão de volumes e marcar indisponibilidade sem inferir remoção em massa. |
| A-19 | Obrigatório | Persistir histórico de varreduras, alterações, erros de acesso, justificativas de exclusões, métricas e timestamps. |
| A-20 | Obrigatório | Backup consistente do catálogo SQLite e recuperação verificada, sem tratar snapshot do inventário como backup dos próprios arquivos. |
| A-21 | Obrigatório | Configurações de limites de I/O, concorrência, periodicidade da reconciliação e política por volume, com padrões conservadores. |
| A-22 | Obrigatório | Preparar contratos/adaptadores para Windows e macOS sem exigir implementação integral nessas plataformas na Fase A. |

### 5. Comportamento do scan

**Full Scan — pipeline lógico:**
1. Descobrir dispositivos, volumes e seus pontos de montagem; associar identidade estável e estado conectado/desconectado.
2. Descobrir a estrutura de diretórios e registrar hierarquia sem seguir links de forma recursiva por padrão.
3. Registrar arquivos e metadados; capturar tamanho lógico e tamanho realmente alocado quando suportado.
4. Classificar extensão/MIME/finalidade; identificação por assinatura é uma verificação leve e opcional quando exigir leitura de conteúdo, sem impedir o inventário.
5. Consolidar estatísticas hierárquicas; publicar status completo e registrar checkpoint.

**Otimização:** as etapas 2 e 3 podem compartilhar a mesma visita ao sistema de arquivos, mas manter indicadores/estados independentes. Não criar uma segunda leitura desnecessária só para satisfazer separação conceitual.

**Fast Scan:** visitar o escopo definido e comparar identidade de entrada, caminho, tipo, tamanho, mtime e demais metadados adequados ao sistema de arquivos. Reaproveitar dados comprovadamente estáveis; atualizar itens novos/mudados, registrar renomeações quando houver evidência e marcar remoções **somente** após reconciliação concluída com sucesso para uma raiz disponível. Eventos de watcher não são fonte de verdade suficiente.

**Sem loops e sem contagem dupla:** links simbólicos são registrados como entradas e não seguidos por padrão; mounts de volumes distintos têm política explícita; nós visitados são identificados por (dispositivo, inode/ID nativo), quando possível. Hard links podem aparecer como entradas de caminho separadas, mas a métrica de uso físico único deve deduplicar a identidade do conteúdo físico no mesmo sistema de arquivos. As somas lógicas por pasta e os bytes físicos exclusivos são métricas diferentes.

**Tamanho de pasta:** representa somatória dos descendentes, não uma “capacidade da pasta”. Exibir livre/capacidade do volume associado e quotas apenas quando disponíveis. Arquivos esparsos, compactados, reflinks/COW e snapshots podem impedir um cálculo exato da economia recuperável: indicar as limitações.

**Escopo Linux:** não percorrer automaticamente /proc, /sys, /dev e outros sistemas de arquivos virtuais. Não ignorar pastas reais com esses mesmos nomes fora do contexto relevante. Erros de permissão, ciclos e exclusões devem ser contabilizados e justificados, não silenciosos.

### 6. Arquitetura executável e processos

Camadas propostas:

- **dag-app:** GUI Rust que implementa contratos da bancada-casca; usa acesso de leitura ao catálogo e envia comandos ao worker.
- **dag-core:** domínio de volumes, entradas, classificação, sessões, métricas, políticas e comandos, sem dependência de plataforma.
- **dag-store:** SQLite via bancada-armazem, migrations versionadas, índices, consultas paginadas, backup e política de acesso.
- **dag-scanner:** pipeline cooperativo de Full/Fast Scan com filas limitadas, checkpoints e cancelamento.
- **dag-monitor:** watcher Linux (inotify ou biblioteca equivalente) + fila limitada/coalescência + reconciliação periódica.
- **dag-agent:** processo worker proprietário de varredura, monitoração e **único escritor** do SQLite; pode rodar enquanto a GUI está aberta ou via serviço instalado opcionalmente.
- **dag-platform-linux:** volumes, mountinfo, inode/metadados, estatísticas de alocação, monitoramento e integração systemd/Polkit.
- **dag-privileged-helper:** interface de leitura estritamente autorizada, com IPC local autenticado, validação de raízes e auditoria. Não aceita comandos arbitrários do shell.
- **dag-platform-windows/mac:** contratos e implementações de compatibilidade futuras (UAC/serviço, autorização macOS), sem acoplamento do domínio.

**Invariante de concorrência:** uma instância do worker é a dona da escrita do SQLite e dos watchers. Se a GUI abrir enquanto o serviço estiver ativo, deverá se conectar a ele, não iniciar um segundo escritor. SQLite usa WAL para leituras concorrentes e transações curtas; o arquivo de banco permanece em armazenamento local, não em NFS/SMB compartilhado.

**Exibição escalável:** árvore com lazy loading, paginação, agregados pré-computados e consultas por índice; Treemap consulta somente níveis e itens relevantes. Nenhum painel requer carregar milhões de caminhos simultaneamente.

### 7. Serviço em segundo plano: opção explícita do usuário

**Decisão de produto (prevalece sobre sugestões anteriores): a instalação e o início automático do serviço são configuráveis; NÃO há instalação nem ativação silenciosa.**

- **Modo 1 — Sem serviço instalado (estado inicial):** aplicativo abre e inicia monitoramento durante seu uso; fechar a GUI encerra o worker local após checkpoint.
- **Modo 2 — Serviço instalado, início automático desativado:** painel permite iniciar/parar o serviço manualmente; quando parado, a GUI pode usar o worker de sessão; não duplicar processamento.
- **Modo 3 — Serviço instalado, início automático ativado:** worker sobe conforme política selecionada (início da sessão do usuário ou serviço de sistema com permissões apropriadas) e persiste quando a GUI é fechada.

A UI em **Configurações > Monitoramento** deve exibir: instalado ou não, habilitado ou não, em execução/parado/erro, último evento, última reconciliação, backlog, erros, volumes monitorados, recursos consumidos; ações instalar, remover, ativar início automático, desativar início automático, iniciar, parar e reiniciar.

No Linux, usar systemd; preferir **serviço de usuário** quando suficiente. Não confundir serviço de usuário com serviço que continua após logout/reboot sem sessão: explicitar a diferença e, se necessário, configurar linger ou serviço de sistema de modo consciente e autorizado. Nunca presumir que todos os desktops oferecem Polkit agent ativo; oferecer diagnóstico e instrução para alternativas autorizadas.

O monitoramento em sessão pode estar ativo por padrão quando a GUI abre; isso **não** implica instalar serviço automaticamente. Desabilitar autostart também **não** desabilita monitoramento durante a sessão.

### 8. Modelo SQLite mínimo proposto

Nomes e colunas são ponto de partida para migrações versionadas; a equipe deverá documentar cardinalidade, chaves, índices e regras de consistência antes de finalizar.

| Tabela | Campos/relacionamentos principais |
| --- | --- |
| **volumes** | id, platform_id, fs_type, capacity_bytes, free_bytes, mounted, last_seen_at, flags |
| **mount_points** | id, volume_id, path_raw, path_display, mounted_at, scope_policy |
| **folders** | id, volume_id, parent_folder_id, path_raw, name_display, native_file_id, class_id, classification_confidence, attrs, mtime, seen_run_id, state |
| **files** | id, volume_id, folder_id, path_raw, name_display, native_file_id, hardlink_group, logical_bytes, allocated_bytes nullable, mtime, ctime, birthtime nullable, attrs, mime_id, class_id, seen_run_id, state |
| **file_types** | id, extension, mime, human_description, broad_category, evidence_source, confidence |
| **classifications** | id, category, scope, rule_id, confidence, classified_at |
| **folder_stats** | folder_id, child_count, recursive_file_count, recursive_folder_count, logical_bytes, allocated_entry_bytes nullable, physical_unique_bytes nullable, aggregate_state, updated_at |
| **scan_runs** | id, mode, roots, started_at, ended_at, status, phase, counters, checkpoint, config_version |
| **scan_queue** | id, run_id, directory_id/path, phase, attempt_count, lease_state, last_error, scheduled_at |
| **scan_events** | id, volume_id, event_type, source, path, file_id, observed_at, processed_at, reconcile_required |
| **scan_errors** | id, run_id, path, phase, error_code, reason, observed_at, severity |
| **scan_exclusions** | id, root/scope, matcher, reason, enabled |
| **service_state** | id, desired_mode, current_mode, last_heartbeat, last_reconcile, error |
| **catalog_snapshots** | id, created_at, backup_path, integrity_result, scope |

**Pontos de atenção:**
- **Linux:** nomes/caminhos podem conter bytes que não são UTF-8. Conservar representação bruta sem perdas e uma versão escapada segura para exibição.
- **Identidade:** inode pode ser reutilizado; combinar evidências e geração/tempo/estado quando adequado, sem tratar (dev,inode) como identidade imutável universal.
- **Nullability:** tamanho alocado, birthtime e classificação podem não estar disponíveis; não preencher zero artificialmente.
- **Monitoramento:** usar journal/transações idempotentes; deduplicar rajadas de eventos; registrar necessidade de reconciliação após overflow.
- **Backup do SQLite:** usar API SQLite de backup ou checkpoint seguro, nunca copiar um arquivo WAL ativo de forma inconsistente.

### 9. Interface Rust com Bancada

**Ribbon e navegação:**
- Abas: **Início**, **Discos**, **Varredura**, **Explorador**, **Análise**, **Monitoramento**, **Configurações**.
- Ações da varredura: Full Scan, Fast Scan, pausar, retomar, cancelar, selecionar raízes, exclusões, propriedades, exportar relatório.
- Explorador: painel de volumes; árvore de pastas; tabela de arquivos; tamanho lógico, alocado, categoria, MIME, datas, atributos; indicadores de confiança.
- Análise: Tree View com barras de proporção e Treemap com drill-down, breadcrumbs e filtros por volume, categoria, tipo MIME, idade, tamanho, profundidade.
- Monitoramento: taxa de eventos, backlog, latência, raízes cobertas, necessidade de reconciliação, logs e falhas.
- Configurações: serviço opcional, scan e limites, níveis de permissão, observadores, política de exclusão, preferências da Bancada.

**Dois modos de análise:** por volume selecionado e por computador inteiro (somando os volumes pertinentes sem contar duas vezes uma mesma montagem). Barras devem informar sua base de percentual: pasta-mãe, volume ou computador.

**Progresso realista:** exibir contagem de diretórios/arquivos descobertos, bytes inventariados, taxa, duração, fase, volume atual e fila. Não inventar percentual global quando o denominador ainda não é conhecido. Mostrar atividade e estimativas explicitamente marcadas quando aplicável.

### 10. Identificação de formatos e finalidade

**Formato:** conservar extensão original, extensão normalizada, tipo MIME conforme cadastro IANA quando houver, descrição humana e categoria (documento, imagem, áudio, vídeo, arquivo compactado, executável, etc.). Exemplos: PDF → Portable Document Format → application/pdf; PNG → Portable Network Graphics → image/png; JPEG → image/jpeg.

**Evidência da classificação:** extensão (inferência), assinatura/magic bytes (verificação leve, quando habilitada), atributos/metadados e, futuramente, análise avançada. Registrar divergência entre extensão e conteúdo, não renomear arquivos automaticamente.

**Finalidade/posse aparente:** sistema, aplicativo, usuário, compartilhado, misto, desconhecido. A classificação não prova propriedade ou segurança de exclusão; pastas podem ter mistura de categorias, com proporções por quantidade e por bytes. Dados de uso misto não devem ser forçados a uma única categoria categórica sem evidência.

### 11. Permissões e segurança

- O DAG opera sem root em uso normal. Quando uma operação autorizada de leitura exigir mais permissão, a GUI apresenta solicitação específica, no Linux com **Polkit**, e o componente privilegiado recebe somente as capacidades necessárias.
- Não executar a GUI, a interface SQL ou todo o scanner permanentemente como root; não pedir senha em campo próprio nem armazená-la.
- Helper valida origem da solicitação e escopo de acesso, usa caminhos/descritores com defesa contra symlink race, IPC local protegido e registra auditoria; não aceita execução de comandos genéricos.
- Permissão recusada: continuar o scan, marcar entradas inacessíveis, oferecer nova tentativa e relatório.
- Banco, logs e configurações devem ter permissões restritas; os caminhos inventariados são informações privadas. Nunca enviar inventário à nuvem na Fase A.
- Mesmo com autorização elevada, **Fase A é exclusivamente de leitura**: não renomear, mover, apagar, corrigir permissões ou alterar conteúdo de arquivos catalogados.
- Windows deve prever UAC; macOS deve prever autorização nativa/helper. A compatibilidade futura não deve ser simulada como funcional agora.

### 12. Consistência, falhas e recuperação

1. Cada scan registra escopo e geração. Uma raiz só pode considerar ausentes os itens não vistos **após** concluir a reconciliação sem erro impeditivo para aquela raiz.
2. Volume desconectado, filesystem indisponível, permissões negadas ou varredura interrompida são estados diferentes de remoção confirmada.
3. Watchers podem perder eventos (inclusive overflow do inotify); registrar lacuna, revarrer o escopo e recuperar consistência.
4. Criações e gravações contínuas: aplicar coalescência, fila com backpressure e limite de recursos; observar arquivos ainda em escrita e permitir reprocessamento.
5. Reinício inesperado: journal e checkpoints preservam scans incompletos; retomar de maneira idempotente.
6. Alterações durante o próprio Full Scan exigem reconciliação posterior; o estado do catálogo deve indicar atualização pendente quando pertinente.
7. Esquema de banco por migrações reversíveis quando possível e teste de backup/restore; health check de integridade SQLite.

### 13. Critérios de aceite testáveis

**CA-A01 — Inicialização Linux/Bancada:** compilar e abrir GUI Rust, Ribbon e páginas funcionais; sem necessidade de root; nenhuma escrita fora do diretório de dados/configuração do DAG.

**CA-A02 — Descoberta:** detectar volumes e montagens reais com IDs estáveis quando disponibilizados, capacidade/espaço livre coerentes com o SO, e estado offline sem deleção fictícia.

**CA-A03 — Varredura e persistência:** criar milhares de pastas/arquivos, executar Full Scan, consultar SQLite e verificar contagens, caminhos, hierarquia, tamanhos lógicos e datas. Tabelas files e folders permanecem separadas.

**CA-A04 — Links/montagens:** teste com symlink circular, symlink para ancestral, bind mount e hard links; scanner termina, registra entradas e não entra em loop. Métricas físicas devem evitar contabilização simples duplicada.

**CA-A05 — Tamanhos:** comparar arquivos pequenos, esparsos e tradicionais com APIs nativas; apresentar indisponível quando o FS não informar alocação; distinguir soma lógica, alocação por entrada e bytes físicos exclusivos.

**CA-A06 — MIME/categorias:** identificar extensões conhecidas; sinalizar desconhecidas; testagem de arquivo com extensão enganosa quando assinatura estiver habilitada; classificar arquivos e pastas com fonte/confiança, inclusive mistos.

**CA-A07 — Escalabilidade:** ensaio sintético de **pelo menos 1 milhão de entradas**; SQLite permanece responsivo, navegação paginada, RAM não cresce proporcionalmente ao catálogo inteiro, e o relatório registra latência/memória/I/O do ambiente de teste.

**CA-A08 — Tree View/Treemap:** navegar por volume, pasta e computador; barra e blocos mostram proporções corretas; zoom/drill-down; filtros; carregamento incremental sem ler o catálogo inteiro na GUI.

**CA-A09 — Fast Scan:** adicionar, modificar, renomear e remover arquivos entre scans; refletir diferenças; não marcar ausências em volumes desconectados ou varreduras incompletas.

**CA-A10 — Monitoramento:** enquanto GUI aberta, eventos de criação/edição/renomeação/remoção são persistidos e refletidos na interface após processamento; concorrência não duplica writers/watchers.

**CA-A11 — Perda de eventos:** forçar overflow ou simular notificação perdida; registrar lacuna e Fast Scan reconciliador; nenhum evento perdido é declarado processado sem correção.

**CA-A12 — Serviço opcional:** em instalação limpa, serviço não existe; UI consegue instalar e selecionar autostart; parar/desativar/remover; ao fechar GUI com serviço ativo o monitor continua; sem serviço, monitor funciona durante sessão da GUI; sem silenciosa instalação.

**CA-A13 — Permissões:** testar diretório protegido com e sem autorização Polkit; sem autorização registro inacessível, com autorização helper restrito lê metadados; GUI e SQLite seguem sem root; nenhum pedido armazena senha.

**CA-A14 — Recuperação:** cancelar, pausar, encerrar abruptamente e reiniciar scan; dados íntegros e fila retomável; nenhuma remoção falsa.

**CA-A15 — Segurança de dados:** confirmar por testes e observação que Full/Fast Scan, watcher e helper não modificam arquivos varridos; nenhuma funcionalidade de apagar/mover na Fase A.

**CA-A16 — Banco:** backup/restore consistente do SQLite com WAL, migração versionada e testes de integridade. Não assumir que snapshot do catálogo recupera conteúdo de arquivo.

**CA-A17 — Observabilidade:** estados de scan/monitor/serviço, erros de acesso, itens ignorados, ultima reconciliação e backlog exibidos, auditáveis e persistidos.

### 14. Plano de implementação e definição de pronto

**Incremento 1 — Fundação:** monorepo/crates Rust, integração Bancada, esquema SQLite, migrações e primeiras telas vazias com testes.  
**Incremento 2 — Inventário:** descoberta de volumes, árvore de diretórios, metadados de arquivos, exclusões, links/montagens e agregados.  
**Incremento 3 — Exploração:** Tree View, barras, Treemap, filtros, relatórios e escalabilidade paginada.  
**Incremento 4 — Atualização:** Fast Scan, watcher, eventos, conciliação, filas duráveis, checkpoint e estados offline.  
**Incremento 5 — Operação:** serviço opcional/configurável, monitor independente, Polkit/helper restrito, backup do catálogo, testes de resiliência e performance.

A entrega de cada incremento exige testes unitários e de integração, observabilidade, validação de segurança e documentação das decisões arquiteturais. Para encerrar a Fase A: todos os critérios CA-A01 a CA-A17 aprovados no Linux; documentação de limitações por FS; nenhum defeito crítico aberto de integridade ou risco de alteração de arquivos originais.

### 15. Decisões aprovadas e pendências técnicas

**Aprovado como requisito:** Linux primeiro; Rust + Bancada + SQLite; arquivos/pastas em tabelas separadas; Full/Fast Scan; watcher; sistema em tempo real enquanto ativo; serviço opt-in controlado na interface; classificação MIME/finalidade; Tree View e Treemap; Polkit sob demanda; somente leitura; preparação futura Windows/macOS.

**A detalhar pela equipe sem alterar o produto:** versão das crates e contratos da Bancada; escolha de bibliotecas de watch e filesystem; política de autenticação IPC e helper; esquema final de índices; empacotamento systemd/Polkit; escolhas exatas de filas e tempos de reconciliação; metas de latência a partir de baseline; documentação ADR.

**Não fazer na Fase A:** hashing integral como obrigação, deduplicação com exclusão, backup de arquivos em nuvem, reconhecimento facial, OCR em massa, ingestão no Calibre, limpeza automática, varredura de contas remotas autenticadas.

### 16. Diretrizes para a Fase D (preservação de decisões)

O DAG evoluirá para catálogos separados para fotos, livros, documentos e demais tipos, podendo integrar volumes locais e nuvens sem duplicar a fonte do inventário. Fotos poderão receber EXIF e agrupamento facial sujeito a confirmação humana e correção. PDFs e livros, inclusive digitalizações, terão análise inicial de **10 a 15 páginas** para sugerir autor, título, editora e outros metadados; o pré-cadastro será armazenado em catálogo SQLite separado, com **curadoria antes de importação**. A integração com Calibre precisa utilizar interfaces oficiais/seguras e respeitar o mecanismo de acesso coordenado existente no calibre-port. Backups futuros exigirão criptografia, retenção, testes de restauração e aprovação explícita para ações destrutivas.

---

**Repositório:** https://github.com/ChicoFigueiredo/Acervo-Digital  
**Documento produzido a partir das decisões de planejamento já discutidas; não há execução de testes nesta etapa.**
