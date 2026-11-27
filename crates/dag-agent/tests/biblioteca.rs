//! Curadoria Bibliográfica no agente: Homologada sintética (SQLite montado no teste), arquivos
//! reais num Catálogo real, extrator falso contável e, quando existir, o `calibredb` real.
//! Cobre CA-B01, CA-B04, CA-B11, CA-B12, CA-B13, CA-B16/B17 e CA-B23 (parcial).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "suporte_pdf/mod.rs"]
mod suporte_pdf;

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use dag_agent::{Agente, ExtratorLivro, SaidaExtracao};
use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, OrigemEvidencia, ResultadoExtracao,
    ValorSugerido,
};
use dag_core::{
    AprovacaoHumana, Autor, Comando, ConfigBiblioteca, DadosBiblioteca, DispositivoDescoberto,
    EstadoFilaInfo, ModoVarredura, MotivoFila, PapelRegistro, Resposta, TipoDispositivo,
    VolumeDescoberto,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const ESQUEMA: &str = "
CREATE TABLE books (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL DEFAULT 'Unknown', sort TEXT,
  timestamp TIMESTAMP, pubdate TIMESTAMP, series_index REAL NOT NULL DEFAULT 1.0, author_sort TEXT,
  isbn TEXT DEFAULT '', lccn TEXT DEFAULT '', path TEXT NOT NULL DEFAULT '', flags INTEGER NOT NULL DEFAULT 1,
  uuid TEXT, has_cover BOOL DEFAULT 0, last_modified TIMESTAMP NOT NULL DEFAULT '2000-01-01 00:00:00+00:00');
CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_authors_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, author INTEGER NOT NULL);
CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_tags_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, tag INTEGER NOT NULL);
CREATE TABLE series (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_series_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, series INTEGER NOT NULL);
CREATE TABLE publishers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort TEXT, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_publishers_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, publisher INTEGER NOT NULL);
CREATE TABLE languages (id INTEGER PRIMARY KEY, lang_code TEXT NOT NULL, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_languages_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, lang_code INTEGER NOT NULL, item_order INTEGER NOT NULL DEFAULT 0);
CREATE TABLE identifiers (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, type TEXT NOT NULL DEFAULT 'isbn', val TEXT NOT NULL);
CREATE TABLE comments (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, text TEXT NOT NULL);
CREATE TABLE ratings (id INTEGER PRIMARY KEY, rating INTEGER, link TEXT NOT NULL DEFAULT '');
CREATE TABLE books_ratings_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, rating INTEGER NOT NULL);
CREATE TABLE data (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, format TEXT NOT NULL, uncompressed_size INTEGER NOT NULL, name TEXT NOT NULL);
CREATE TABLE custom_columns (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL, name TEXT NOT NULL, datatype TEXT NOT NULL,
  mark_for_delete BOOL DEFAULT 0 NOT NULL, editable BOOL DEFAULT 1 NOT NULL, display TEXT DEFAULT '{}' NOT NULL,
  is_multiple BOOL DEFAULT 0 NOT NULL, normalized BOOL NOT NULL);
CREATE TABLE preferences (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE, val TEXT NOT NULL);
CREATE TABLE library_id (id INTEGER PRIMARY KEY, uuid TEXT NOT NULL UNIQUE);
PRAGMA user_version = 26;
";

fn volume(raiz: &Path) -> VolumeDescoberto {
    let dev = fs::metadata(raiz).unwrap().dev();
    VolumeDescoberto {
        platform_id: format!("SSD-{dev}"),
        st_dev: dev,
        rotulo: Some("SSD".into()),
        fs_type: "teste".into(),
        capacidade_bytes: Some(1_000_000),
        livre_bytes: Some(400_000),
        pontos_de_montagem: vec![raiz.as_os_str().as_bytes().to_vec()],
        pseudo: false,
        classe: dag_core::ClasseVolume::Fisico,
        sistema: false,
        dispositivo: Some(DispositivoDescoberto {
            identificador: "disco-ssd".into(),
            modelo: None,
            tipo: TipoDispositivo::Ssd,
            removivel: false,
        }),
    }
}

/// Conteúdo de um "livro" de tamanho razoável, distinto por semente.
fn corpo(semente: u8, n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| semente.wrapping_add((i % 251) as u8))
        .collect()
}

struct Mundo {
    _tmp: TempDir,
    raiz: PathBuf,
    homologada: PathBuf,
    downloads: PathBuf,
    catalogo: PathBuf,
}

/// Homologada sintética com 3 livros (PDF/EPUB), `downloads/` vazio.
fn mundo() -> Mundo {
    let tmp = tempfile::tempdir().unwrap();
    let raiz = fs::canonicalize(tmp.path()).unwrap().join("acervo");
    let homologada = raiz.join("Homologada");
    let downloads = raiz.join("Downloads");
    fs::create_dir_all(&downloads).unwrap();
    fs::create_dir_all(&homologada).unwrap();
    let conn = Connection::open(homologada.join("metadata.db")).unwrap();
    conn.execute_batch(ESQUEMA).unwrap();
    conn.execute(
        "INSERT INTO library_id(uuid) VALUES ('uuid-homologada')",
        [],
    )
    .unwrap();
    let livros = [
        (
            1_i64,
            "Dom Casmurro",
            "Machado de Assis",
            "9788535902778",
            "PDF",
            1_u8,
        ),
        (
            2,
            "Memórias Póstumas",
            "Machado de Assis",
            "9788535910663",
            "EPUB",
            2,
        ),
        (
            3,
            "O Alienista",
            "Machado de Assis",
            "9788535911381",
            "PDF",
            3,
        ),
    ];
    for (id, titulo, autor, isbn, fmt, semente) in livros {
        let pasta = format!("{autor}/{titulo} ({id})");
        let nome = titulo.to_owned();
        conn.execute(
            "INSERT INTO books(id,title,sort,pubdate,series_index,author_sort,path,uuid,has_cover,last_modified)
             VALUES (?1,?2,?2,'0101-01-01 00:00:00+00:00',1.0,'Assis, Machado de',?3,?4,0,'2024-01-01 00:00:00+00:00')",
            params![id, titulo, pasta, format!("uuid-{id}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO authors(id,name,sort) VALUES (?1,?2,'Assis, Machado de')",
            params![id, autor],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books_authors_link(book,author) VALUES (?1,?1)",
            [id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO identifiers(book,type,val) VALUES (?1,'isbn',?2)",
            params![id, isbn],
        )
        .unwrap();
        let dados = corpo(semente, 20_000 + usize::from(semente) * 1000);
        conn.execute(
            "INSERT INTO data(book,format,uncompressed_size,name) VALUES (?1,?2,?3,?4)",
            params![id, fmt, dados.len() as i64, nome],
        )
        .unwrap();
        let dir = homologada.join(&pasta);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{nome}.{}", fmt.to_lowercase())), &dados).unwrap();
    }
    drop(conn);
    let catalogo = tmp.path().join("dados/catalogo.sqlite");
    Mundo {
        _tmp: tmp,
        raiz,
        homologada,
        downloads,
        catalogo,
    }
}

/// Extrator falso: conta chamadas e devolve metadados fixos (com ISBN).
struct ExtratorContavel {
    chamadas: Arc<AtomicUsize>,
    estado: EstadoLeitura,
}

impl ExtratorLivro for ExtratorContavel {
    fn nome(&self) -> String {
        "falso".into()
    }

    fn extrair(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        _cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        self.chamadas.fetch_add(1, Ordering::SeqCst);
        let nome = caminho.file_stem().unwrap().to_string_lossy().into_owned();
        let ev = |c: f32| Evidencia {
            origem: OrigemEvidencia::MetadadoInterno,
            pagina: None,
            trecho: Some(nome.clone()),
            detalhe: None,
            confianca: c,
        };
        let v = |campo, valor: &str, c| ValorSugerido {
            campo,
            valor: Some(valor.into()),
            lista: Vec::new(),
            confianca: c,
            evidencias: vec![ev(c)],
        };
        Ok(SaidaExtracao {
            resultado: ResultadoExtracao {
                formato,
                estado: self.estado.clone(),
                metadados: vec![
                    v(Campo::Titulo, &format!("Título de {nome}"), 0.8),
                    v(Campo::Autores, "Autor Desconhecido", 0.7),
                ],
                isbns: Vec::new(),
                sumario: Vec::new(),
                paginas: Vec::new(),
                total_paginas: Some(10),
                ficha_catalografica: None,
                capa: None,
                capa_mime: None,
                paginas_ocr: Vec::new(),
                avisos: Vec::new(),
            },
            paginas_ocr: 0,
            capa: None,
            avisos: Vec::new(),
        })
    }
}

struct Ambiente {
    m: Mundo,
    chamadas: Arc<AtomicUsize>,
    deslocamento: Arc<AtomicI64>,
}

impl Ambiente {
    fn novo() -> Self {
        Self {
            m: mundo(),
            chamadas: Arc::new(AtomicUsize::new(0)),
            deslocamento: Arc::new(AtomicI64::new(0)),
        }
    }

    fn agente(&self, autor: Autor) -> Agente {
        let d = Arc::clone(&self.deslocamento);
        Agente::abrir(&self.m.catalogo)
            .unwrap()
            .silencioso()
            .com_autor(autor)
            .com_volumes_fixos(vec![volume(&self.m.raiz)])
            .com_extrator_livro(Arc::new(ExtratorContavel {
                chamadas: Arc::clone(&self.chamadas),
                estado: EstadoLeitura::Lido,
            }))
            .com_relogio_biblioteca(move || {
                let real = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                real + d.load(Ordering::SeqCst)
            })
    }

    fn avancar(&self, s: i64) {
        self.deslocamento.fetch_add(s, Ordering::SeqCst);
    }
}

fn scan(m: &Mundo, modo: ModoVarredura) -> Comando {
    Comando::IniciarScan {
        modo,
        raizes: vec![m.raiz.to_str().unwrap().to_string()],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    }
}

fn bib(r: Resposta) -> DadosBiblioteca {
    match r {
        Resposta::Biblioteca { dados } => dados,
        outra => panic!("esperava Biblioteca, veio {outra:?}"),
    }
}

fn registrar_homologada(a: &mut Agente, m: &Mundo) -> i64 {
    match bib(a.executar(Comando::BibliotecaRegistrar {
        caminho: m.homologada.to_str().unwrap().into(),
        papel: PapelRegistro::Homologada,
        nome: Some("Homologada".into()),
    })) {
        DadosBiblioteca::Registrada { biblioteca, .. } => biblioteca.id,
        d => panic!("{d:?}"),
    }
}

fn sincronizar(a: &mut Agente, id: i64) -> dag_core::ResumoSincronizacao {
    match bib(a.executar(Comando::BibliotecaSincronizar {
        biblioteca: id.to_string(),
    })) {
        DadosBiblioteca::Sincronizada { resumo } => resumo,
        d => panic!("{d:?}"),
    }
}

fn enfileirar(a: &mut Agente, raiz: &Path, motivo: MotivoFila) -> dag_core::ResumoEnfileiramento {
    match bib(a.executar(Comando::BibliotecaEnfileirar {
        raizes: vec![raiz.to_str().unwrap().into()],
        motivo: Some(motivo),
    })) {
        DadosBiblioteca::Enfileirada { resumo } => resumo,
        d => panic!("{d:?}"),
    }
}

fn processar(a: &mut Agente) -> dag_core::ResumoProcessamento {
    match bib(a.executar(Comando::BibliotecaProcessar {
        limite: None,
        ia: false,
    })) {
        DadosBiblioteca::Processada { resumo } => resumo,
        d => panic!("{d:?}"),
    }
}

fn alertas(a: &mut Agente) -> Vec<dag_core::AlertaInfo> {
    match bib(a.executar(Comando::BibliotecaAlertas {
        nao_vistos: false,
        limite: Some(200),
        cursor: None,
    })) {
        DadosBiblioteca::Alertas { itens, .. } => itens,
        d => panic!("{d:?}"),
    }
}

fn arvore(raiz: &Path) -> Vec<(PathBuf, u64, i64, Vec<u8>)> {
    fn rec(d: &Path, v: &mut Vec<(PathBuf, u64, i64, Vec<u8>)>) {
        let mut es: Vec<_> = fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        es.sort();
        for p in es {
            let md = fs::symlink_metadata(&p).unwrap();
            if md.is_dir() {
                rec(&p, v);
            } else {
                v.push((p.clone(), md.len(), md.mtime(), fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    rec(raiz, &mut v);
    v
}

// ---------------------------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------------------------

#[test]
fn ca_b01_sincronizar_nao_altera_a_homologada() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let antes = arvore(&e.m.homologada);
    let id = registrar_homologada(&mut a, &e.m);
    let r = sincronizar(&mut a, id);
    assert_eq!((r.livros, r.formatos), (3, 3));
    assert!(r.biblioteca_intacta, "diferenças: {:?}", r.diferencas);
    assert_eq!(r.formatos_ligados_ao_catalogo, 3);
    assert_eq!(r.formatos_sem_hash, 0);
    // Segunda vez: idempotente e igual.
    let r2 = sincronizar(&mut a, id);
    assert_eq!(r2.comparacao_metadata_db, "igual");
    assert!(r2.biblioteca_intacta);
    // Nenhum byte, tamanho ou mtime mudou na Homologada (nem metadata.db, nem formatos).
    assert_eq!(arvore(&e.m.homologada), antes);
    // Perfil lê sem escrever.
    let p = bib(a.executar(Comando::BibliotecaPerfil {
        biblioteca: id.to_string(),
    }));
    assert!(matches!(p, DadosBiblioteca::Perfil { .. }));
    assert_eq!(arvore(&e.m.homologada), antes);
}

#[test]
fn ca_b04_duplicata_exata_sem_extracao_nem_ocr() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    // Cópia byte a byte de um formato da Homologada, e um livro novo.
    let original =
        e.m.homologada
            .join("Machado de Assis/Dom Casmurro (1)/Dom Casmurro.pdf");
    fs::copy(&original, e.m.downloads.join("dom_casmurro_baixado.pdf")).unwrap();
    fs::write(e.m.downloads.join("novo.pdf"), corpo(77, 30_000)).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    // Lista de duplicatas exatas (só índices).
    let DadosBiblioteca::Duplicatas {
        grupos,
        alertas_emitidos,
        ..
    } = bib(a.executar(Comando::BibliotecaDuplicatas {
        raiz: Some(e.m.downloads.to_str().unwrap().into()),
        limite: None,
        cursor: None,
    }))
    else {
        panic!()
    };
    assert_eq!(grupos.len(), 1);
    assert_eq!(grupos[0].copias.len(), 1);
    assert_eq!(alertas_emitidos, 1);
    assert_eq!(e.chamadas.load(Ordering::SeqCst), 0, "nenhuma extração/OCR");
    // Enfileira e processa: a duplicata sai antes de extrair; o livro novo é extraído.
    let enf = enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    assert_eq!(enf.enfileirados, 2);
    let r = processar(&mut a);
    assert_eq!((r.processados, r.duplicatas_exatas, r.extraidos), (2, 1, 1));
    assert_eq!(
        e.chamadas.load(Ordering::SeqCst),
        1,
        "só o livro novo foi extraído"
    );
    assert!(r.espaco_potencial_bytes > 0);
    let tipos: Vec<String> = alertas(&mut a).into_iter().map(|x| x.tipo).collect();
    assert!(tipos.iter().any(|t| t == "ja_tem_este_livro"), "{tipos:?}");
    assert!(tipos.iter().any(|t| t == "duplicata_exata"), "{tipos:?}");
}

#[test]
fn ca_b11_reprocessar_tres_vezes_reaproveita() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    fs::write(e.m.downloads.join("novo.pdf"), corpo(77, 30_000)).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r1 = processar(&mut a);
    assert_eq!((r1.extraidos, r1.reaproveitados), (1, 0));
    let sugestoes = |a: &mut Agente| match bib(a.executar(Comando::BibliotecaRevisao {
        limite: None,
        cursor: None,
    })) {
        DadosBiblioteca::Revisao {
            total_pendentes, ..
        } => total_pendentes,
        d => panic!("{d:?}"),
    };
    let pendentes = sugestoes(&mut a);
    assert!(pendentes >= 2);
    // Já processado e sem mudança: não volta como Novo.
    let enf = enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    assert_eq!((enf.enfileirados, enf.ignorados >= 1), (0, true));
    // Reprocessar 3 vezes: nenhuma extração nova, nenhuma sugestão duplicada.
    for _ in 0..3 {
        let enf = enfileirar(&mut a, &e.m.downloads, MotivoFila::Reprocessar);
        assert_eq!(enf.enfileirados, 1);
        let r = processar(&mut a);
        assert_eq!((r.extraidos, r.reaproveitados), (0, 1));
    }
    assert_eq!(e.chamadas.load(Ordering::SeqCst), 1);
    assert_eq!(sugestoes(&mut a), pendentes);
}

#[test]
fn ca_b13_processo_morto_retoma_do_catalogo() {
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("a.pdf"), corpo(10, 30_000)).unwrap();
    fs::write(e.m.downloads.join("b.pdf"), corpo(20, 30_000)).unwrap();
    {
        let mut a = e.agente(Autor::Gui);
        a.executar(scan(&e.m, ModoVarredura::FullScan));
        let id = registrar_homologada(&mut a, &e.m);
        sincronizar(&mut a, id);
        enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
        // Simula a morte do processo no meio: um job fica `em_andamento`, para sempre.
        let conn = Connection::open(&e.m.catalogo).unwrap();
        conn.execute(
            "UPDATE jobs_bibliograficos SET estado = 'em_andamento', etapa = 'extraindo', tentativas = 1
             WHERE id = (SELECT MIN(id) FROM jobs_bibliograficos)",
            [],
        )
        .unwrap();
    }
    // Novo processo: o órfão volta à fila e tudo termina, sem perder nem repetir trabalho.
    let mut a = e.agente(Autor::Gui);
    let r = processar(&mut a);
    assert_eq!(r.orfaos_retomados, 1);
    assert_eq!(r.processados, 2);
    assert_eq!(r.pendentes_restantes, 0);
    let DadosBiblioteca::Fila { contagem, .. } = bib(a.executar(Comando::BibliotecaFila {
        estado: Some(EstadoFilaInfo::Concluido),
        limite: None,
        cursor: None,
    })) else {
        panic!()
    };
    assert_eq!(
        (
            contagem.concluidos,
            contagem.pendentes,
            contagem.em_andamento
        ),
        (2, 0, 0)
    );
}

#[test]
fn ca_b12_part_debounce_e_conteudo_alterado() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id); // o vigia começa daqui
    // Download em andamento (.part), um livro pronto e uma cópia de livro já homologado.
    fs::write(e.m.downloads.join("livro.pdf.part"), corpo(5, 30_000)).unwrap();
    fs::write(e.m.downloads.join("pronto.pdf"), corpo(6, 30_000)).unwrap();
    let original =
        e.m.homologada
            .join("Machado de Assis/O Alienista (3)/O Alienista.pdf");
    fs::copy(&original, e.m.downloads.join("alienista.pdf")).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FastScan));
    // Debounce: eventos recentes esperam.
    let r0 = a.vigiar_biblioteca().unwrap();
    assert_eq!(r0.enfileirados, 0);
    let fila = |a: &mut Agente| match bib(a.executar(Comando::BibliotecaFila {
        estado: Some(EstadoFilaInfo::Pendente),
        limite: None,
        cursor: None,
    })) {
        DadosBiblioteca::Fila { contagem, .. } => contagem.pendentes,
        d => panic!("{d:?}"),
    };
    assert_eq!(fila(&mut a), 0);
    // Passado o debounce: o .part é ignorado; os outros dois entram; a cópia avisa na hora.
    e.avancar(120);
    let r1 = a.vigiar_biblioteca().unwrap();
    assert_eq!(r1.enfileirados, 2, "{r1:?}");
    assert_eq!(r1.alertas, 1, "{r1:?}");
    assert_eq!(fila(&mut a), 2);
    // O `.part` vira o livro de verdade (renomeado): a varredura já aciona o vigia, e como o
    // relógio está adiantado o evento é tratado ali mesmo.
    fs::rename(
        e.m.downloads.join("livro.pdf.part"),
        e.m.downloads.join("livro.pdf"),
    )
    .unwrap();
    a.executar(scan(&e.m, ModoVarredura::FastScan));
    assert_eq!(fila(&mut a), 3);
    // Processa tudo e depois muda o conteúdo de um: as sugestões do conteúdo antigo saem.
    processar(&mut a);
    assert_eq!(fila(&mut a), 0);
    let pendentes = |a: &mut Agente| match bib(a.executar(Comando::BibliotecaRevisao {
        limite: None,
        cursor: None,
    })) {
        DadosBiblioteca::Revisao {
            total_pendentes, ..
        } => total_pendentes,
        d => panic!("{d:?}"),
    };
    let antes = pendentes(&mut a);
    assert!(antes > 0);
    e.avancar(120);
    fs::write(e.m.downloads.join("pronto.pdf"), corpo(99, 31_000)).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FastScan));
    assert_eq!(fila(&mut a), 1, "o conteúdo mudou: volta à fila");
    assert!(
        pendentes(&mut a) < antes,
        "sugestões do conteúdo antigo foram desassociadas"
    );
}

#[test]
fn ca_b16_b17_descarte_bloqueado_sem_cobertura_e_sem_iniciativa_automatica() {
    let e = Ambiente::novo();
    let copia = e.m.downloads.join("copia.pdf");
    let original =
        e.m.homologada
            .join("Machado de Assis/Dom Casmurro (1)/Dom Casmurro.pdf");
    fs::copy(&original, &copia).unwrap();
    let estado_antes = arvore(&e.m.raiz);
    // Humano (GUI): sem Cobertura (nenhuma Marcação de Backup em outro disco), bloqueado.
    let mut a = e.agente(Autor::Gui);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    let pedir = |a: &mut Agente| {
        let DadosBiblioteca::Duplicatas { grupos, .. } =
            bib(a.executar(Comando::BibliotecaDuplicatas {
                raiz: Some(e.m.downloads.to_str().unwrap().into()),
                limite: None,
                cursor: None,
            }))
        else {
            panic!()
        };
        grupos
    };
    let g = pedir(&mut a);
    let c = &g[0].copias[0];
    assert!(!c.coberta);
    assert!(
        !c.decisao.sugerir,
        "sem Cobertura não sugere descarte: {:?}",
        c.decisao
    );
    assert!(!c.decisao.razoes.is_empty());
    assert_eq!(g[0].bytes_sugeridos, 0);
    drop(a);
    // MCP: iniciativa automática, sempre bloqueada, com a razão dita.
    let mut m = e.agente(Autor::Mcp);
    let g = pedir(&mut m);
    let c = &g[0].copias[0];
    assert!(!c.decisao.sugerir);
    assert!(
        c.decisao
            .razoes
            .iter()
            .any(|r| r.to_lowercase().contains("ia")
                || r.to_lowercase().contains("mcp")
                || r.to_lowercase().contains("autom")),
        "{:?}",
        c.decisao.razoes
    );
    // Nada foi apagado nem alterado: o DAG só leu (exceto o próprio Catálogo, fora da árvore).
    assert!(copia.exists());
    assert_eq!(arvore(&e.m.raiz), estado_antes);
}

#[test]
fn ca_b23_parcial_mcp_nao_aprova_homologacao() {
    let e = Ambiente::novo();
    let mut m = e.agente(Autor::Mcp);
    let r = m.executar(Comando::BibliotecaHomologar {
        item: 1,
        aprovacao: Some(AprovacaoHumana {
            quem: "mcp".into(),
            quando: None,
            motivo: None,
        }),
    });
    match r {
        Resposta::Erro { codigo, .. } => assert_eq!(codigo, "aprovacao_humana_exigida"),
        outra => panic!("{outra:?}"),
    }
    // O MCP também não consegue a aprovação disfarçada de proposta: sem cadastro, nada acontece.
    let r = m.executar(Comando::BibliotecaHomologar {
        item: 1,
        aprovacao: None,
    });
    assert!(matches!(r, Resposta::Erro { .. }));
}

#[test]
fn configuracao_aplica_patch_e_recusa_ia_remota() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"usar_unshare": true, "ia": {"ativa": true}}),
    });
    let DadosBiblioteca::Config { config } = bib(r) else {
        panic!()
    };
    assert!(config.usar_unshare && config.ia.ativa);
    assert_eq!(config.ia.modelo_primario, "gemma3:12b");
    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"ia": {"endpoint": "http://exemplo.com:11434"}}),
    });
    assert!(matches!(r, Resposta::Erro { .. }));
}

#[test]
fn config_recusa_endpoint_que_so_parece_loopback() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    for ruim in [
        "http://127.0.0.1@evil.com:11434",
        "http://localhost.evil.com",
        "http://127.0.0.10.evil.com",
        "http://[::1]@evil.com",
    ] {
        let r = a.executar(Comando::BibliotecaConfigDefinir {
            alteracoes: serde_json::json!({"ia": {"ativa": true, "endpoint": ruim}}),
        });
        assert!(matches!(r, Resposta::Erro { .. }), "{ruim} foi aceito");
    }
    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"ia": {"ativa": true, "endpoint": "http://[::1]:11434"}}),
    });
    assert!(matches!(r, Resposta::Biblioteca { .. }));
}

#[test]
fn calibredb_real_cadastra_na_a_classificar_sem_tocar_a_homologada() {
    if dag_calibre::executor::ExecutorProcesso::disponivel().is_none() {
        eprintln!("AVISO: teste pulado, calibredb ausente");
        return;
    }
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    fs::write(e.m.downloads.join("novo.pdf"), corpo(42, 30_000)).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    let antes = arvore(&e.m.homologada);
    let destino = e.m._tmp.path().join("A_Classificar");
    let r = a.executar(Comando::BibliotecaRegistrar {
        caminho: destino.to_str().unwrap().into(),
        papel: PapelRegistro::AClassificar,
        nome: None,
    });
    match &r {
        Resposta::Biblioteca { .. } => {}
        Resposta::Erro { codigo, mensagem } => {
            eprintln!("AVISO: calibredb não pôde criar a A Classificar ({codigo}): {mensagem}");
            assert_eq!(arvore(&e.m.homologada), antes);
            return;
        }
        outra => panic!("{outra:?}"),
    }
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    let file_id: i64 = Connection::open(&e.m.catalogo)
        .unwrap()
        .query_row(
            "SELECT id FROM files WHERE name_display = 'novo.pdf'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    match a.executar(Comando::BibliotecaCadastrar {
        itens: vec![file_id],
    }) {
        Resposta::Biblioteca {
            dados:
                DadosBiblioteca::Cadastrada {
                    resultados,
                    bloqueado,
                },
        } => {
            if let Some(b) = bloqueado {
                eprintln!("AVISO: Calibre ocupado, cadastro recusado: {b}");
            } else {
                assert_eq!(resultados[0].estado, "cadastrado", "{resultados:?}");
            }
        }
        Resposta::Erro { codigo, .. } => assert_eq!(codigo, "calibre_ocupado"),
        outra => panic!("{outra:?}"),
    }
    // A Homologada continua exatamente como estava.
    assert_eq!(arvore(&e.m.homologada), antes);
}

// ---------------------------------------------------------------------------------------------
// Fase 3 (agente): reconciliação, vocabulário, IA com contexto, compactados, fila, Homologada,
// senha do servidor
// ---------------------------------------------------------------------------------------------

use dag_agent::SaidaMembro;
use dag_biblio_ia::{
    EntradaClassificacao, Metricas, PapelModelo, RegistroInferencia, ResultadoClassificacao,
};
use dag_core::{AcaoRevisao, AlvoRevisao, ItemDecisao, ItemRevisaoInfo};

type Roteiro = dyn Fn(&str) -> Vec<(String, Vec<ValorSugerido>)> + Send + Sync;

/// Extrator de roteiro: o teste diz, por nome de arquivo, quais livros (e metadados) saem.
struct ExtratorRoteiro {
    f: Box<Roteiro>,
    capa: Option<Vec<u8>>,
    paginas: Vec<dag_biblio::PaginaTexto>,
}

impl ExtratorRoteiro {
    fn novo(
        f: impl Fn(&str) -> Vec<(String, Vec<ValorSugerido>)> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            f: Box::new(f),
            capa: None,
            paginas: Vec::new(),
        })
    }
}

fn saida(
    metadados: Vec<ValorSugerido>,
    capa: Option<Vec<u8>>,
    paginas: Vec<dag_biblio::PaginaTexto>,
) -> SaidaExtracao {
    SaidaExtracao {
        resultado: ResultadoExtracao {
            formato: FormatoLivro::Pdf,
            estado: EstadoLeitura::Lido,
            metadados,
            isbns: Vec::new(),
            sumario: Vec::new(),
            paginas,
            total_paginas: Some(10),
            ficha_catalografica: None,
            capa: None,
            capa_mime: None,
            paginas_ocr: Vec::new(),
            avisos: Vec::new(),
        },
        paginas_ocr: 0,
        capa,
        avisos: Vec::new(),
    }
}

impl ExtratorLivro for ExtratorRoteiro {
    fn nome(&self) -> String {
        "roteiro".into()
    }

    fn extrair(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        Ok(self.extrair_membros(caminho, formato, cfg)?.remove(0).saida)
    }

    fn extrair_membros(
        &self,
        caminho: &Path,
        formato: FormatoLivro,
        _cfg: &ConfigBiblioteca,
    ) -> Result<Vec<SaidaMembro>, String> {
        let nome = caminho.file_name().unwrap().to_string_lossy().into_owned();
        Ok((self.f)(&nome)
            .into_iter()
            .map(|(membro, valores)| SaidaMembro {
                caminho_interno: membro,
                formato: if formato == FormatoLivro::Outro {
                    FormatoLivro::Epub
                } else {
                    formato
                },
                tamanho: 0,
                saida: saida(valores, self.capa.clone(), self.paginas.clone()),
            })
            .collect())
    }
}

fn texto_v(campo: Campo, valor: &str, origem: OrigemEvidencia, c: f32) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: Some(valor.into()),
        lista: Vec::new(),
        confianca: c,
        evidencias: vec![Evidencia {
            origem,
            pagina: None,
            trecho: Some(valor.into()),
            detalhe: None,
            confianca: c,
        }],
    }
}

fn lista_v(campo: Campo, lista: &[&str], origem: OrigemEvidencia, c: f32) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: None,
        lista: lista.iter().map(|s| (*s).to_owned()).collect(),
        confianca: c,
        evidencias: vec![Evidencia {
            origem,
            pagina: None,
            trecho: Some(lista.join(" & ")),
            detalhe: None,
            confianca: c,
        }],
    }
}

impl Ambiente {
    fn agente_com(&self, autor: Autor, ext: Arc<dyn ExtratorLivro>) -> Agente {
        self.agente(autor).com_extrator_livro(ext)
    }
}

fn revisao(a: &mut Agente) -> Vec<ItemRevisaoInfo> {
    match bib(a.executar(Comando::BibliotecaRevisao {
        limite: Some(500),
        cursor: None,
    })) {
        DadosBiblioteca::Revisao { itens, .. } => itens,
        d => panic!("{d:?}"),
    }
}

fn item_de<'a>(itens: &'a [ItemRevisaoInfo], sufixo: &str) -> &'a ItemRevisaoInfo {
    itens
        .iter()
        .find(|i| i.caminho.as_deref().is_some_and(|c| c.ends_with(sufixo)))
        .unwrap_or_else(|| panic!("item {sufixo} não está na revisão: {itens:#?}"))
}

fn campo<'a>(i: &'a ItemRevisaoInfo, nome: &str) -> Option<&'a dag_core::CampoLadoALado> {
    i.campos.iter().find(|c| c.campo == nome)
}

fn decidir(a: &mut Agente, d: Vec<ItemDecisao>) {
    match bib(a.executar(Comando::BibliotecaDecidir {
        itens: d,
        lote: None,
    })) {
        DadosBiblioteca::Decidida {
            aplicadas,
            ignoradas,
            ..
        } => {
            assert!(ignoradas.is_empty(), "{ignoradas:?}");
            assert!(aplicadas >= 1);
        }
        d => panic!("{d:?}"),
    }
}

/// Acrescenta um livro à Homologada sintética (metadata.db + arquivo no disco), com tags.
fn adicionar_livro(m: &Mundo, id: i64, titulo: &str, autor: &str, fmt: &str, tags: &[&str]) {
    let conn = Connection::open(m.homologada.join("metadata.db")).unwrap();
    let pasta = format!("{autor}/{titulo} ({id})");
    conn.execute(
        "INSERT INTO books(id,title,sort,pubdate,series_index,author_sort,path,uuid,has_cover,last_modified)
         VALUES (?1,?2,?2,'0101-01-01 00:00:00+00:00',1.0,?3,?4,?5,0,'2024-01-01 00:00:00+00:00')",
        params![id, titulo, autor, pasta, format!("uuid-{id}")],
    )
    .unwrap();
    let autor_id: i64 =
        match conn.query_row("SELECT id FROM authors WHERE name = ?1", [autor], |l| {
            l.get(0)
        }) {
            Ok(i) => i,
            Err(_) => {
                conn.execute("INSERT INTO authors(name,sort) VALUES (?1,?1)", [autor])
                    .unwrap();
                conn.last_insert_rowid()
            }
        };
    conn.execute(
        "INSERT INTO books_authors_link(book,author) VALUES (?1,?2)",
        params![id, autor_id],
    )
    .unwrap();
    for t in tags {
        let tid: i64 =
            match conn.query_row("SELECT id FROM tags WHERE name = ?1", [t], |l| l.get(0)) {
                Ok(i) => i,
                Err(_) => {
                    conn.execute("INSERT INTO tags(name) VALUES (?1)", [t])
                        .unwrap();
                    conn.last_insert_rowid()
                }
            };
        conn.execute(
            "INSERT INTO books_tags_link(book,tag) VALUES (?1,?2)",
            params![id, tid],
        )
        .unwrap();
    }
    let dados = corpo(100 + id as u8, 20_000 + id as usize * 100);
    conn.execute(
        "INSERT INTO data(book,format,uncompressed_size,name) VALUES (?1,?2,?3,?4)",
        params![id, fmt, dados.len() as i64, titulo],
    )
    .unwrap();
    let dir = m.homologada.join(&pasta);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{titulo}.{}", fmt.to_lowercase())), &dados).unwrap();
}

/// Etiqueta livros que já existem em `mundo()`.
fn etiquetar(m: &Mundo, livro: i64, tag: &str) {
    let conn = Connection::open(m.homologada.join("metadata.db")).unwrap();
    let tid: i64 = match conn.query_row("SELECT id FROM tags WHERE name = ?1", [tag], |l| l.get(0))
    {
        Ok(i) => i,
        Err(_) => {
            conn.execute("INSERT INTO tags(name) VALUES (?1)", [tag])
                .unwrap();
            conn.last_insert_rowid()
        }
    };
    conn.execute(
        "INSERT INTO books_tags_link(book,tag) VALUES (?1,?2)",
        params![livro, tid],
    )
    .unwrap();
}

#[test]
fn reconciliacao_com_divergencia_vai_para_revisao_e_humano_vence() {
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("div.pdf"), corpo(5, 30_000)).unwrap();
    let ext = ExtratorRoteiro::novo(|_| {
        vec![(
            String::new(),
            vec![
                texto_v(
                    Campo::Titulo,
                    "Cálculo Volume Um",
                    OrigemEvidencia::MetadadoInterno,
                    0.9,
                ),
                texto_v(
                    Campo::Titulo,
                    "Álgebra Linear Aplicada",
                    OrigemEvidencia::Conteudo,
                    0.9,
                ),
                lista_v(
                    Campo::Autores,
                    &["Maria Exemplo"],
                    OrigemEvidencia::MetadadoInterno,
                    0.9,
                ),
            ],
        )]
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);

    let itens = revisao(&mut a);
    let it = item_de(&itens, "div.pdf");
    let t = campo(it, "titulo").expect("título sugerido");
    // Fontes fortes divergentes: a de maior evidência vence, mas a confiança cai e vai para revisão.
    assert_eq!(t.sugerido.as_deref(), Some("Álgebra Linear Aplicada"));
    assert!(t.precisa_revisao, "{t:?}");
    assert!(
        t.motivos.iter().any(|m| m.contains("divergem")),
        "{:?}",
        t.motivos
    );
    assert!(t.confianca.unwrap() <= 0.75, "{:?}", t.confianca);
    assert!(
        t.motivos.iter().any(|m| m.contains("Cálculo Volume Um")),
        "a alternativa que perdeu aparece: {:?}",
        t.motivos
    );

    // O humano corrige: a decisão vence qualquer reprocessamento, sem pedir de novo.
    decidir(
        &mut a,
        vec![ItemDecisao {
            alvo: AlvoRevisao::Arquivo(it.file_id),
            campo: "titulo".into(),
            acao: AcaoRevisao::Corrigir,
            valor: Some("Cálculo I".into()),
        }],
    );
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Reprocessar);
    processar(&mut a);
    let itens = revisao(&mut a);
    if let Some(it) = itens.iter().find(|i| i.file_id == it.file_id) {
        assert!(
            campo(it, "titulo").is_none_or(|c| c.estado != "pendente"),
            "título decidido não volta como pendente: {it:#?}"
        );
    }
    drop(a);
    let conn = Connection::open(&e.m.catalogo).unwrap();
    let estado: String = conn
        .query_row(
            "SELECT estado FROM sugestoes WHERE campo = 'titulo' AND estado = 'corrigida'",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(estado, "corrigida");
    let valor: String = conn
        .query_row(
            "SELECT valor_novo FROM decisoes_humanas WHERE campo = 'titulo' AND desfeita_em IS NULL",
            [],
            |l| l.get(0),
        )
        .unwrap();
    assert_eq!(valor, "Cálculo I");
}

#[test]
fn correcao_humana_muda_a_proxima_sugestao_de_tags() {
    let e = Ambiente::novo();
    etiquetar(&e.m, 1, "Literatura");
    etiquetar(&e.m, 2, "Literatura");
    etiquetar(&e.m, 3, "Literatura");
    adicionar_livro(
        &e.m,
        4,
        "Termodinâmica Clássica",
        "Callen",
        "PDF",
        &["Matemática"],
    );
    adicionar_livro(
        &e.m,
        5,
        "Cálculo Diferencial",
        "Stewart",
        "PDF",
        &["Matemática"],
    );
    fs::write(e.m.downloads.join("quantica_a.pdf"), corpo(11, 30_000)).unwrap();
    fs::write(e.m.downloads.join("quantica_b.pdf"), corpo(12, 30_000)).unwrap();
    let ext = ExtratorRoteiro::novo(|nome| {
        let t = if nome.starts_with("quantica_a") {
            "Mecânica Quântica Aplicada"
        } else {
            "Mecânica Quântica Avançada"
        };
        vec![(
            String::new(),
            vec![
                texto_v(Campo::Titulo, t, OrigemEvidencia::MetadadoInterno, 0.9),
                lista_v(
                    Campo::Autores,
                    &["Fulano Quântico"],
                    OrigemEvidencia::MetadadoInterno,
                    0.9,
                ),
            ],
        )]
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);

    // ANTES: nada na Homologada liga esses títulos a Matemática.
    let itens = revisao(&mut a);
    let tem_mat = |i: &ItemRevisaoInfo| {
        campo(i, "tags").is_some_and(|c| c.sugerido_lista.iter().any(|t| t == "Matemática"))
    };
    assert!(
        !tem_mat(item_de(&itens, "quantica_a.pdf")),
        "{:#?}",
        campo(item_de(&itens, "quantica_a.pdf"), "tags")
    );
    assert!(!tem_mat(item_de(&itens, "quantica_b.pdf")));

    // O humano corrige as tags do A.
    let fid_a = item_de(&itens, "quantica_a.pdf").file_id;
    decidir(
        &mut a,
        vec![ItemDecisao {
            alvo: AlvoRevisao::Arquivo(fid_a),
            campo: "tags".into(),
            acao: AcaoRevisao::Corrigir,
            valor: Some("Matemática".into()),
        }],
    );

    // DEPOIS: o B, parecido com o A, passa a ser sugerido com a tag que o humano escolheu (Matemática), e a
    // sugestão diz por quê (WOW-03): parecido com a decisão dele.
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Reprocessar);
    processar(&mut a);
    let itens = revisao(&mut a);
    let b = item_de(&itens, "quantica_b.pdf");
    let tags = campo(b, "tags").expect("tags sugeridas depois da correção");
    assert!(
        tags.sugerido_lista.iter().any(|t| t == "Matemática"),
        "{tags:?}"
    );
    assert!(
        tags.evidencias.iter().any(|x| x.contains("decisão sua")),
        "a sugestão cita a decisão humana: {:?}",
        tags.evidencias
    );
}

#[test]
fn compactado_com_dois_livros_gera_dois_itens_ligados_ao_conteiner() {
    let e = Ambiente::novo();
    // Um zip real com dois e-books dentro (a etapa de Contêineres da Fase A os cataloga).
    let mut zip_bytes = Vec::new();
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("colecao/primeiro.epub", o).unwrap();
        z.write_all(&corpo(31, 3000)).unwrap();
        z.start_file("colecao/segundo.epub", o).unwrap();
        z.write_all(&corpo(32, 3100)).unwrap();
        z.finish().unwrap();
    }
    fs::write(e.m.downloads.join("pacote.zip"), &zip_bytes).unwrap();
    let ext = ExtratorRoteiro::novo(|_| {
        ["colecao/primeiro.epub", "colecao/segundo.epub"]
            .iter()
            .map(|m| {
                let t = if m.contains("primeiro") {
                    "O Primeiro Livro"
                } else {
                    "O Segundo Tratado"
                };
                (
                    (*m).to_owned(),
                    vec![
                        texto_v(Campo::Titulo, t, OrigemEvidencia::MetadadoInterno, 0.9),
                        lista_v(
                            Campo::Autores,
                            &["Beltrano Coletor"],
                            OrigemEvidencia::MetadadoInterno,
                            0.9,
                        ),
                    ],
                )
            })
            .collect()
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    let enf = enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    assert_eq!(enf.enfileirados, 1, "{enf:?}");
    let r = processar(&mut a);
    assert_eq!(r.membros_de_conteiner, 2, "{r:?}");

    let itens = revisao(&mut a);
    let do_zip: Vec<&ItemRevisaoInfo> = itens
        .iter()
        .filter(|i| {
            i.caminho
                .as_deref()
                .is_some_and(|c| c.ends_with("pacote.zip"))
        })
        .collect();
    assert_eq!(do_zip.len(), 2, "{itens:#?}");
    assert_eq!(do_zip[0].file_id, do_zip[1].file_id, "mesmo Contêiner");
    let mut membros: Vec<&str> = do_zip.iter().filter_map(|i| i.membro.as_deref()).collect();
    membros.sort_unstable();
    assert_eq!(membros, ["colecao/primeiro.epub", "colecao/segundo.epub"]);
    let titulo_de = |m: &str| {
        let i = do_zip
            .iter()
            .find(|i| i.membro.as_deref() == Some(m))
            .unwrap();
        campo(i, "titulo").unwrap().sugerido.clone().unwrap()
    };
    assert_eq!(titulo_de("colecao/primeiro.epub"), "O Primeiro Livro");
    assert_eq!(titulo_de("colecao/segundo.epub"), "O Segundo Tratado");

    // Decidir um Membro (pela Edição dele) não mexe no outro.
    let p = do_zip
        .iter()
        .find(|i| i.membro.as_deref() == Some("colecao/primeiro.epub"))
        .unwrap();
    decidir(
        &mut a,
        vec![ItemDecisao {
            alvo: AlvoRevisao::Edicao(p.edicao_id.unwrap()),
            campo: "titulo".into(),
            acao: AcaoRevisao::Aceitar,
            valor: None,
        }],
    );
    let itens = revisao(&mut a);
    let do_zip: Vec<&ItemRevisaoInfo> = itens
        .iter()
        .filter(|i| {
            i.caminho
                .as_deref()
                .is_some_and(|c| c.ends_with("pacote.zip"))
        })
        .collect();
    let estado = |m: &str| {
        let i = do_zip
            .iter()
            .find(|i| i.membro.as_deref() == Some(m))
            .unwrap();
        campo(i, "titulo").unwrap().estado.clone()
    };
    assert_eq!(estado("colecao/segundo.epub"), "pendente");
    assert_ne!(estado("colecao/primeiro.epub"), "pendente");
    // Reprocessar não duplica nada.
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Reprocessar);
    let r = processar(&mut a);
    assert_eq!(r.membros_de_conteiner, 2);
    assert_eq!(
        e.chamadas.load(Ordering::SeqCst),
        0,
        "extrator de roteiro não conta"
    );
}

#[test]
fn aging_nao_deixa_arquivo_pequeno_morrer_de_fome() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    let grande = |nome: &str, marca: u8| {
        let f = fs::File::create(e.m.downloads.join(nome)).unwrap();
        f.set_len(400 * 1024 * 1024).unwrap(); // esparso: não ocupa disco
        use std::os::unix::fs::FileExt;
        f.write_all_at(&[marca; 64], 1000).unwrap();
    };
    grande("grande_1.pdf", 1);
    fs::write(e.m.downloads.join("pequeno.pdf"), corpo(9, 30_000)).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let ordem = |a: &mut Agente| -> Vec<String> {
        match bib(a.executar(Comando::BibliotecaFila {
            estado: Some(EstadoFilaInfo::Pendente),
            limite: Some(50),
            cursor: None,
        })) {
            DadosBiblioteca::Fila { itens, .. } => itens
                .into_iter()
                .map(|i| {
                    Path::new(&i.caminho.unwrap())
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            d => panic!("{d:?}"),
        }
    };
    // No começo o grande (mais economia potencial) sai na frente do pequeno.
    assert_eq!(ordem(&mut a), ["grande_1.pdf", "pequeno.pdf"]);

    // 20 horas depois chega outro grande. Sem aging o pequeno ficaria para sempre atrás dos
    // grandes que vão chegando; com aging ele passa o recém-chegado.
    e.avancar(20 * 3600);
    grande("grande_2.pdf", 2);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    assert_eq!(
        ordem(&mut a),
        ["grande_1.pdf", "pequeno.pdf", "grande_2.pdf"],
        "o pequeno que espera há 20 h passa o grande recém-chegado"
    );

    // O aging é configurável.
    drop(a);
    let mut b = e.agente(Autor::Gui);
    let r = b.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"aging_por_hora": 0.0}),
    });
    let DadosBiblioteca::Config { config } = bib(r) else {
        panic!()
    };
    assert_eq!(config.aging_por_hora, 0.0, "o valor pedido é guardado");
    let r = b.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"aging_por_hora": -1.0}),
    });
    assert!(matches!(r, Resposta::Erro { .. }));
}

#[test]
fn item_da_homologada_com_catalogar_vira_proposta_sem_escrever_nela() {
    let e = Ambiente::novo();
    adicionar_livro(
        &e.m,
        4,
        "Termodinâmica Clássica",
        "Autor Físico",
        "PDF",
        &["Física"],
    );
    adicionar_livro(
        &e.m,
        5,
        "Mecânica Clássica",
        "Autor Físico",
        "PDF",
        &["Física"],
    );
    adicionar_livro(
        &e.m,
        6,
        "Eletromagnetismo Clássico",
        "Autor Físico",
        "PDF",
        &["!Catalogar"],
    );
    let mut a = e.agente(Autor::Gui);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    let s = sincronizar(&mut a, id);
    assert!(s.biblioteca_intacta);
    let antes = arvore(&e.m.homologada);
    let r = processar(&mut a);
    assert!(r.propostas_homologada >= 1, "{r:?}");

    let itens = revisao(&mut a);
    let it = itens
        .iter()
        .find(|i| {
            i.proposta_homologada
                .as_ref()
                .is_some_and(|l| l.titulo == "Eletromagnetismo Clássico")
        })
        .unwrap_or_else(|| panic!("sem proposta para o livro !Catalogar: {itens:#?}"));
    let tags = campo(it, "tags").expect("tags propostas");
    assert!(
        tags.sugerido_lista.iter().any(|t| t == "Física"),
        "{tags:?}"
    );
    assert!(
        tags.motivos.iter().any(|m| m.contains("nada é escrito")),
        "{:?}",
        tags.motivos
    );
    // O livro que já tem tag (Física) não vira proposta.
    assert!(!itens.iter().any(|i| {
        i.proposta_homologada
            .as_ref()
            .is_some_and(|l| l.titulo == "Mecânica Clássica")
    }));
    // Nada foi escrito na Homologada (nem no metadata.db).
    assert_eq!(arvore(&e.m.homologada), antes);
    // Reprocessar não repete a proposta.
    let r2 = processar(&mut a);
    assert_eq!(r2.propostas_homologada, 0);
}

#[test]
fn senha_do_servidor_vai_ao_cofre_e_nunca_volta() {
    use bancada_segredos::testes::ChaveiroEmMemoria;
    const SENHA: &str = "S3nh@-Do-Servidor-9z";
    let e = Ambiente::novo();
    let kr = Arc::new(ChaveiroEmMemoria::novo());
    let mut a = e.agente(Autor::Mcp).com_chaveiro(kr);
    let ser = |r: &Resposta| serde_json::to_string(r).unwrap();

    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"homologacao": {
            "modo": "servidor", "url": "http://127.0.0.1:17790", "usuario": "dag", "senha": SENHA}}),
    });
    assert!(!ser(&r).contains(SENHA) && !format!("{r:?}").contains(SENHA));
    let DadosBiblioteca::Config { config } = bib(r) else {
        panic!()
    };
    assert!(
        config.homologacao.senha_no_cofre,
        "informa que há senha, sem revelá-la"
    );

    let r = a.executar(Comando::BibliotecaConfigObter);
    assert!(!ser(&r).contains(SENHA), "{}", ser(&r));
    let DadosBiblioteca::Config { config } = bib(r) else {
        panic!()
    };
    assert!(config.homologacao.senha_no_cofre);
    assert_eq!(config.homologacao.usuario.as_deref(), Some("dag"));

    // Em disco: nem a configuração nem o arquivo do Catálogo carregam a senha em claro.
    let conn = Connection::open(&e.m.catalogo).unwrap();
    let valores: Vec<String> = conn
        .prepare("SELECT valor FROM configuracoes")
        .unwrap()
        .query_map([], |l| l.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(valores.iter().any(|v| v.contains("servidor")));
    assert!(valores.iter().all(|v| !v.contains(SENHA)), "{valores:?}");
    drop(conn);
    for sufixo in ["", "-wal"] {
        let p = format!("{}{sufixo}", e.m.catalogo.display());
        if let Ok(b) = fs::read(&p) {
            assert!(
                !b.windows(SENHA.len()).any(|w| w == SENHA.as_bytes()),
                "a senha apareceu em {p}"
            );
        }
    }

    // Um patch inválido que traga a senha também não a devolve na mensagem de erro.
    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"homologacao": {"modo": "inexistente", "senha": SENHA}}),
    });
    assert!(matches!(r, Resposta::Erro { .. }), "{r:?}");
    assert!(!ser(&r).contains(SENHA) && !format!("{r:?}").contains(SENHA));

    // Apagar a senha: patch com `senha` vazia.
    let r = a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"homologacao": {"senha": ""}}),
    });
    let DadosBiblioteca::Config { config } = bib(r) else {
        panic!()
    };
    assert!(!config.homologacao.senha_no_cofre);

    // Sem chaveiro utilizável, recusa com erro claro (e sem a senha).
    drop(a);
    let mut b = e
        .agente(Autor::Gui)
        .com_chaveiro(Arc::new(ChaveiroEmMemoria::indisponivel()));
    let r = b.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"homologacao": {"senha": SENHA}}),
    });
    match &r {
        Resposta::Erro { codigo, mensagem } => {
            assert_eq!(codigo, "cofre_indisponivel");
            assert!(!mensagem.contains(SENHA));
        }
        outra => panic!("{outra:?}"),
    }
}

#[test]
fn ia_recebe_contexto_e_as_inferencias_guardam_tokens_e_tempo_reais() {
    use std::sync::Mutex;
    struct IaEspia {
        vista: Arc<Mutex<Option<EntradaClassificacao>>>,
    }
    impl dag_agent::ClassificadorIa for IaEspia {
        fn nome(&self) -> String {
            "espia:1b".into()
        }

        fn classificar(
            &self,
            entrada: &EntradaClassificacao,
        ) -> Result<ResultadoClassificacao, String> {
            *self.vista.lock().unwrap() = Some(entrada.clone());
            Ok(ResultadoClassificacao {
                valores: vec![lista_v(
                    Campo::Tags,
                    &["Literatura"],
                    OrigemEvidencia::Ia,
                    0.8,
                )],
                registros: vec![RegistroInferencia {
                    papel: PapelModelo::Primario,
                    modelo: "espia:1b".into(),
                    versao_modelo: "espia:1b-q4".into(),
                    info_servidor: None,
                    etapa: "texto".into(),
                    versao_prompt: "p1".into(),
                    hash_prompt: "não-é-hex-de-32-bytes".into(),
                    seed: 1,
                    temperatura: 0.0,
                    metricas: Metricas {
                        tokens_entrada: Some(1234),
                        tokens_saida: Some(56),
                        ..Metricas::default()
                    },
                    tempo_ms: 789,
                    resposta_bruta: None,
                    erro: None,
                    decisao: None,
                    material_truncado: false,
                    suspeitas_injecao: Vec::new(),
                    inicio_unix_ms: 0,
                }],
                ..ResultadoClassificacao::default()
            })
        }
    }
    let e = Ambiente::novo();
    etiquetar(&e.m, 1, "Literatura");
    etiquetar(&e.m, 2, "Literatura");
    fs::write(e.m.downloads.join("ia.pdf"), corpo(21, 30_000)).unwrap();
    let mut ext = ExtratorRoteiro {
        f: Box::new(|_| {
            vec![(
                String::new(),
                vec![
                    texto_v(
                        Campo::Titulo,
                        "Quincas Borba",
                        OrigemEvidencia::MetadadoInterno,
                        0.9,
                    ),
                    lista_v(
                        Campo::Autores,
                        &["Machado de Assis"],
                        OrigemEvidencia::MetadadoInterno,
                        0.9,
                    ),
                ],
            )]
        }),
        capa: None,
        paginas: Vec::new(),
    };
    ext.capa = Some(vec![1, 2, 3, 4]);
    let vista = Arc::new(Mutex::new(None));
    let mut a = e
        .agente_com(Autor::Gui, Arc::new(ext))
        .com_classificador_ia(Arc::new(IaEspia {
            vista: Arc::clone(&vista),
        }));
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r = match bib(a.executar(Comando::BibliotecaProcessar {
        limite: None,
        ia: true,
    })) {
        DadosBiblioteca::Processada { resumo } => resumo,
        d => panic!("{d:?}"),
    };
    assert_eq!(r.consultas_ia, 1, "{r:?}");

    let ent = vista.lock().unwrap().clone().expect("a IA foi consultada");
    assert!(
        ent.vocabulario.tags.iter().any(|t| t == "Literatura"),
        "{:?}",
        ent.vocabulario
    );
    assert!(
        ent.vocabulario
            .autores_parecidos
            .iter()
            .any(|x| x == "Machado de Assis"),
        "{:?}",
        ent.vocabulario.autores_parecidos
    );
    assert!(ent.vocabulario.estantes.len() <= 40);
    assert_eq!(
        ent.imagem_pagina1.as_deref(),
        Some(&[1u8, 2, 3, 4][..]),
        "capa para a visão"
    );
    assert!(ent.nome_arquivo.ends_with("ia.pdf"));

    let conn = Connection::open(&e.m.catalogo).unwrap();
    let (m, te, ts, ms): (String, i64, i64, i64) = conn
        .query_row(
            "SELECT modelo, tokens_entrada, tokens_saida, tempo_ms FROM inferencias",
            [],
            |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?, l.get(3)?)),
        )
        .unwrap();
    assert_eq!((m.as_str(), te, ts, ms), ("espia:1b", 1234, 56, 789));

    // Autor igual ao da Homologada, sem outra evidência: identidade não confirmada, revisão.
    let itens = revisao(&mut a);
    let autores = campo(item_de(&itens, "ia.pdf"), "autores").unwrap();
    assert_eq!(autores.sugerido_lista, ["Machado de Assis"], "{autores:?}");
    assert!(
        autores.homologado.is_some() || !autores.evidencias.is_empty(),
        "{autores:?}"
    );
}

#[test]
fn resumo_do_periodo_conta_jobs_classificados_e_duplicatas() {
    let e = Ambiente::novo();
    let mut a = e.agente(Autor::Gui);
    fs::write(e.m.downloads.join("novo.pdf"), corpo(77, 30_000)).unwrap();
    // Cópia exata de um livro homologado: duplicata.
    let copia = fs::read(
        e.m.homologada
            .join("Machado de Assis/Dom Casmurro (1)/Dom Casmurro.pdf"),
    )
    .unwrap();
    fs::write(e.m.downloads.join("copia.pdf"), copia).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    let DadosBiblioteca::Resumo { resumo } =
        bib(a.executar(Comando::BibliotecaResumo { desde: None }))
    else {
        panic!()
    };
    assert_eq!(resumo.jobs_concluidos_periodo, 2, "{resumo:?}");
    assert_eq!(resumo.classificados_periodo, 1, "{resumo:?}");
    assert!(resumo.revisoes_pendentes >= 2);
    assert_eq!(resumo.duplicatas_exatas_periodo, 1, "{resumo:?}");
    assert!(resumo.duplicatas_periodo >= 1);
}

#[test]
fn revisao_traz_paginas_resumidas_e_marca_de_ocr_nos_avisos() {
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("imagem.pdf"), corpo(41, 30_000)).unwrap();
    fs::write(e.m.downloads.join("texto.pdf"), corpo(42, 30_000)).unwrap();
    let mut ext = ExtratorRoteiro {
        f: Box::new(|nome| {
            vec![(
                String::new(),
                vec![texto_v(
                    Campo::Titulo,
                    &format!("Livro {nome}"),
                    OrigemEvidencia::MetadadoInterno,
                    0.9,
                )],
            )]
        }),
        capa: None,
        paginas: Vec::new(),
    };
    ext.paginas = vec![dag_biblio::PaginaTexto {
        numero: 3,
        texto: "Capítulo   um\n".repeat(40),
        via_ocr: true,
        qualidade: 0.8,
        idioma_ocr: Some("por".into()),
    }];
    let mut a = e.agente_com(Autor::Gui, Arc::new(ext));
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    let itens = revisao(&mut a);
    let it = item_de(&itens, "imagem.pdf");
    assert_eq!(it.paginas_ocr, 1);
    assert_eq!(it.paginas.len(), 1);
    assert!(it.paginas[0].via_ocr && it.paginas[0].numero == 3);
    assert!(
        it.paginas[0].trecho.chars().count() <= 160
            && it.paginas[0].trecho.starts_with("Capítulo um")
    );
    assert!(
        it.avisos.iter().any(|a| a.starts_with("ocr:")),
        "{:?}",
        it.avisos
    );
}

#[test]
fn cadastros_lista_prontos_so_depois_da_revisao_e_e_somente_leitura() {
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("pronto.pdf"), corpo(51, 30_000)).unwrap();
    let ext = ExtratorRoteiro::novo(|_| {
        vec![(
            String::new(),
            vec![
                texto_v(
                    Campo::Titulo,
                    "Livro Pronto",
                    OrigemEvidencia::MetadadoInterno,
                    0.95,
                ),
                lista_v(
                    Campo::Autores,
                    &["Autora Nova"],
                    OrigemEvidencia::MetadadoInterno,
                    0.95,
                ),
            ],
        )]
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    let cmd = Comando::BibliotecaCadastros {
        estado: None,
        limite: None,
        cursor: None,
    };
    assert!(dag_agent::somente_leitura(&cmd));
    let listar = |a: &mut Agente| match bib(a.executar(Comando::BibliotecaCadastros {
        estado: None,
        limite: None,
        cursor: None,
    })) {
        DadosBiblioteca::Cadastros { itens, .. } => itens,
        d => panic!("{d:?}"),
    };
    let antes = listar(&mut a);
    assert_eq!(antes.len(), 1, "{antes:?}");
    assert!(
        !antes[0].pronto_para_cadastrar,
        "revisão pendente: {antes:?}"
    );

    let itens = revisao(&mut a);
    let it = item_de(&itens, "pronto.pdf");
    let decisoes = it
        .campos
        .iter()
        .filter(|c| c.estado == "pendente")
        .map(|c| ItemDecisao {
            alvo: AlvoRevisao::Arquivo(it.file_id),
            campo: c.campo.clone(),
            acao: AcaoRevisao::Aceitar,
            valor: None,
        })
        .collect();
    decidir(&mut a, decisoes);
    let depois = listar(&mut a);
    assert!(depois[0].pronto_para_cadastrar, "{depois:?}");
    assert_eq!(depois[0].cadastro_id, None);
    assert_eq!(depois[0].titulo.as_deref(), Some("Livro Pronto"));
    assert_eq!(depois[0].estado, None);
}

// ---------------------------------------------------------------------------------------------
// Lacunas da Fase B: B-02 (clonar estrutura), B-14 (impressão textual), B-26 (cobertura)
// ---------------------------------------------------------------------------------------------

use dag_calibre::executor::{Comando as ComandoCalibre, ExecutorCalibre, SaidaProcesso};

/// `calibredb` falso: `list` cria o `metadata.db`; `add_custom_column` grava em `custom_columns`.
#[derive(Debug, Default)]
struct CalibredbFalso {
    chamadas: std::sync::Mutex<Vec<Vec<String>>>,
}

impl ExecutorCalibre for CalibredbFalso {
    fn executar(&self, cmd: &ComandoCalibre) -> dag_calibre::Resultado<SaidaProcesso> {
        self.chamadas.lock().unwrap().push(cmd.args.clone());
        let a = &cmd.args;
        let lib = a
            .iter()
            .position(|x| x == "--library-path")
            .map(|i| PathBuf::from(&a[i + 1]))
            .unwrap();
        match a[0].as_str() {
            "list" => {
                if !lib.join("metadata.db").exists() {
                    let c = Connection::open(lib.join("metadata.db")).unwrap();
                    c.execute_batch(ESQUEMA).unwrap();
                    c.execute(
                        "INSERT INTO library_id(uuid) VALUES ('uuid-a-classificar')",
                        [],
                    )
                    .unwrap();
                }
                Ok(SaidaProcesso::ok("[]"))
            }
            "add_custom_column" => {
                let c = Connection::open(lib.join("metadata.db")).unwrap();
                let multiplo = a.iter().any(|x| x == "--is-multiple");
                let tipo = a[3].clone();
                let display = a
                    .iter()
                    .position(|x| x == "--display")
                    .map_or("{}".to_owned(), |i| a[i + 1].clone());
                c.execute(
                    "INSERT INTO custom_columns(label,name,datatype,display,is_multiple,normalized)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![a[1], a[2], tipo, display, multiplo, tipo == "text"],
                )
                .unwrap();
                let n = c.last_insert_rowid();
                if tipo == "text" {
                    c.execute_batch(&format!(
                        "CREATE TABLE custom_column_{n} (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
                         CREATE TABLE books_custom_column_{n}_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value INTEGER NOT NULL);"
                    ))
                    .unwrap();
                } else {
                    c.execute_batch(&format!(
                        "CREATE TABLE custom_column_{n} (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value REAL);"
                    ))
                    .unwrap();
                }
                Ok(SaidaProcesso::ok(""))
            }
            _ => Ok(SaidaProcesso::ok("")),
        }
    }
}

/// Acrescenta `#pasta` (text) e `#nota` (int) à Homologada sintética.
fn colunas_na_homologada(m: &Mundo) {
    let c = Connection::open(m.homologada.join("metadata.db")).unwrap();
    c.execute_batch(
        "INSERT INTO custom_columns(label,name,datatype,display,is_multiple,normalized)
           VALUES ('pasta','!1.Pasta','text','{}',0,1), ('nota','Nota','int','{}',0,0);
         CREATE TABLE custom_column_1 (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE books_custom_column_1_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value INTEGER NOT NULL);
         CREATE TABLE custom_column_2 (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, value INTEGER);",
    )
    .unwrap();
}

fn colunas_de(raiz: &Path) -> Vec<String> {
    let c = Connection::open(raiz.join("metadata.db")).unwrap();
    let mut st = c
        .prepare("SELECT label FROM custom_columns ORDER BY label")
        .unwrap();
    st.query_map([], |l| l.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn ca_b02_a_classificar_recria_as_colunas_da_homologada_e_e_idempotente() {
    let e = Ambiente::novo();
    colunas_na_homologada(&e.m);
    let falso = Arc::new(CalibredbFalso::default());
    let mut a = e
        .agente(Autor::Gui)
        .com_executor_calibre(falso.clone())
        .com_processos_calibre(vec![]);
    let _ = registrar_homologada(&mut a, &e.m);
    let antes = arvore(&e.m.homologada);
    let destino = e.m._tmp.path().join("A_Classificar");
    let registrar = |a: &mut Agente| {
        bib(a.executar(Comando::BibliotecaRegistrar {
            caminho: destino.to_str().unwrap().into(),
            papel: PapelRegistro::AClassificar,
            nome: None,
        }))
    };
    let (id, avisos) = match registrar(&mut a) {
        DadosBiblioteca::Registrada {
            biblioteca, avisos, ..
        } => (biblioteca.id, avisos),
        d => panic!("{d:?}"),
    };
    let cols = colunas_de(&destino);
    for esperada in ["dag_confianca", "dag_hash", "nota", "pasta"] {
        assert!(cols.contains(&esperada.to_owned()), "{cols:?}");
    }
    assert!(avisos.iter().any(|v| v.contains("pasta")), "{avisos:?}");
    // O relatório de compatibilidade fica em campos_calibre.
    match bib(a.executar(Comando::BibliotecaPerfil {
        biblioteca: id.to_string(),
    })) {
        DadosBiblioteca::Perfil { campos, .. } => {
            assert!(campos.iter().any(|c| c.rotulo == "pasta" && c.clonavel));
        }
        d => panic!("{d:?}"),
    }
    // Nenhum comando do calibredb apontou para a Homologada.
    let h = e.m.homologada.to_str().unwrap();
    assert!(
        falso
            .chamadas
            .lock()
            .unwrap()
            .iter()
            .all(|c| !c.iter().any(|x| x.starts_with(h)))
    );
    assert_eq!(arvore(&e.m.homologada), antes);
    // Segundo registro: nada novo é criado.
    let n = falso.chamadas.lock().unwrap().len();
    registrar(&mut a);
    assert_eq!(falso.chamadas.lock().unwrap().len(), n);
    assert_eq!(colunas_de(&destino), cols);
}

// --- B-14: impressão digital textual -----------------------------------------------------------

type PaginasDe = dyn Fn(&str) -> Vec<String> + Send + Sync;

/// Extrator que devolve páginas de texto por nome de arquivo.
struct ExtratorPaginas {
    paginas_de: Box<PaginasDe>,
}

impl ExtratorLivro for ExtratorPaginas {
    fn nome(&self) -> String {
        "paginas".into()
    }

    fn extrair(
        &self,
        caminho: &Path,
        _formato: FormatoLivro,
        _cfg: &ConfigBiblioteca,
    ) -> Result<SaidaExtracao, String> {
        let nome = caminho.file_name().unwrap().to_string_lossy().into_owned();
        let paginas = (self.paginas_de)(&nome)
            .into_iter()
            .enumerate()
            .map(|(i, texto)| dag_biblio::PaginaTexto {
                numero: i as u32 + 1,
                texto,
                via_ocr: true,
                qualidade: 0.8,
                idioma_ocr: None,
            })
            .collect();
        Ok(saida(Vec::new(), None, paginas))
    }
}

/// Texto pseudo-aleatório determinístico (`n` palavras).
fn texto_livro(semente: u64, n: usize) -> Vec<String> {
    let silabas = [
        "ca", "mo", "ri", "tu", "be", "lo", "sa", "ne", "du", "fi", "ga", "po", "re", "vu", "xo",
        "ze",
    ];
    let mut s = semente.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut palavras = Vec::new();
    for _ in 0..n {
        let mut p = String::new();
        for _ in 0..3 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            p.push_str(silabas[(s % 16) as usize]);
        }
        palavras.push(p);
    }
    palavras
        .chunks(300)
        .map(|c| c.join(" "))
        .collect::<Vec<_>>()
}

#[test]
fn b14_duas_digitalizacoes_do_mesmo_livro_viram_hipotese_de_mesma_edicao_nunca_exata() {
    let e = Ambiente::novo();
    let original = texto_livro(11, 3000);
    // A segunda digitalização tem OCR ruim: letras trocadas, números de página e caixa alta.
    let ruidosa: Vec<String> = original
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut v: Vec<String> = p.split(' ').map(str::to_owned).collect();
            for (k, w) in v.iter_mut().enumerate() {
                if k % 30 == 0 {
                    w.replace_range(0..1, "x");
                }
            }
            format!("{} {}\nLIVRO", v.join(" ").to_uppercase(), i + 1)
        })
        .collect();
    let outro = texto_livro(99, 3000);
    let (a_txt, b_txt, c_txt) = (original.clone(), ruidosa, outro);
    let ext = Arc::new(ExtratorPaginas {
        paginas_de: Box::new(move |n| match n {
            "scan_a.pdf" => a_txt.clone(),
            "scan_b.pdf" => b_txt.clone(),
            _ => c_txt.clone(),
        }),
    });
    fs::write(e.m.downloads.join("scan_a.pdf"), corpo(60, 30_000)).unwrap();
    fs::write(e.m.downloads.join("scan_b.pdf"), corpo(61, 30_000)).unwrap();
    fs::write(e.m.downloads.join("outro.pdf"), corpo(62, 30_000)).unwrap();
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    let conn = Connection::open(&e.m.catalogo).unwrap();
    let nome_de = |id: i64| -> String {
        conn.query_row("SELECT name_display FROM files WHERE id = ?1", [id], |l| {
            l.get(0)
        })
        .unwrap()
    };
    let mut st = conn
        .prepare(
            "SELECT file_id, alvo_id, tipo, bytes FROM candidatos_duplicidade
             WHERE alvo_tipo = 'arquivo'",
        )
        .unwrap();
    let linhas: Vec<(String, String, String, i64)> = st
        .query_map([], |l| {
            Ok((
                nome_de(l.get(0)?),
                nome_de(l.get(1)?),
                l.get::<_, String>(2)?,
                l.get(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        linhas.iter().any(|(f, al, t, _)| t == "mesma_edicao"
            && ["scan_a.pdf", "scan_b.pdf"].contains(&f.as_str())
            && ["scan_a.pdf", "scan_b.pdf"].contains(&al.as_str())),
        "{linhas:?}"
    );
    // Nunca Exata, nunca espaço recuperável, e o texto diferente não gera hipótese.
    assert!(linhas.iter().all(|(_, _, t, b)| t != "exata" && *b == 0));
    assert!(
        !linhas
            .iter()
            .any(|(f, al, _, _)| f == "outro.pdf" || al == "outro.pdf"),
        "{linhas:?}"
    );
}

// --- B-26: verificação de Cobertura -------------------------------------------------------------

fn volume_hd(raiz: &Path) -> VolumeDescoberto {
    let mut v = volume(raiz);
    v.platform_id = format!("HD-{}", v.st_dev);
    v.rotulo = Some("HD-Externo".into());
    v.dispositivo = Some(DispositivoDescoberto {
        identificador: "disco-hd".into(),
        modelo: None,
        tipo: TipoDispositivo::Hdd,
        removivel: false,
    });
    v
}

fn cobertura(a: &mut Agente, id: i64, amostra: u32) -> dag_core::ResumoCobertura {
    match bib(a.executar(Comando::BibliotecaVerificarCobertura {
        biblioteca: id.to_string(),
        amostra: Some(amostra),
    })) {
        DadosBiblioteca::Cobertura { resumo } => resumo,
        d => panic!("{d:?}"),
    }
}

#[test]
fn ca_b24_cobertura_verificada_amostra_relida_e_descarte_exige_verificacao_recente() {
    let e = Ambiente::novo();
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() || fs::metadata(shm).unwrap().dev() == fs::metadata(&e.m.raiz).unwrap().dev() {
        eprintln!("AVISO: sem segundo filesystem; teste de Cobertura ignorado");
        return;
    }
    let hd = tempfile::Builder::new()
        .prefix("dag-hd-")
        .tempdir_in(shm)
        .unwrap();
    let hd_raiz = fs::canonicalize(hd.path()).unwrap();
    // Cópias de backup dos 3 Formatos da Homologada e uma cópia solta em Downloads.
    let originais = [
        "Machado de Assis/Dom Casmurro (1)/Dom Casmurro.pdf",
        "Machado de Assis/Memórias Póstumas (2)/Memórias Póstumas.epub",
        "Machado de Assis/O Alienista (3)/O Alienista.pdf",
    ];
    for (i, o) in originais.iter().enumerate() {
        let destino = hd_raiz.join(format!("backup/{i}_{}", o.rsplit('/').next().unwrap()));
        fs::create_dir_all(destino.parent().unwrap()).unwrap();
        fs::copy(e.m.homologada.join(o), destino).unwrap();
    }
    fs::copy(
        e.m.homologada.join(originais[0]),
        e.m.downloads.join("copia.pdf"),
    )
    .unwrap();
    let mut a = e
        .agente(Autor::Gui)
        .com_volumes_fixos(vec![volume(&e.m.raiz), volume_hd(&hd_raiz)]);
    a.executar(Comando::IniciarScan {
        modo: ModoVarredura::FullScan,
        raizes: vec![
            e.m.raiz.to_str().unwrap().into(),
            hd_raiz.to_str().unwrap().into(),
        ],
        sem_hash: false,
        elevar: false,
        releitura_total: false,
        threads_por_disco: None,
        max_discos: None,
    });
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    let homologada_antes = arvore(&e.m.homologada);
    let razoes_da_copia = |a: &mut Agente| -> Vec<String> {
        let DadosBiblioteca::Duplicatas { grupos, .. } =
            bib(a.executar(Comando::BibliotecaDuplicatas {
                raiz: Some(e.m.downloads.to_str().unwrap().into()),
                limite: None,
                cursor: None,
            }))
        else {
            panic!()
        };
        grupos[0].copias[0].decisao.razoes.clone()
    };
    // Nunca verificada: o descarte nem é sugerido.
    assert!(
        razoes_da_copia(&mut a)
            .iter()
            .any(|r| r.contains("nunca foi verificada")),
    );
    // Sem Marcação de Backup, nenhum Formato tem Cobertura.
    let r0 = cobertura(&mut a, id, 5);
    assert_eq!((r0.formatos_total, r0.formatos_cobertos), (3, 0));
    assert!(!r0.aprovada);
    assert!(r0.arquivos_no_manifesto >= 4, "{r0:?}"); // metadata.db + 3 formatos
    assert!(!r0.hash_metadata_db.is_empty());
    // Marcação de Backup no HD: os 3 Formatos passam a ter Cobertura e a amostra confere.
    let m = a.executar(Comando::MarcarBackup {
        alvo: dag_core::Alvo::Volume("HD-Externo".into()),
    });
    assert!(matches!(m, Resposta::Marcacao { .. }), "{m:?}");
    let r1 = cobertura(&mut a, id, 5);
    assert_eq!(r1.formatos_cobertos, 3, "{r1:?}");
    assert_eq!(
        (r1.amostra_verificada, r1.amostra_ok, r1.amostra_falha),
        (3, 3, 0)
    );
    assert!(r1.aprovada, "{r1:?}");
    let conn = Connection::open(&e.m.catalogo).unwrap();
    let (estado, rest): (String, Option<String>) = conn
        .query_row(
            "SELECT verificacao_estado, restauracao_json FROM backups_biblioteca
             ORDER BY id DESC LIMIT 1",
            [],
            |l| Ok((l.get(0)?, l.get(1)?)),
        )
        .unwrap();
    assert_eq!(estado, "ok");
    assert!(rest.unwrap().contains("releitura_blake3_sem_copia"));
    // Verificação aprovada e recente: a razão some (outras razões da Fase A podem continuar).
    assert!(
        !razoes_da_copia(&mut a).iter().any(
            |r| r.contains("Cobertura da Biblioteca") || r.contains("verificação de Cobertura")
        ),
    );
    // Passados 40 dias (validade padrão: 30), volta a exigir.
    e.avancar(40 * 86_400);
    assert!(
        razoes_da_copia(&mut a)
            .iter()
            .any(|r| r.contains("40 dias")),
    );
    // Corrompe uma cópia de backup (mesmo tamanho): a amostra detecta e reprova.
    let alvo = fs::read_dir(hd_raiz.join("backup"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut b = fs::read(&alvo).unwrap();
    b[100] ^= 0xFF;
    fs::write(&alvo, b).unwrap();
    let r2 = cobertura(&mut a, id, 10);
    assert_eq!(r2.amostra_falha, 1, "{r2:?}");
    assert!(!r2.aprovada);
    assert!(r2.falhas[0].contains("BLAKE3"), "{:?}", r2.falhas);
    // Tudo foi só leitura do acervo.
    assert_eq!(arvore(&e.m.homologada), homologada_antes);
}

#[test]
fn ca_b02_calibredb_real_cria_a_classificar_com_pasta_e_demais_colunas() {
    if dag_calibre::executor::ExecutorProcesso::disponivel().is_none() {
        eprintln!("AVISO: teste pulado, calibredb ausente");
        return;
    }
    let e = Ambiente::novo();
    colunas_na_homologada(&e.m);
    let mut a = e.agente(Autor::Gui);
    registrar_homologada(&mut a, &e.m);
    let antes = arvore(&e.m.homologada);
    let destino = e.m._tmp.path().join("A_Classificar_real");
    let r = a.executar(Comando::BibliotecaRegistrar {
        caminho: destino.to_str().unwrap().into(),
        papel: PapelRegistro::AClassificar,
        nome: None,
    });
    match r {
        Resposta::Biblioteca { .. } => {}
        Resposta::Erro { codigo, mensagem } => {
            eprintln!("AVISO: calibredb não pôde criar a A Classificar ({codigo}): {mensagem}");
            assert_eq!(arvore(&e.m.homologada), antes);
            return;
        }
        outra => panic!("{outra:?}"),
    }
    let cols = colunas_de(&destino);
    for esperada in ["dag_confianca", "dag_hash", "nota", "pasta"] {
        assert!(cols.contains(&esperada.to_owned()), "{cols:?}");
    }
    assert_eq!(arvore(&e.m.homologada), antes);
}

/// IA falsa que registra a ordem das chamadas. Em lote, imita o `Classificador` real: todos os
/// primários e, depois, os segundos pareceres dos que precisam (nome com "dup").
struct IaOrdem {
    log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl IaOrdem {
    fn resultado(entrada: &EntradaClassificacao) -> ResultadoClassificacao {
        ResultadoClassificacao {
            valores: vec![lista_v(
                Campo::Tags,
                &["Literatura"],
                OrigemEvidencia::Ia,
                0.8,
            )],
            segundo_parecer_consultado: entrada.nome_arquivo.contains("dup"),
            ..ResultadoClassificacao::default()
        }
    }
}

impl dag_agent::ClassificadorIa for IaOrdem {
    fn nome(&self) -> String {
        "ordem:1b".into()
    }

    fn classificar(
        &self,
        entrada: &EntradaClassificacao,
    ) -> Result<ResultadoClassificacao, String> {
        let mut l = self.log.lock().unwrap();
        l.push(format!("P:{}", entrada.nome_arquivo));
        if entrada.nome_arquivo.contains("dup") {
            l.push(format!("S:{}", entrada.nome_arquivo));
        }
        Ok(Self::resultado(entrada))
    }

    fn classificar_lote(
        &self,
        entradas: &[EntradaClassificacao],
    ) -> Vec<Result<ResultadoClassificacao, String>> {
        let mut l = self.log.lock().unwrap();
        for e in entradas {
            l.push(format!("P:{}", e.nome_arquivo));
        }
        for e in entradas.iter().filter(|e| e.nome_arquivo.contains("dup")) {
            l.push(format!("S:{}", e.nome_arquivo));
        }
        entradas.iter().map(|e| Ok(Self::resultado(e))).collect()
    }
}

fn processar_com_lote(lote: usize) -> (Vec<String>, Vec<String>, dag_core::ResumoProcessamento) {
    let e = Ambiente::novo();
    etiquetar(&e.m, 1, "Literatura");
    etiquetar(&e.m, 2, "Literatura");
    for (i, n) in ["a.pdf", "b_dup.pdf", "c.pdf", "d_dup.pdf"]
        .iter()
        .enumerate()
    {
        fs::write(e.m.downloads.join(n), corpo(30 + i as u8, 30_000)).unwrap();
    }
    let ext = ExtratorRoteiro::novo(|_| {
        vec![(
            String::new(),
            vec![texto_v(
                Campo::Titulo,
                "Quincas Borba",
                OrigemEvidencia::MetadadoInterno,
                0.9,
            )],
        )]
    });
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut a = e
        .agente_com(Autor::Gui, ext)
        .com_classificador_ia(Arc::new(IaOrdem {
            log: Arc::clone(&log),
        }));
    a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"ia": {"lote": lote}}),
    });
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r = match bib(a.executar(Comando::BibliotecaProcessar {
        limite: None,
        ia: true,
    })) {
        DadosBiblioteca::Processada { resumo } => resumo,
        d => panic!("{d:?}"),
    };
    let itens = revisao(&mut a);
    let mut gravado: Vec<String> = itens
        .iter()
        .filter(|i| {
            i.caminho.as_deref().is_some_and(|c| {
                ["a.pdf", "b_dup.pdf", "c.pdf", "d_dup.pdf"]
                    .iter()
                    .any(|n| c.ends_with(&format!("/{n}")))
            })
        })
        .map(|i| {
            let nome = i.caminho.clone().unwrap_or_default();
            let nome = nome.rsplit('/').next().unwrap_or_default().to_owned();
            let campos: Vec<String> = i
                .campos
                .iter()
                .map(|c| {
                    format!(
                        "{}={:?}/{:?}/{:?}/{:?}/{:?}/{}",
                        c.campo,
                        c.sugerido,
                        c.sugerido_lista,
                        c.confianca,
                        c.origem,
                        c.motivos,
                        c.estado
                    )
                })
                .collect();
            format!("{nome}: {campos:?}")
        })
        .collect();
    gravado.sort();
    let l = log.lock().unwrap().clone();
    (l, gravado, r)
}

#[test]
fn ia_em_lote_agrupa_primarios_e_depois_segundos_pareceres_com_o_mesmo_resultado() {
    let (log_um, gravado_um, r_um) = processar_com_lote(1);
    let (log_lote, gravado_lote, r_lote) = processar_com_lote(8);

    let primarios = |l: &[String]| l.iter().filter(|x| x.starts_with("P:")).count();
    assert_eq!(primarios(&log_um), 4, "{log_um:?}");
    assert_eq!(primarios(&log_lote), 4, "{log_lote:?}");

    // Item a item, o segundo parecer vem logo depois do primário do mesmo livro, e a sequência
    // de modelos alterna. Em lote, nenhum primário vem depois do primeiro segundo parecer.
    let trocas = |l: &[String]| l.windows(2).filter(|w| w[0][..1] != w[1][..1]).count();
    let primeiro_s = log_lote.iter().position(|x| x.starts_with("S:")).unwrap();
    assert!(
        log_lote[primeiro_s..].iter().all(|x| x.starts_with("S:")),
        "{log_lote:?}"
    );
    assert_eq!(trocas(&log_lote), 1, "{log_lote:?}");
    assert!(trocas(&log_um) > 1, "{log_um:?}");

    assert_eq!(r_um.consultas_ia, 4, "{r_um:?}");
    assert_eq!(r_lote.consultas_ia, 4, "{r_lote:?}");
    assert_eq!(r_um.revisao_pendente, r_lote.revisao_pendente);
    assert_eq!(r_um.falhas + r_lote.falhas, 0);
    assert_eq!(gravado_um.len(), 4, "{gravado_um:?}");
    assert_eq!(gravado_um, gravado_lote, "mesmo resultado gravado");
}

#[test]
fn compactado_grande_so_manda_o_teto_de_membros_para_a_ia() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct IaContadora(Arc<AtomicUsize>);
    impl dag_agent::ClassificadorIa for IaContadora {
        fn nome(&self) -> String {
            "conta:1b".into()
        }

        fn classificar(
            &self,
            _entrada: &EntradaClassificacao,
        ) -> Result<ResultadoClassificacao, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ResultadoClassificacao::default())
        }
    }
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("cursos.pdf"), corpo(33, 30_000)).unwrap();
    // 20 "Membros" num só arquivo: mais que o teto padrão (12).
    let ext = ExtratorRoteiro::novo(|_| {
        (0..20)
            .map(|i| {
                (
                    format!("curso/aula{i:02}.pdf"),
                    vec![texto_v(
                        Campo::Titulo,
                        &format!("Aula {i}"),
                        OrigemEvidencia::MetadadoInterno,
                        0.9,
                    )],
                )
            })
            .collect()
    });
    let chamadas = Arc::new(AtomicUsize::new(0));
    let mut a = e
        .agente_com(Autor::Gui, ext)
        .com_classificador_ia(Arc::new(IaContadora(Arc::clone(&chamadas))));
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r = match bib(a.executar(Comando::BibliotecaProcessar {
        limite: None,
        ia: true,
    })) {
        DadosBiblioteca::Processada { resumo } => resumo,
        d => panic!("{d:?}"),
    };
    assert_eq!(chamadas.load(Ordering::SeqCst), 12, "{r:?}");
    assert_eq!(r.membros_de_conteiner, 20, "{r:?}");
    // Todos os 20 viram itens de revisão; os 8 sem IA dizem por quê.
    let itens: Vec<ItemRevisaoInfo> = revisao(&mut a)
        .into_iter()
        .filter(|i| i.membro.is_some())
        .collect();
    assert_eq!(itens.len(), 20, "{itens:#?}");
    let sem_ia = itens
        .iter()
        .filter(|i| format!("{i:?}").contains("IA não consultada"))
        .count();
    assert_eq!(sem_ia, 8, "{itens:#?}");
}

#[test]
fn copia_de_arquivo_ja_curado_vira_duplicata_sem_nova_extracao() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let e = Ambiente::novo();
    let conteudo = corpo(41, 40_000);
    fs::create_dir_all(e.m.downloads.join("a")).unwrap();
    fs::create_dir_all(e.m.downloads.join("b")).unwrap();
    fs::write(e.m.downloads.join("a/livro.pdf"), &conteudo).unwrap();
    let extracoes = Arc::new(AtomicUsize::new(0));
    let cont = Arc::clone(&extracoes);
    let ext = ExtratorRoteiro::novo(move |_| {
        cont.fetch_add(1, Ordering::SeqCst);
        vec![(
            String::new(),
            vec![texto_v(
                Campo::Titulo,
                "Um Livro",
                OrigemEvidencia::MetadadoInterno,
                0.9,
            )],
        )]
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    processar(&mut a);
    assert_eq!(extracoes.load(Ordering::SeqCst), 1);

    // A mesma cópia aparece em outra pasta: vira Duplicata Exata do primeiro, sem extrair.
    fs::write(e.m.downloads.join("b/copia.pdf"), &conteudo).unwrap();
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r = processar(&mut a);
    assert_eq!(extracoes.load(Ordering::SeqCst), 1, "{r:?}");
    assert!(
        alertas(&mut a)
            .iter()
            .any(|al| format!("{al:?}").contains("arquivo_igual")),
        "alerta de cópia"
    );
    let itens = revisao(&mut a);
    assert_eq!(
        itens
            .iter()
            .filter(|i| i.caminho.as_deref().is_some_and(|c| c.ends_with(".pdf")))
            .count(),
        1,
        "{itens:#?}"
    );
}

#[test]
fn priorizar_por_tamanho_poe_o_maior_na_frente_mesmo_com_espera_menor() {
    let e = Ambiente::novo();
    fs::write(e.m.downloads.join("pequeno.pdf"), corpo(31, 20_000)).unwrap();
    fs::write(e.m.downloads.join("grande.pdf"), corpo(32, 900_000)).unwrap();
    let mut a = e.agente(Autor::Gui);
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    // O pequeno espera há 60 h: pelo aging, passa na frente do grande.
    Connection::open(&e.m.catalogo)
        .unwrap()
        .execute(
            "UPDATE jobs_bibliograficos SET enfileirado_em = enfileirado_em - 60 * 3600
             WHERE file_id IN (SELECT id FROM files WHERE name_display = 'pequeno.pdf')",
            [],
        )
        .unwrap();
    let primeiro = |a: &mut Agente| match bib(a.executar(Comando::BibliotecaFila {
        estado: Some(EstadoFilaInfo::Pendente),
        limite: None,
        cursor: None,
    })) {
        DadosBiblioteca::Fila { itens, .. } => itens[0].caminho.clone().unwrap(),
        d => panic!("{d:?}"),
    };
    assert!(primeiro(&mut a).ends_with("pequeno.pdf"));
    let r = a.executar(Comando::BibliotecaPriorizar {
        criterio: dag_core::CriterioPrioridade::Tamanho,
    });
    assert!(
        matches!(&r, Resposta::Ok { mensagem: Some(m) } if m.starts_with("2 ")),
        "{r:?}"
    );
    assert!(primeiro(&mut a).ends_with("grande.pdf"));
}

#[test]
fn copias_identicas_na_mesma_rodada_extraem_uma_vez_so() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let e = Ambiente::novo();
    let conteudo = corpo(43, 60_000);
    for d in ["a", "b", "c"] {
        fs::create_dir_all(e.m.downloads.join(d)).unwrap();
        fs::write(e.m.downloads.join(format!("{d}/livro.pdf")), &conteudo).unwrap();
    }
    let extracoes = Arc::new(AtomicUsize::new(0));
    let cont = Arc::clone(&extracoes);
    let ext = ExtratorRoteiro::novo(move |_| {
        cont.fetch_add(1, Ordering::SeqCst);
        // Devagar o bastante para as três cópias serem pegas antes de a primeira terminar.
        std::thread::sleep(std::time::Duration::from_millis(300));
        vec![(
            String::new(),
            vec![texto_v(
                Campo::Titulo,
                "Um Livro",
                OrigemEvidencia::MetadadoInterno,
                0.9,
            )],
        )]
    });
    let mut a = e.agente_com(Autor::Gui, ext);
    a.executar(Comando::BibliotecaConfigDefinir {
        alteracoes: serde_json::json!({"extracoes_por_dispositivo": 4}),
    });
    a.executar(scan(&e.m, ModoVarredura::FullScan));
    let id = registrar_homologada(&mut a, &e.m);
    sincronizar(&mut a, id);
    enfileirar(&mut a, &e.m.downloads, MotivoFila::Novo);
    let r = processar(&mut a);
    assert_eq!(extracoes.load(Ordering::SeqCst), 1, "{r:?}");
    assert_eq!(r.pendentes_restantes, 0, "{r:?}");
    let copias = alertas(&mut a)
        .iter()
        .filter(|al| format!("{al:?}").contains("arquivo_igual"))
        .count();
    assert_eq!(copias, 2);
}
