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
