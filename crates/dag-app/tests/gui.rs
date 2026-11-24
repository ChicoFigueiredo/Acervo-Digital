//! Fumaça da GUI sem display, com um Catálogo de verdade: o worker varre uma árvore temporária,
//! as páginas leem pela conexão somente-leitura e desenhamos todas por alguns quadros. Também
//! cobre o caminho completo de uma marcação (GUI → worker → Catálogo → leitura).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use bancada_casca::egui::{self, RawInput, Rect, pos2, vec2};
use bancada_casca::{Local, Rota};
use dag_app::app::{AppDag, ConfigApp, montar};
use dag_app::confirmacao::AcaoMarcacao;
use dag_app::cores::ModoCor;
use dag_app::paginas::{self, analise};
use dag_app::worker::ModoWorker;
use dag_core::VolumeDescoberto;

fn volume_de(p: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(p).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("teste-dev-{dev}"),
        st_dev: dev,
        rotulo: Some("teste".into()),
        fs_type: "ext4".into(),
        capacidade_bytes: Some(10_000_000),
        livre_bytes: Some(4_000_000),
        // Dois pontos de montagem do mesmo volume (bind mount).
        pontos_de_montagem: vec![
            p.as_os_str().as_bytes().to_vec(),
            [p.as_os_str().as_bytes(), b"/../arvore"].concat(),
        ],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: None,
    }
}

fn escrever(p: &Path, conteudo: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::File::create(p).unwrap().write_all(conteudo).unwrap();
}

struct Ambiente {
    _tmp: tempfile::TempDir,
    raiz: std::path::PathBuf,
    casca: bancada_casca::Casca<AppDag>,
    ctx: egui::Context,
}

impl Ambiente {
    fn quadro(&mut self) {
        let entrada = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0))),
            ..Default::default()
        };
        self.ctx
            .run_ui(entrada, |ui| self.casca.desenhar(ui))
            .drop_without_applying_deltas();
    }

    /// Desenha um quadro e devolve todo o texto que as formas dele contêm.
    fn quadro_texto(&mut self) -> String {
        fn coletar(f: &egui::Shape, t: &mut String) {
            match f {
                egui::Shape::Text(x) => {
                    t.push_str(x.galley.text());
                    t.push('\n');
                }
                egui::Shape::Vec(v) => v.iter().for_each(|x| coletar(x, t)),
                _ => {}
            }
        }
        let entrada = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
            ..Default::default()
        };
        let saida = self.ctx.run_ui(entrada, |ui| self.casca.desenhar(ui));
        let mut t = String::new();
        for c in &saida.shapes {
            coletar(&c.shape, &mut t);
        }
        saida.drop_without_applying_deltas();
        t
    }

    fn abrir(&mut self, pagina: &str) {
        self.casca
            .abrir(&self.ctx, Rota::nova(pagina), Local::Centro);
    }

    /// Desenha quadros até `pronto` (ou estourar o tempo).
    fn ate(&mut self, descricao: &str, pronto: impl Fn(&AppDag) -> bool) {
        let fim = Instant::now() + Duration::from_secs(60);
        loop {
            self.quadro();
            if pronto(&self.casca.app) {
                return;
            }
            assert!(Instant::now() < fim, "tempo esgotado: {descricao}");
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

/// Árvore com um grupo de duplicados (big.pdf e copia/big.pdf) e arquivos de tipos variados.
fn ambiente_populado() -> Ambiente {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("arvore");
    escrever(&raiz.join("big.pdf"), &[7u8; 9000]);
    escrever(&raiz.join("copia/big.pdf"), &[7u8; 9000]);
    escrever(&raiz.join("sub/x.bin"), &[1u8; 3000]);
    escrever(&raiz.join("sub/y.txt"), &[2u8; 2000]);
    escrever(&raiz.join("sub/fundo/z.mp3"), &[3u8; 1500]);
    escrever(&raiz.join("a.txt"), &[4u8; 100]);
    fs::create_dir_all(raiz.join("vazia")).unwrap();
    let mut cfg = ConfigApp::com_caminho(tmp.path().join("dados/catalogo.sqlite"));
    cfg.volumes_fixos = Some(vec![volume_de(&raiz)]);
    let casca = montar(cfg).unwrap();
    let mut a = Ambiente {
        _tmp: tmp,
        raiz,
        casca,
        ctx: egui::Context::default(),
    };
    a.ate("worker abrir o Catálogo", |app| {
        *app.worker.modo() == ModoWorker::Escritor && app.scan.is_none()
    });
    a
}

fn varrer(a: &mut Ambiente) {
    let raiz = a.raiz.display().to_string();
    a.casca.app.varredura.adicionar_raiz(raiz);
    a.casca.app.atualizar_volumes();
    a.casca.app.iniciar_full_scan(false);
    assert!(a.casca.app.scan_ativo());
    a.abrir("varredura");
    a.ate("varredura terminar", |app| {
        !app.scan_ativo() && app.ultimo_scan.is_some()
    });
}

#[test]
fn desenha_todas_as_paginas_com_catalogo_vazio_e_depois_populado() {
    let mut a = ambiente_populado();
    for p in paginas::todas() {
        a.abrir(p.id);
        for _ in 0..2 {
            a.quadro();
        }
    }
    varrer(&mut a);
    let r = a.casca.app.ultimo_scan.clone().unwrap();
    assert_eq!(r.desfecho, "concluida");
    assert_eq!(r.arquivos, 6);
    assert_eq!(r.grupos_duplicados, Some(1));
    for p in paginas::todas() {
        a.abrir(p.id);
        for _ in 0..3 {
            a.quadro();
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    a.ate("totais do catálogo", |app| app.totais.dado.is_some());
    let t = a.casca.app.totais.dado.unwrap();
    assert_eq!(t.volumes, 1, "dois pontos de montagem, um volume");
    assert_eq!(t.arquivos, 6);
    let s = a.casca.app.segmentos_de_status();
    assert!(
        s.iter()
            .any(|s| s.id == "totais" && s.texto.contains("1 volume(s)")),
        "{s:?}"
    );
}

#[test]
fn discos_e_varredura_mostram_o_que_o_worker_gravou() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    a.abrir("discos");
    a.ate("tabela de volumes", |app| {
        app.discos
            .volumes
            .dado
            .as_ref()
            .is_some_and(|v| v.iter().any(|x| x.bytes_catalogados.is_some()))
    });
    let v = a.casca.app.discos.volumes.dado.clone().unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].montagens, 2);
    assert!(v[0].conectado);
    assert!(v[0].bytes_catalogados.is_some());
    a.abrir("varredura");
    a.ate("últimas varreduras", |app| {
        app.varredura
            .execucoes
            .dado
            .as_ref()
            .is_some_and(|e| e.iter().any(|x| x.estado == "concluida"))
    });
    let e = a.casca.app.varredura.execucoes.dado.clone().unwrap();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].estado, "concluida");
}

#[test]
fn explorador_carrega_arvore_sob_demanda_e_arquivos_da_pasta() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    a.abrir("explorador");
    a.ate("raízes", |app| !app.explorador.arvore.is_empty());
    assert_eq!(a.casca.app.explorador.arvore.len(), 1);
    assert_eq!(
        a.casca.app.explorador.arvore[0].contar(),
        1,
        "nada de filhos carregado antes de expandir"
    );
    let raiz = a.casca.app.explorador.arvore[0].pasta.clone();
    assert!(raiz.tem_subpastas);
    // A árvore pode ser relida quando o Catálogo muda logo após a varredura (perdendo a
    // expansão): repete selecionar e expandir até os filhos aparecerem.
    let fim = Instant::now() + Duration::from_secs(60);
    loop {
        {
            let app = &mut a.casca.app;
            if app.explorador.arvore[0].filhos.is_empty() {
                let raiz = app.explorador.arvore[0].pasta.clone();
                app.explorador.selecionar(&app.leitor, raiz);
                app.explorador.expandir_selecionada(&app.leitor);
            }
        }
        for _ in 0..40 {
            a.quadro();
            std::thread::sleep(Duration::from_millis(10));
        }
        let app = &a.casca.app;
        if app.explorador.arvore[0].filhos.len() == 3 && app.explorador.arquivos.dado.is_some() {
            break;
        }
        assert!(Instant::now() < fim, "tempo esgotado: filhos e arquivos");
    }
    let arquivos = a.casca.app.explorador.arquivos.dado.clone().unwrap();
    let nomes: Vec<_> = arquivos.itens.iter().map(|f| f.nome.as_str()).collect();
    assert_eq!(nomes, ["big.pdf", "a.txt"]);
    assert!(arquivos.itens[0].tem_hash && arquivos.itens[0].duplicado);
    assert_eq!(arquivos.itens[0].categoria.as_deref(), Some("documento"));
}

#[test]
fn analise_trata_os_dois_modos_e_desce_um_nivel() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    a.abrir("analise");
    // Modo padrão: computador inteiro, um volume (sem contar duas vezes).
    a.ate("nível computador", |app| app.analise.dados.dado.is_some());
    let d = a.casca.app.analise.dados.dado.clone().unwrap();
    assert_eq!(d.filhos.len(), 1);
    assert_eq!(d.bytes_nivel, 9000 + 9000 + 3000 + 2000 + 1500 + 100);
    assert_eq!(d.bytes_nivel, d.bytes_computador);
    // Modo por volume.
    a.casca.app.analise.usar_modo(analise::ModoAnalise::Volume);
    a.ate("nível volume", |app| {
        app.analise
            .dados
            .dado
            .as_ref()
            .is_some_and(|d| matches!(d.alvo, dag_store::consulta_gui::AlvoAnalise::Volume(_)))
    });
    // Desce para a pasta raiz e desenha as duas visões em todos os modos de cor.
    let d = a.casca.app.analise.dados.dado.clone().unwrap();
    let alvo = match d.filhos[0].chave {
        dag_store::consulta_gui::ChaveNo::Pasta(id) => {
            dag_store::consulta_gui::AlvoAnalise::Pasta(id)
        }
        outro => panic!("esperava pasta: {outro:?}"),
    };
    a.casca.app.analise.alvo = alvo;
    a.ate("nível da pasta", |app| {
        app.analise
            .dados
            .dado
            .as_ref()
            .is_some_and(|d| d.alvo == alvo)
    });
    let d = a.casca.app.analise.dados.dado.clone().unwrap();
    assert!(d.filhos.len() >= 5);
    assert!(d.trilha.len() >= 3);
    for vista in [analise::Vista::Tree, analise::Vista::Treemap] {
        for cor in ModoCor::TODOS {
            a.casca.app.analise.vista = vista;
            a.casca.app.analise.cor = cor;
            for base in dag_app::bases::BasePercentual::TODAS {
                a.casca.app.analise.base = base;
                a.quadro();
            }
        }
    }
}

#[test]
fn duplicados_marcacao_de_ponta_a_ponta_com_confirmacao() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    a.abrir("duplicados");
    a.ate("lista de grupos", |app| {
        app.duplicados
            .lista
            .dado
            .as_ref()
            .is_some_and(|l| !l.grupos.is_empty())
    });
    let g = a.casca.app.duplicados.lista.dado.as_ref().unwrap().grupos[0].id;
    assert_eq!(a.casca.app.duplicados.lista.dado.as_ref().unwrap().total, 1);
    a.casca.app.duplicados.selecionado = Some(g);
    a.ate("detalhe do grupo", |app| {
        app.duplicados.detalhe.dado.is_some()
    });
    let det = a.casca.app.duplicados.detalhe.dado.clone().unwrap();
    assert_eq!(det.copias.len(), 2);
    assert_eq!(det.copias.iter().filter(|c| c.principal).count(), 1);

    // Pedir a marcação só abre a confirmação: nada é enviado ainda.
    a.casca.app.duplicados.grupo_inteiro = true;
    a.casca.app.pedir_confirmacao(AcaoMarcacao::MarcarDelecao);
    let plano = a.casca.app.duplicados.confirmacao.clone().unwrap();
    assert_eq!(plano.afetadas.len(), 1, "todas menos a principal");
    assert!(!plano.unica_copia);
    a.quadro(); // desenha o diálogo
    assert!(a.casca.app.pedidos.is_empty());

    // Confirmar manda o pedido ao worker; a página relê e mostra a marcação.
    let id = a.casca.app.enviar(
        dag_app::confirmacao::comando(&plano),
        dag_app::app::Proposito::Marcacao(plano.acao),
    );
    a.casca.app.duplicados.confirmacao = None;
    a.ate("resposta da marcação", |app| {
        !app.pedidos.contains_key(&id)
    });
    a.ate("detalhe com marcação", |app| {
        app.duplicados
            .detalhe
            .dado
            .as_ref()
            .is_some_and(|d| d.copias.iter().any(|c| c.marcacao.is_some()))
    });
    let det = a.casca.app.duplicados.detalhe.dado.clone().unwrap();
    let marcada = det.copias.iter().find(|c| c.marcacao.is_some()).unwrap();
    assert!(!marcada.principal);
    assert!(
        a.casca
            .app
            .avisos
            .iter()
            .any(|v| v.texto.contains("marcação(ões) registrada(s)")),
        "{:?}",
        a.casca.app.avisos
    );
    // O arquivo marcado continua no disco, intacto (o DAG só escreve no Catálogo).
    assert_eq!(fs::read(a.raiz.join("big.pdf")).unwrap().len(), 9000);
    assert_eq!(fs::read(a.raiz.join("copia/big.pdf")).unwrap().len(), 9000);
}

#[test]
fn exportar_relatorio_pelo_worker_grava_o_arquivo() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    let saida = a.raiz.parent().unwrap().join("saida.csv");
    let id = a.casca.app.enviar(
        dag_core::Comando::Relatorio {
            tipo: dag_core::TipoRelatorio::Marcacoes,
            nivel: None,
            limite: None,
            cursor: None,
            saida: Some(saida.display().to_string()),
        },
        dag_app::app::Proposito::Exportar,
    );
    a.ate("exportação", |app| !app.pedidos.contains_key(&id));
    assert!(saida.is_file());
    assert!(
        a.casca
            .app
            .avisos
            .iter()
            .any(|v| v.texto.contains("Relatório gravado"))
    );
}

#[test]
fn discos_e_varredura_com_mistura_de_volumes_reais_e_de_imagem() {
    use dag_core::ClasseVolume;
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("arvore");
    escrever(&raiz.join("a.txt"), &[4u8; 100]);
    let mut snap = volume_de(&raiz);
    snap.platform_id = "loop-snap".into();
    snap.st_dev = 0x9_0001;
    snap.fs_type = "squashfs".into();
    snap.classe = ClasseVolume::Imagem;
    snap.pontos_de_montagem = vec![b"/snap/bare/5".to_vec()];
    let mut zram = snap.clone();
    zram.platform_id = "zram".into();
    zram.st_dev = 0x9_0002;
    zram.classe = ClasseVolume::Virtual;
    zram.pontos_de_montagem = vec![b"/mnt/zram".to_vec()];
    let mut cfg = ConfigApp::com_caminho(tmp.path().join("dados/catalogo.sqlite"));
    cfg.volumes_fixos = Some(vec![volume_de(&raiz), snap, zram]);
    let mut a = Ambiente {
        _tmp: tmp,
        raiz,
        casca: montar(cfg).unwrap(),
        ctx: egui::Context::default(),
    };
    a.ate("worker abrir o Catálogo", |app| {
        *app.worker.modo() == ModoWorker::Escritor
    });
    a.abrir("discos");
    // A página pede a classificação sozinha; só então os ocultos aparecem.
    a.ate("classificação recebida", |app| app.classificacao.recebida);
    a.ate("tabela de volumes", |app| app.discos.volumes.dado.is_some());
    let (linhas, ocultos) = a.casca.app.linhas_de_discos();
    assert_eq!((linhas.len(), ocultos), (1, 2));
    a.casca.app.discos.mostrar_todos = true;
    let (linhas, ocultos) = a.casca.app.linhas_de_discos();
    assert_eq!((linhas.len(), ocultos), (3, 0));
    // Marcar o volume real e levar à Varredura.
    let id = linhas
        .iter()
        .find(|l| l.classe.eh_real())
        .unwrap()
        .volume
        .id;
    a.casca.app.discos.marcados.insert(id);
    for _ in 0..3 {
        a.quadro();
    }
    a.abrir("varredura");
    a.casca.app.varredura.adicionar_raiz("/");
    for _ in 0..3 {
        a.quadro();
    }
    a.abrir("discos");
    a.quadro();
}

#[test]
fn configuracoes_desenha_edita_e_persiste_o_paralelismo_do_hash() {
    let mut a = ambiente_populado();
    a.abrir("configuracoes");
    // Os campos recebem o valor gravado (padrão 1 thread por disco, 16 discos).
    a.ate("config carregar", |app| {
        app.configuracoes.gravado.dado.is_some()
    });
    a.quadro();
    assert_eq!(
        (
            a.casca.app.configuracoes.threads_por_disco,
            a.casca.app.configuracoes.max_discos
        ),
        (1, 16)
    );
    a.casca.app.salvar_config_hash(3, 5);
    a.ate("config gravar e recarregar", |app| {
        app.configuracoes
            .gravado
            .dado
            .is_some_and(|o| o.threads_por_disco == Some(3) && o.max_discos == Some(5))
    });
    for _ in 0..3 {
        a.quadro();
    }
    assert_eq!(
        (
            a.casca.app.configuracoes.threads_por_disco,
            a.casca.app.configuracoes.max_discos
        ),
        (3, 5)
    );
}

#[test]
fn abertura_retoma_hash_do_binario_antigo_e_a_gui_termina_ociosa() {
    use dag_agent::Agente;
    use dag_core::{Comando, ModoVarredura, Resposta};
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("arvore");
    escrever(&raiz.join("a.bin"), &[1u8; 3000]);
    escrever(&raiz.join("b.bin"), &[2u8; 4000]);
    let caminho = tmp.path().join("dados/catalogo.sqlite");
    {
        let mut ag = Agente::abrir(&caminho)
            .unwrap()
            .silencioso()
            .com_volumes_fixos(vec![volume_de(&raiz)]);
        let r = ag.executar(Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec![raiz.display().to_string()],
            sem_hash: false,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        });
        assert!(matches!(r, Resposta::ScanConcluido { .. }), "{r:?}");
    }
    {
        let cat = dag_store::Catalogo::abrir(&caminho).unwrap();
        cat.armazem()
            .conexao()
            .execute_batch(
                "DELETE FROM hashes; DELETE FROM fila_hash;
                 UPDATE scan_runs SET status='concluida_com_erros', phase='hash';",
            )
            .unwrap();
    }
    let mut cfg = ConfigApp::com_caminho(caminho.clone());
    cfg.volumes_fixos = Some(vec![volume_de(&raiz)]);
    let casca = montar(cfg).unwrap();
    let mut a = Ambiente {
        _tmp: tmp,
        raiz,
        casca,
        ctx: egui::Context::default(),
    };
    a.ate("worker abrir", |app| {
        *app.worker.modo() == ModoWorker::Escritor
    });
    a.ate("abertura terminar", |app| app.scan.is_none());
    let con = dag_store::consulta_gui::abrir_leitura(&caminho).unwrap();
    let n: i64 = con
        .query_row("SELECT COUNT(*) FROM hashes", [], |l| l.get(0))
        .unwrap();
    assert_eq!(n, 2, "a abertura retomou o hash pendente");
}

/// A GUI é só cliente do agente (ADR 0004): vê e controla a varredura que a CLI iniciou.
#[test]
fn gui_cliente_do_agente_ve_e_cancela_a_varredura_iniciada_pela_cli() {
    use dag_core::{Autor, Comando, ModoVarredura, OrigemJob, Pedido, Resposta};

    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let raiz = base.join("arvore");
    for d in 0..120 {
        for f in 0..60 {
            escrever(&raiz.join(format!("g{d}/f{f}.txt")), &[b'x'; 80]);
        }
    }
    let catalogo = base.join("dados/catalogo.sqlite");
    let socket = base.join("run/agente.sock");
    let mut cfg_srv = dag_agent::ConfigServidor::novo(&catalogo, &socket);
    cfg_srv.monitor = false;
    cfg_srv.abertura = dag_agent::PoliticaAbertura::desligada();
    cfg_srv.volumes_fixos = Some(vec![volume_de(&raiz)]);
    let srv = dag_agent::Servidor::iniciar(cfg_srv).unwrap();

    let mut cfg = ConfigApp::com_caminho(catalogo);
    cfg.socket = Some(socket.clone());
    cfg.sem_agente = false;
    let casca = montar(cfg).unwrap();
    let mut a = Ambiente {
        _tmp: tmp,
        raiz: raiz.clone(),
        casca,
        ctx: egui::Context::default(),
    };
    a.ate("GUI conectar ao agente", |app| {
        *app.worker.modo() == ModoWorker::Servico
    });
    assert!(!a.casca.app.scan_ativo());

    // A CLI inicia uma varredura por outra conexão; a GUI não pediu nada.
    let cli = std::thread::spawn({
        let (socket, raiz) = (socket.clone(), raiz.clone());
        move || {
            dag_agent::Cliente::conectar(&socket)
                .unwrap()
                .enviar(
                    Pedido {
                        autor: Autor::Cli,
                        comando: Comando::IniciarScan {
                            modo: ModoVarredura::FullScan,
                            raizes: vec![raiz.to_str().unwrap().into()],
                            sem_hash: false,
                            elevar: false,
                            releitura_total: false,
                            threads_por_disco: None,
                            max_discos: None,
                        },
                    },
                    &mut |_| {},
                )
                .unwrap()
        }
    });
    a.ate("varredura da CLI aparecer na GUI com progresso", |app| {
        app.scan
            .as_ref()
            .is_some_and(|s| s.origem_externa() == Some(OrigemJob::Cli) && s.progresso.is_some())
    });
    // A barra de status e a página Varredura dizem de quem é a varredura.
    let status = a.casca.app.segmentos_de_status();
    let fase = status.iter().find(|s| s.id == "fase").unwrap();
    assert!(fase.texto.contains("[iniciada por cli]"), "{}", fase.texto);
    a.abrir("varredura");
    a.quadro();
    // Escrita pela GUI fica bloqueada enquanto há varredura, com o motivo.
    assert!(a.casca.app.motivo_sem_escrita().is_some());

    // Pausar e cancelar pela GUI agem sobre o trabalho da CLI.
    a.casca.app.alternar_pausa();
    a.ate("varredura pausada", |app| {
        app.estado_do_scan() == Some(dag_scanner::EstadoControle::Pausado)
    });
    a.casca.app.alternar_pausa();
    a.ate("varredura retomada", |app| {
        app.estado_do_scan() == Some(dag_scanner::EstadoControle::Executando)
    });
    a.casca.app.worker.cancelar();
    a.ate("varredura cancelada", |app| {
        app.scan.is_none() && app.ultimo_scan.is_some()
    });
    assert_eq!(
        a.casca.app.ultimo_scan.as_ref().unwrap().desfecho,
        "cancelada"
    );
    assert!(matches!(
        cli.join().unwrap(),
        Resposta::ScanConcluido { desfecho, .. } if desfecho == "cancelada"
    ));
    // A GUI continua usando o mesmo agente (um só escritor).
    assert!(a.casca.app.pode_escrever());
    drop(a);
    srv.encerrar();
}

/// A retomada de hash da abertura aparece em palavras simples (nunca texto de inventário), com
/// totais, threads e a tabela por disco, também na barra de status.
#[test]
fn painel_da_varredura_mostra_a_retomada_do_hash_com_dados_por_disco() {
    use dag_app::app::ScanAtivo;
    use dag_core::{PassoAbertura, ProgressoDiscoHash, ProgressoHash, TipoPasso};

    let mut a = ambiente_populado();
    let mut s = ScanAtivo::novo(dag_app::app::PEDIDO_ABERTURA, false);
    s.abertura = Some("etapa 1 de 2: retomando a execução #1 (hash)".into());
    s.passo = Some(PassoAbertura {
        tipo: TipoPasso::Retomada,
        execucao: Some(1),
        modo: "Full Scan".into(),
        no_inventario: false,
        passada: 0,
        de: 0,
    });
    s.bytes_hash = Some(ProgressoHash {
        arquivos_feitos: 8_000_000,
        arquivos_total: 19_700_000,
        bytes_lidos: 5 << 40,
        bytes_total: 12 << 40,
        bytes_por_seg: 210.0 * 1_048_576.0,
        reaproveitados: 7_900_000,
        execucao: Some(1),
        threads_por_disco: 1,
        max_discos: 4,
        erros: 2,
        arquivos_por_seg: 205.0,
        janela_ms: 30_000,
        discos: vec![ProgressoDiscoHash {
            disco: "WD Red 4TB".into(),
            volumes: vec![1, 2],
            threads: 1,
            leitores_ativos: 1,
            arquivos_feitos: 3_000_000,
            arquivos_total: 9_000_000,
            bytes_lidos: 2 << 40,
            bytes_total: 6 << 40,
            bytes_por_seg: 110.0 * 1_048_576.0,
            ativo: true,
            concluido: false,
            lendo: Vec::new(),
        }],
        ..ProgressoHash::default()
    });
    a.casca.app.scan = Some(s);
    a.abrir("varredura");
    let t = a.quadro_texto();
    for esperado in [
        "Retomada do hash da execução #1 (Full Scan)",
        "nesta janela (modo sem agente) — fechar interrompe",
        "1 thread(s) por disco, até 4 disco(s) em paralelo",
        "7.999.998 com hash de 19.700.000 arquivos (40,6%)",
        "lidos nesta etapa",
        "Por disco físico",
        "WD Red 4TB",
        "lendo",
        "Tempo restante",
    ] {
        assert!(t.contains(esperado), "faltou {esperado:?} em:\n{t}");
    }
    assert!(!t.contains("Descobrindo volumes"), "{t}");
    assert!(!t.contains("Sem percentual global"), "{t}");
    let barra = a.casca.app.segmentos_de_status();
    let fase = barra.iter().find(|s| s.id == "fase").unwrap();
    assert!(
        fase.texto.contains("Retomada do hash da execução #1") && fase.texto.contains("40,6%"),
        "{}",
        fase.texto
    );
    let totais = barra.iter().find(|s| s.id == "totais").unwrap();
    assert!(totais.texto.contains("com hash"), "{}", totais.texto);
    a.casca.app.scan = None;
}

#[test]
fn todas_as_paginas_desenham_em_janela_baixa_sem_cortar_a_rolagem() {
    let mut a = ambiente_populado();
    varrer(&mut a);
    for p in paginas::todas() {
        a.abrir(p.id);
        for _ in 0..3 {
            let entrada = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(720.0, 300.0))),
                ..Default::default()
            };
            a.ctx
                .run_ui(entrada, |ui| a.casca.desenhar(ui))
                .drop_without_applying_deltas();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn lixeira_dois_dialogos_mover_e_restaurar_de_ponta_a_ponta() {
    use dag_app::lixeira::{AlvoMenu, ItemMenu, Passo};
    use dag_core::Alvo;

    let mut a = ambiente_populado();
    varrer(&mut a);
    let sub = a.raiz.join("sub");
    let alvo = AlvoMenu {
        alvo: Alvo::Pasta(sub.display().to_string()),
        rotulo: "sub".into(),
        caminho: Some(sub.display().to_string()),
        pasta: true,
        na_lixeira: false,
        marca: None,
        arquivos: Some(3),
    };
    let ctx = a.ctx.clone();
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::MoverParaLixeira, alvo));
    // Primeiro diálogo: a simulação não tem nada a mover (nenhum arquivo está marcado).
    a.ate("prévia", |app| {
        app.lixeira
            .dialogo
            .as_ref()
            .is_some_and(|d| !matches!(d.passo, Passo::Calculando))
    });
    {
        let d = a.casca.app.lixeira.dialogo.as_mut().unwrap();
        d.continuar();
        assert!(
            matches!(d.passo, Passo::Previa(_)),
            "nada a mover: não avança"
        );
        assert!(d.comando_mover().is_none());
        d.marcar_tudo = true;
        d.passo = Passo::Calculando;
    }
    let cmd = a
        .casca
        .app
        .lixeira
        .dialogo
        .as_ref()
        .unwrap()
        .comando_previa();
    a.casca
        .app
        .enviar(cmd, dag_app::app::Proposito::LixeiraPrevia);
    a.ate("prévia com marcar tudo", |app| {
        matches!(
            app.lixeira.dialogo.as_ref().map(|d| &d.passo),
            Some(Passo::Previa(r)) if r.total_arquivos > 0 || !r.recusados.is_empty()
        )
    });
    if let Some(Passo::Previa(r)) = a.casca.app.lixeira.dialogo.as_ref().map(|d| &d.passo) {
        assert_eq!(r.total_arquivos, 3, "{r:?}");
    }
    // Segundo diálogo: sem a confirmação e o aceite da Única Cópia, nada é enviado.
    {
        let d = a.casca.app.lixeira.dialogo.as_mut().unwrap();
        d.continuar();
        assert!(matches!(d.passo, Passo::Confirmar(_)));
        assert!(d.comando_mover().is_none());
        d.entendo = true;
        assert!(d.comando_mover().is_none(), "falta aceitar a Única Cópia");
        d.aceita_unica = true;
    }
    a.quadro_texto();
    let cmd = a
        .casca
        .app
        .lixeira
        .dialogo
        .as_ref()
        .unwrap()
        .comando_mover()
        .unwrap();
    a.casca.app.lixeira.dialogo = None;
    a.casca
        .app
        .enviar(cmd, dag_app::app::Proposito::LixeiraMover);
    a.ate("mover", |app| app.pedidos.is_empty());
    assert!(!sub.join("x.bin").exists());
    assert!(a.raiz.join("DAG-Lixeira/sub/x.bin").exists());

    // Página Lixeira: lista, banner e restauração pelo Ribbon.
    a.abrir("lixeira");
    a.ate("lista Na Lixeira", |app| {
        app.lixeira
            .lista
            .dado
            .as_ref()
            .is_some_and(|l| l.total == 3)
    });
    let texto = a.quadro_texto();
    assert!(texto.contains("Na Lixeira"), "{texto}");
    let ids: Vec<i64> = a
        .casca
        .app
        .lixeira
        .lista
        .dado
        .as_ref()
        .unwrap()
        .itens
        .iter()
        .map(|i| i.arquivo.0)
        .collect();
    a.casca.app.lixeira.marcados.extend(ids);
    a.casca.app.executar_acao(&ctx, "lixeira.restaurar");
    a.ate("restaurar", |app| app.pedidos.is_empty());
    assert!(sub.join("x.bin").exists());
    assert!(!a.raiz.join("DAG-Lixeira/sub/x.bin").exists());
}

#[test]
fn menu_de_contexto_depende_de_estar_na_lixeira() {
    use dag_app::lixeira::{AlvoMenu, ItemMenu, itens_do_menu};
    use dag_core::{Alvo, ArquivoId};
    let mut m = AlvoMenu {
        alvo: Alvo::Arquivo(ArquivoId(1)),
        rotulo: "a".into(),
        caminho: Some("/x/a".into()),
        pasta: false,
        na_lixeira: false,
        marca: None,
        arquivos: None,
    };
    assert!(itens_do_menu(&m).contains(&ItemMenu::MoverParaLixeira));
    m.na_lixeira = true;
    assert!(itens_do_menu(&m).contains(&ItemMenu::RestaurarDaLixeira));
}

#[derive(Debug, Default)]
struct AbridorGravador(std::sync::Mutex<Vec<std::path::PathBuf>>);

impl dag_app::lixeira::Abridor for AbridorGravador {
    fn abrir(&self, caminho: &Path) -> Result<(), String> {
        self.0.lock().unwrap().push(caminho.to_path_buf());
        Ok(())
    }
}

fn marcas_no_catalogo(a: &Ambiente) -> Vec<(String, String)> {
    let c = rusqlite::Connection::open_with_flags(
        a._tmp.path().join("dados/catalogo.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut st = c
        .prepare("SELECT tipo, escopo FROM marcacoes ORDER BY id")
        .unwrap();
    st.query_map([], |l| Ok((l.get(0)?, l.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn menu_abre_arquivo_e_pasta_pelo_abridor_injetavel() {
    use dag_app::lixeira::{AlvoMenu, ItemMenu};
    use dag_core::{Alvo, ArquivoId};
    let mut a = ambiente_populado();
    let g = std::sync::Arc::new(AbridorGravador::default());
    a.casca.app.lixeira.abridor = g.clone();
    let ctx = a.ctx.clone();
    let mk = |pasta: bool, c: &str| AlvoMenu {
        alvo: if pasta {
            Alvo::Pasta(c.into())
        } else {
            Alvo::Arquivo(ArquivoId(1))
        },
        rotulo: "n".into(),
        caminho: Some(c.into()),
        pasta,
        na_lixeira: false,
        marca: None,
        arquivos: None,
    };
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::AbrirArquivo, mk(false, "/d/sub/y.txt")));
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::AbrirPasta, mk(true, "/d/sub")));
    a.casca.app.tratar_escolha(
        &ctx,
        (ItemMenu::AbrirNoGerenciador, mk(false, "/d/sub/y.txt")),
    );
    assert_eq!(
        *g.0.lock().unwrap(),
        vec![
            std::path::PathBuf::from("/d/sub/y.txt"),
            std::path::PathBuf::from("/d/sub"),
            std::path::PathBuf::from("/d/sub"),
        ]
    );
}

#[test]
fn menu_marca_e_desmarca_copia_na_pagina_duplicados() {
    use dag_app::lixeira::{AlvoMenu, ItemMenu, itens_do_menu};
    let mut a = ambiente_populado();
    varrer(&mut a);
    a.abrir("duplicados");
    a.ate("lista de grupos", |app| {
        app.duplicados
            .lista
            .dado
            .as_ref()
            .is_some_and(|l| !l.grupos.is_empty())
    });
    let g = a.casca.app.duplicados.lista.dado.as_ref().unwrap().grupos[0].id;
    a.casca.app.duplicados.selecionado = Some(g);
    a.ate("detalhe do grupo", |app| {
        app.duplicados.detalhe.dado.is_some()
    });
    let ctx = a.ctx.clone();
    let copia = a
        .casca
        .app
        .duplicados
        .detalhe
        .dado
        .as_ref()
        .unwrap()
        .copias
        .iter()
        .find(|c| !c.principal)
        .unwrap()
        .clone();
    let alvo = AlvoMenu::de_copia(&copia);
    assert!(itens_do_menu(&alvo).contains(&ItemMenu::MarcarDelecao));
    assert_eq!(alvo.cabecalho(), "Sem marcação para deleção");
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::MarcarDelecao, alvo));
    a.ate("copia marcada", |app| {
        app.duplicados.detalhe.dado.as_ref().is_some_and(|d| {
            d.copias
                .iter()
                .any(|c| c.arquivo == copia.arquivo && c.marcacao.is_some())
        })
    });
    assert_eq!(
        marcas_no_catalogo(&a),
        vec![("delecao".to_string(), "manual_arquivo".to_string())]
    );
    let marcada = a
        .casca
        .app
        .duplicados
        .detalhe
        .dado
        .as_ref()
        .unwrap()
        .copias
        .iter()
        .find(|c| c.arquivo == copia.arquivo)
        .unwrap()
        .clone();
    let alvo = AlvoMenu::de_copia(&marcada);
    assert!(itens_do_menu(&alvo).contains(&ItemMenu::DesmarcarDelecao));
    assert!(alvo.cabecalho().starts_with("Marcado para deleção"));
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::DesmarcarDelecao, alvo));
    a.ate("copia desmarcada", |app| {
        app.pedidos.is_empty()
            && app
                .duplicados
                .detalhe
                .dado
                .as_ref()
                .is_some_and(|d| d.copias.iter().all(|c| c.marcacao.is_none()))
    });
    assert!(marcas_no_catalogo(&a).is_empty());
    // Só o Catálogo mudou.
    assert!(a.raiz.join("copia/big.pdf").exists() && a.raiz.join("big.pdf").exists());
}

#[test]
fn menu_marca_pasta_com_confirmacao_de_contagem_e_mostra_o_estado() {
    use dag_app::lixeira::{AlvoMenu, ItemMenu, comando_marca};
    use dag_store::consulta_gui::{NoPasta, subpastas};
    let mut a = ambiente_populado();
    varrer(&mut a);
    let ctx = a.ctx.clone();
    let sub = a.raiz.join("sub").display().to_string();
    let pasta = NoPasta {
        id: 0,
        volume_id: 0,
        nome: "sub".into(),
        caminho: sub,
        bytes_logicos: 0,
        bytes_alocados: None,
        arquivos: 3,
        pastas: 1,
        estado_agregado: None,
        tem_subpastas: true,
        mtime_ns: None,
        marca: None,
    };
    let alvo = AlvoMenu::de_pasta(&pasta);
    a.ate("quietude", |app| app.pedidos.is_empty());
    a.casca
        .app
        .tratar_escolha(&ctx, (ItemMenu::MarcarDelecao, alvo.clone()));
    // Pasta pede confirmação com a contagem; nada foi enviado.
    let c = a.casca.app.lixeira.confirma_marca.clone().unwrap();
    assert!(c.texto().contains("3 arquivo(s)"), "{}", c.texto());
    a.quadro();
    assert!(a.casca.app.pedidos.is_empty());
    assert!(marcas_no_catalogo(&a).is_empty());
    // Confirmar grava a regra de pasta (só no Catálogo).
    a.casca.app.lixeira.confirma_marca = None;
    a.casca.app.enviar(
        comando_marca(&alvo, true),
        dag_app::app::Proposito::Marcacao(dag_app::confirmacao::AcaoMarcacao::MarcarDelecao),
    );
    a.ate("marca da pasta", |app| app.pedidos.is_empty());
    assert_eq!(
        marcas_no_catalogo(&a),
        vec![("delecao".to_string(), "regra_pasta".to_string())]
    );
    assert!(a.raiz.join("sub/x.bin").exists());
    // A árvore agora traz o estado e o menu mostra "Marcado para deleção por gui em ...".
    let c = rusqlite::Connection::open_with_flags(
        a._tmp.path().join("dados/catalogo.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let raiz_id: i64 = c
        .query_row(
            "SELECT id FROM folders WHERE path_display = ?1",
            [a.raiz.display().to_string()],
            |l| l.get(0),
        )
        .unwrap();
    let filhas = subpastas(&c, raiz_id, None, 50).unwrap().itens;
    let sub = filhas.iter().find(|p| p.nome == "sub").unwrap();
    let alvo = AlvoMenu::de_pasta(sub);
    assert!(
        alvo.cabecalho()
            .starts_with("Marcado para deleção por gui em "),
        "{}",
        alvo.cabecalho()
    );
    assert!(alvo.marcado());
}
