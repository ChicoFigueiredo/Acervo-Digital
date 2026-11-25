//! Testes da camada de persistência da Curadoria Bibliográfica (migração 0011).
#![allow(clippy::unwrap_used, clippy::too_many_arguments)]

use bancada_armazem::{Armazem, Escada, Pragmas};
use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, OrigemEvidencia, PapelBiblioteca,
    ResultadoExtracao, TipoDuplicidade, ValorSugerido,
};
use dag_store::biblioteca::*;
use dag_store::{Catalogo, MIGRACOES};
use rusqlite::Connection;

fn cat() -> Catalogo {
    Catalogo::em_memoria().expect("catálogo em memória")
}

/// Volume + pasta + `n` arquivos (`/b/f<i>`); devolve (volume, ids dos arquivos).
fn volume_e_arquivos(c: &Connection, n: usize) -> (i64, Vec<i64>) {
    c.execute("INSERT INTO volumes (platform_id) VALUES ('vol-b')", [])
        .unwrap();
    let vol = c.last_insert_rowid();
    c.execute(
        "INSERT INTO folders (volume_id, parent_folder_id, path_raw, path_display, name_raw, name_display)
         VALUES (?1, NULL, x'2f62', '/b', x'62', 'b')",
        [vol],
    )
    .unwrap();
    let pasta = c.last_insert_rowid();
    let mut st = c
        .prepare(
            "INSERT INTO files (volume_id, folder_id, path_raw, path_display, name_raw, name_display, logical_bytes)
             VALUES (?1, ?2, ?3, 'a', ?3, 'a', ?4) RETURNING id",
        )
        .unwrap();
    let ids = (0..n)
        .map(|i| {
            st.query_row(
                rusqlite::params![vol, pasta, format!("/b/f{i}").into_bytes(), 1000 + i as i64],
                |l| l.get(0),
            )
            .unwrap()
        })
        .collect();
    (vol, ids)
}

fn arquivos(c: &Connection, n: usize) -> Vec<i64> {
    volume_e_arquivos(c, n).1
}

fn hash(n: u8) -> [u8; 32] {
    [n; 32]
}

fn livro(id: i64, titulo: &str, formatos: &[(&str, u64)]) -> LivroEspelho {
    LivroEspelho {
        calibre_id: id,
        uuid: Some(format!("uuid-{id}")),
        titulo: titulo.into(),
        autores: vec!["Fulano de Tal".into()],
        isbn13: Some("9788535902778".into()),
        idioma: Some("por".into()),
        serie: None,
        indice: None,
        editora: Some("Cia".into()),
        tags: vec!["matemática".into()],
        estante: None,
        caminho: format!("Fulano/{titulo} ({id})"),
        formatos: formatos
            .iter()
            .map(|(f, t)| FormatoEspelho {
                formato: (*f).into(),
                caminho: format!("/lib/{id}.{f}"),
                tamanho: *t,
                mtime_ns: Some(7),
            })
            .collect(),
    }
}

fn bib(c: &Connection, caminho: &str, papel: PapelBiblioteca) -> i64 {
    registrar_biblioteca(
        c,
        &NovaBiblioteca {
            caminho,
            papel,
            nome: "n",
            library_uuid: None,
            versao_calibre: Some("7.6"),
        },
        100,
    )
    .unwrap()
}

fn contar(c: &Connection, t: &str) -> i64 {
    c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |l| l.get(0))
        .unwrap()
}

#[test]
fn migracao_em_catalogo_novo_cria_as_tabelas() {
    let cat = cat();
    assert_eq!(cat.versao().unwrap(), 13);
    let c = cat.armazem().conexao();
    for t in [
        "bibliotecas_calibre",
        "instantaneos_biblioteca",
        "campos_calibre",
        "livros_homologados",
        "formatos_homologados",
        "obras",
        "edicoes",
        "formatos_bibliograficos",
        "autores_biblio",
        "vinculo_autoria",
        "jobs_bibliograficos",
        "extracoes",
        "sugestoes",
        "decisoes_humanas",
        "inferencias",
        "candidatos_duplicidade",
        "alertas_biblio",
        "cadastros_a_classificar",
        "homologacoes",
        "backups_biblioteca",
    ] {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [t],
                |l| l.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "tabela {t} ausente");
    }
}

#[test]
fn migracao_a_partir_da_versao_10_preserva_dados() {
    let mut a = Armazem::em_memoria(&Pragmas::default()).unwrap();
    a.migrar_ate(&Escada::nova(MIGRACOES), 10).unwrap();
    assert_eq!(a.versao().unwrap(), 10);
    let ids = arquivos(a.conexao(), 3);
    a.conexao()
        .execute("INSERT INTO contadores (chave, valor) VALUES ('x', 5)", [])
        .unwrap();
    a.migrar(&Escada::nova(MIGRACOES)).unwrap();
    assert_eq!(a.versao().unwrap(), 13);
    let c = a.conexao();
    assert_eq!(contar(c, "files"), 3);
    assert_eq!(contador(c, "x").unwrap(), 5);
    // As tabelas novas funcionam sobre o catálogo migrado (FK para files).
    enfileirar_job(c, ids[0], MotivoJob::Novo, 0, None, 1).unwrap();
    assert_eq!(contagem_jobs(c).unwrap().pendentes, 1);
    a.verificar_integridade().unwrap();
}

#[test]
fn bibliotecas_papeis_e_perfil() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let h1 = bib(c, "/lib/h1", PapelBiblioteca::Homologada);
    let h2 = bib(c, "/lib/h2", PapelBiblioteca::Homologada);
    assert_ne!(h1, h2, "várias homologadas são permitidas");
    let ac = bib(c, "/lib/ac", PapelBiblioteca::AClassificar);
    assert_eq!(bib(c, "/lib/ac", PapelBiblioteca::AClassificar), ac);
    let nova = |caminho, papel| NovaBiblioteca {
        caminho,
        papel,
        nome: "",
        library_uuid: None,
        versao_calibre: None,
    };
    // Mesmo caminho, outro papel: erro.
    assert!(registrar_biblioteca(c, &nova("/lib/ac", PapelBiblioteca::Homologada), 1).is_err());
    // Segunda A Classificar ativa: o índice parcial recusa.
    assert!(registrar_biblioteca(c, &nova("/lib/ac2", PapelBiblioteca::AClassificar), 1).is_err());
    gravar_perfil(c, h1, "{\"x\":1}", Some("7.7"), 200).unwrap();
    let b = obter_biblioteca(c, h1).unwrap().unwrap();
    assert_eq!(b.perfil_json.as_deref(), Some("{\"x\":1}"));
    assert_eq!(b.versao_calibre.as_deref(), Some("7.7"));
    assert_eq!(
        listar_bibliotecas(c, Some(PapelBiblioteca::Homologada))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(listar_bibliotecas(c, None).unwrap().len(), 3);

    let campo = CampoCalibre {
        rotulo: "pasta".into(),
        nome: "Pasta".into(),
        datatype: "text".into(),
        multiplo: false,
        clonavel: true,
        motivo: None,
    };
    gravar_campos(c, h1, std::slice::from_ref(&campo)).unwrap();
    gravar_campos(c, h1, std::slice::from_ref(&campo)).unwrap();
    assert_eq!(listar_campos(c, h1).unwrap(), vec![campo]);

    let inst = |quando, h| NovoInstantaneo {
        quando,
        hash_metadata_db: Some(hash(h)),
        ..Default::default()
    };
    assert_eq!(
        gravar_instantaneo(c, h1, &inst(1, 1)).unwrap().resultado,
        ResultadoComparacao::Primeiro
    );
    assert_eq!(
        gravar_instantaneo(c, h1, &inst(2, 1)).unwrap().resultado,
        ResultadoComparacao::Igual
    );
    let i3 = gravar_instantaneo(c, h1, &inst(3, 2)).unwrap();
    assert_eq!(i3.resultado, ResultadoComparacao::Alterado);
    assert_eq!(ultimo_instantaneo(c, h1).unwrap().unwrap().id, i3.id);
}

#[test]
fn espelho_idempotente_remove_ausentes_e_preserva_hash_valido() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let b = bib(c, "/lib/h", PapelBiblioteca::Homologada);
    let livros = vec![
        livro(1, "Álgebra Linear", &[("EPUB", 100), ("PDF", 200)]),
        livro(2, "Cálculo", &[("PDF", 300)]),
        livro(3, "Topologia", &[("PDF", 400)]),
    ];
    for _ in 0..3 {
        let r = sincronizar_espelho(c, b, livros.clone(), 10).unwrap();
        assert_eq!(r.livros, 3);
        assert_eq!(r.removidos, 0);
    }
    assert_eq!(contar(c, "livros_homologados"), 3);
    assert_eq!(contar(c, "formatos_homologados"), 4);
    assert_eq!(contar_livros(c, b).unwrap(), 3);
    assert_eq!(contar_formatos(c, b).unwrap(), 4);

    let sem = formatos_sem_hash(c, b, 0, 100).unwrap();
    assert_eq!(sem.len(), 4);
    for (i, f) in sem.iter().enumerate() {
        assert!(gravar_hash_formato(c, f.id, &hash(i as u8 + 1), f.tamanho, 50).unwrap());
    }
    assert!(formatos_sem_hash(c, b, 0, 100).unwrap().is_empty());
    assert!(!gravar_hash_formato(c, sem[0].id, &hash(9), 999_999, 50).unwrap());

    // Reprocessar não perde o hash (tamanho e mtime iguais).
    sincronizar_espelho(c, b, livros.clone(), 20).unwrap();
    assert!(formatos_sem_hash(c, b, 0, 100).unwrap().is_empty());

    let achados = buscar_por_hash(c, &hash(1), Some(sem[0].tamanho)).unwrap();
    assert_eq!(achados.len(), 1);
    assert_eq!(achados[0].livro.titulo, "Álgebra Linear");
    assert_eq!(achados[0].formato.formato, "EPUB");
    assert!(
        buscar_por_hash(c, &hash(1), Some(sem[0].tamanho + 1))
            .unwrap()
            .is_empty()
    );
    assert!(buscar_por_hash(c, &hash(77), None).unwrap().is_empty());

    // O livro 2 mudou de tamanho (hash antigo deixa de valer); o 3 sumiu; o 1 perdeu o PDF.
    let novos = vec![
        livro(1, "Álgebra Linear", &[("EPUB", 100)]),
        livro(2, "Cálculo", &[("PDF", 301)]),
    ];
    let r = sincronizar_espelho(c, b, novos, 30).unwrap();
    assert_eq!(r.removidos, 1);
    assert_eq!(contar(c, "livros_homologados"), 2);
    assert_eq!(contar(c, "formatos_homologados"), 2);
    assert_eq!(contar_livros(c, b).unwrap(), 2);
    assert_eq!(contar_formatos(c, b).unwrap(), 2);
    let sem = formatos_sem_hash(c, b, 0, 100).unwrap();
    assert_eq!(sem.len(), 1);
    assert_eq!(sem[0].tamanho, 301);
    assert_eq!(buscar_livros_por_isbn(c, "9788535902778").unwrap().len(), 2);
}

#[test]
fn livro_calibre_converte_para_espelho() {
    let l = dag_biblio::LivroCalibre {
        id: 9,
        titulo: "T".into(),
        identificadores: vec![("isbn".into(), "85-359-0277-5".into())],
        idiomas: vec!["por".into()],
        formatos: vec![dag_biblio::FormatoCalibre {
            formato: "epub".into(),
            nome: "x".into(),
            tamanho: 5,
            caminho_absoluto: "/a/x.epub".into(),
        }],
        ..Default::default()
    };
    let e = LivroEspelho::de_calibre(&l, Some("Matemática".into()));
    assert_eq!(e.isbn13.as_deref(), Some("9788535902778"));
    assert_eq!(e.formatos[0].formato, "EPUB");
    assert_eq!(e.estante.as_deref(), Some("Matemática"));
}

#[test]
fn ligacao_com_o_catalogo_reusa_hashes_e_acha_iguais() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let (vol, f) = volume_e_arquivos(c, 4);
    let b = bib(c, "/lib/h", PapelBiblioteca::Homologada);
    // Formatos apontam para /mnt/lib/b/f0 e /mnt/lib/b/f1; o Volume é montado em /mnt/lib.
    let mut l1 = livro(1, "A", &[("PDF", 1000)]);
    l1.formatos[0].caminho = "/mnt/lib/b/f0".into();
    let mut l2 = livro(2, "B", &[("PDF", 1001)]);
    l2.formatos[0].caminho = "/mnt/lib/b/f1".into();
    let mut l3 = livro(3, "C", &[("PDF", 5)]);
    l3.formatos[0].caminho = "/mnt/lib/b/inexistente".into();
    sincronizar_espelho(c, b, vec![l1, l2, l3], 1).unwrap();
    let (ligados, resto) = ligar_formatos_por_caminho(c, b, vol, "/mnt/lib", 0, 100).unwrap();
    assert_eq!((ligados, resto), (2, None));
    // O Catálogo já tem hash de f0 (tamanho 1000) e de f1 (tamanho divergente: não vale).
    for (id, h, t) in [(f[0], 1u8, 1000), (f[1], 2, 777), (f[2], 1, 1000)] {
        c.execute(
            "INSERT INTO hashes (file_id, hash, hashed_at, size_bytes) VALUES (?1, ?2, 1, ?3)",
            rusqlite::params![id, hash(h).as_slice(), t],
        )
        .unwrap();
    }
    let (n, ult) = importar_hashes_do_catalogo(c, b, 0, 100, 5).unwrap();
    assert_eq!((n, ult.is_some()), (1, true));
    // f2 (outro arquivo do Catálogo, mesmo hash e tamanho que o homologado f0) é duplicata exata.
    let iguais = arquivos_iguais_aos_homologados(c, b, 0, 100).unwrap();
    assert_eq!(iguais.len(), 1);
    assert_eq!(iguais[0].file_id, f[2]);
    assert_eq!(iguais[0].tamanho, 1000);
}

#[test]
fn listagem_de_livros_pagina_por_cursor() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let b = bib(c, "/lib/h", PapelBiblioteca::Homologada);
    let livros: Vec<_> = (1..=25).map(|i| livro(i, &format!("L{i}"), &[])).collect();
    sincronizar_espelho(c, b, livros, 1).unwrap();
    let mut vistos = Vec::new();
    let mut cursor = None;
    let mut paginas = 0;
    loop {
        let p = listar_livros(c, b, cursor, 10).unwrap();
        paginas += 1;
        vistos.extend(p.itens.iter().map(|l| l.calibre_id));
        match p.proximo {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    assert_eq!(paginas, 3);
    assert_eq!(vistos, (1..=25).collect::<Vec<_>>());
}

#[test]
fn fila_nao_duplica_e_respeita_prioridade_com_aging() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 4);
    let a = enfileirar_job(c, f[0], MotivoJob::Novo, 1, Some("r1"), 1000).unwrap();
    let b = enfileirar_job(c, f[0], MotivoJob::Novo, 5, Some("r1"), 1001).unwrap();
    assert!(matches!(a, Enfileirado::Criado(_)));
    assert_eq!(b, Enfileirado::JaAtivo(a.id()));
    assert_eq!(contar(c, "jobs_bibliograficos"), 1);
    assert_eq!(obter_job(c, a.id()).unwrap().unwrap().prioridade, 5);

    // f0: prio 5 desde t=1000 (ordem 5*3600-1000); f1: 3 em 10000; f2: 4 há 2 h a mais; f3: 5 em 10000.
    enfileirar_job(c, f[1], MotivoJob::Alterado, 3, None, 10_000).unwrap();
    enfileirar_job(c, f[2], MotivoJob::Novo, 4, None, 10_000 - 2 * 3600).unwrap();
    enfileirar_job(c, f[3], MotivoJob::Novo, 5, None, 10_000).unwrap();
    let ordem: Vec<i64> = (0..4)
        .map(|_| pegar_proximo_job(c, 20_000).unwrap().unwrap().file_id)
        .collect();
    assert_eq!(ordem, vec![f[0], f[2], f[3], f[1]]);
    assert!(pegar_proximo_job(c, 20_000).unwrap().is_none());
    assert!(matches!(
        enfileirar_job(c, f[1], MotivoJob::Reprocessar, 0, None, 30_000).unwrap(),
        Enfileirado::JaAtivo(_)
    ));
}

#[test]
fn job_orfao_volta_a_pendente_com_checkpoint_e_ciclo_de_vida() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 3);
    for (i, id) in f.iter().enumerate() {
        enfileirar_job(c, *id, MotivoJob::Novo, 0, None, 1 + i as i64).unwrap();
    }
    let j = pegar_proximo_job(c, 10).unwrap().unwrap();
    assert_eq!(j.file_id, f[0]);
    assert_eq!((j.estado, j.tentativas), (EstadoJob::EmAndamento, 1));
    gravar_checkpoint(c, j.id, "ocr", Some("{\"pagina\":42}"), 11).unwrap();
    assert_eq!(contagem_jobs(c).unwrap().em_andamento, 1);

    // "Queda": o agente reabre e devolve os órfãos.
    assert_eq!(retomar_orfaos(c, 20).unwrap(), 1);
    let c1 = contagem_jobs(c).unwrap();
    assert_eq!((c1.pendentes, c1.em_andamento), (3, 0));
    let r = obter_job(c, j.id).unwrap().unwrap();
    assert_eq!(r.estado, EstadoJob::Pendente);
    assert_eq!(r.etapa.as_deref(), Some("ocr"));
    assert_eq!(r.checkpoint_json.as_deref(), Some("{\"pagina\":42}"));

    // Falha com retry volta ao fim da fila (aging reinicia); esgotadas as tentativas, `falhou`.
    let j = pegar_proximo_job(c, 30).unwrap().unwrap();
    assert_eq!(j.id, r.id);
    assert_eq!(
        falhar_job(c, j.id, "boom", 3, 31).unwrap(),
        EstadoJob::Pendente
    );
    for _ in 0..2 {
        let o = pegar_proximo_job(c, 40).unwrap().unwrap();
        assert_ne!(o.id, j.id);
        concluir_job(c, o.id, Some("r1"), 41).unwrap();
    }
    let alvo = pegar_proximo_job(c, 50).unwrap().unwrap();
    assert_eq!((alvo.id, alvo.tentativas), (j.id, 3));
    assert_eq!(
        falhar_job(c, alvo.id, "boom", 3, 51).unwrap(),
        EstadoJob::Falhou
    );
    let c2 = contagem_jobs(c).unwrap();
    assert_eq!(
        (c2.concluidos, c2.falhos, c2.pendentes, c2.em_andamento),
        (2, 1, 0, 0)
    );
    assert!(processado_com_versao(c, f[1], "r1").unwrap());
    assert!(!processado_com_versao(c, f[1], "r2").unwrap());

    // Reprocessar cria um novo job (histórico preservado); exceção não volta sozinha.
    assert!(matches!(
        enfileirar_job(c, alvo.file_id, MotivoJob::Reprocessar, 0, None, 60).unwrap(),
        Enfileirado::Criado(_)
    ));
    let e = pegar_proximo_job(c, 61).unwrap().unwrap();
    marcar_excecao(c, e.id, "DRM", 62).unwrap();
    assert_eq!(contagem_jobs(c).unwrap().excecoes, 1);
    assert!(pegar_proximo_job(c, 63).unwrap().is_none());
    assert_eq!(jobs_do_arquivo(c, alvo.file_id).unwrap().len(), 2);
}

#[test]
fn listagem_da_fila_pagina_na_ordem_de_saida() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 30);
    for (i, id) in f.iter().enumerate() {
        enfileirar_job(
            c,
            *id,
            MotivoJob::Novo,
            (i % 5) as i64,
            None,
            1000 + i as i64,
        )
        .unwrap();
    }
    let mut ordem_lista = Vec::new();
    let mut cur = None;
    loop {
        let p = listar_jobs(c, EstadoJob::Pendente, cur, 7).unwrap();
        ordem_lista.extend(p.itens.iter().map(|j| j.id));
        match p.proximo {
            Some(n) => cur = Some(n),
            None => break,
        }
    }
    assert_eq!(ordem_lista.len(), 30);
    let ordem_pegar: Vec<i64> = (0..30)
        .map(|_| pegar_proximo_job(c, 5000).unwrap().unwrap().id)
        .collect();
    assert_eq!(ordem_lista, ordem_pegar);
}

#[test]
fn obras_edicoes_formatos_autores_sao_idempotentes() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 1);
    for rodada in 0..3 {
        let obra = garantir_obra(
            c,
            &NovaObra {
                chave: "isbn:9788535902778",
                titulo: "Álgebra Linear",
                idioma_original: if rodada == 0 { None } else { Some("en") },
            },
            10,
        )
        .unwrap();
        let ed = garantir_edicao(
            c,
            &NovaEdicao {
                obra_id: obra,
                chave: "isbn:9788535902778",
                titulo: "Álgebra Linear",
                editora: Some("Cia"),
                ..Default::default()
            },
            10,
        )
        .unwrap();
        let fb = garantir_formato_biblio(
            c,
            &NovoFormatoBiblio {
                edicao_id: ed,
                formato: FormatoLivro::Epub,
                file_id: Some(f[0]),
                hash: hash(3),
                tamanho: 10,
                estado: if rodada == 0 {
                    EstadoFormatoBiblio::Candidato
                } else {
                    EstadoFormatoBiblio::AClassificar
                },
            },
            10,
        )
        .unwrap();
        let a = garantir_autor(
            c,
            &NovoAutor {
                nome: "José da Silva",
                variantes: vec!["J. Silva".into()],
                ..Default::default()
            },
            10,
        )
        .unwrap();
        let a2 = garantir_autor(
            c,
            &NovoAutor {
                nome: "jose da silva",
                variantes: vec!["J. Silva".into(), "Silva, J.".into()],
                ..Default::default()
            },
            10,
        )
        .unwrap();
        assert_eq!(a, a2);
        vincular_autoria(c, ed, a, PapelAutoria::Autor, 0).unwrap();
        if rodada == 1 {
            definir_estado_formato_biblio(c, fb, EstadoFormatoBiblio::Homologado, 11).unwrap();
        }
    }
    for t in [
        "obras",
        "edicoes",
        "formatos_bibliograficos",
        "autores_biblio",
        "vinculo_autoria",
    ] {
        assert_eq!(contar(c, t), 1, "{t} duplicou");
    }
    let idioma: Option<String> = c
        .query_row("SELECT idioma_original FROM obras", [], |l| l.get(0))
        .unwrap();
    assert_eq!(idioma.as_deref(), Some("en"));
    let variantes: String = c
        .query_row("SELECT variantes_json FROM autores_biblio", [], |l| {
            l.get(0)
        })
        .unwrap();
    assert_eq!(variantes, "[\"J. Silva\",\"Silva, J.\"]");
    assert_eq!(
        obter_edicao(c, 1).unwrap().unwrap().editora.as_deref(),
        Some("Cia")
    );
    // Homologado na rodada 1 não foi rebaixado pelo reprocessamento da rodada 2.
    assert_eq!(formatos_da_edicao(c, 1).unwrap()[0].estado, "homologado");
    assert_eq!(autores_da_edicao(c, 1).unwrap().len(), 1);
}

fn extracao_exemplo() -> ResultadoExtracao {
    ResultadoExtracao {
        formato: FormatoLivro::Pdf,
        estado: EstadoLeitura::Lido,
        metadados: vec![],
        isbns: vec![],
        sumario: vec![],
        paginas: vec![],
        total_paginas: Some(300),
        ficha_catalografica: None,
        capa: None,
        capa_mime: None,
        paginas_ocr: vec![1, 2],
        avisos: vec![],
    }
}

fn valor(campo: Campo, v: &str, conf: f32) -> ValorSugerido {
    ValorSugerido {
        campo,
        valor: Some(v.into()),
        lista: vec![],
        confianca: conf,
        evidencias: vec![Evidencia {
            origem: OrigemEvidencia::MetadadoInterno,
            pagina: None,
            trecho: Some("t".into()),
            detalhe: None,
            confianca: conf,
        }],
    }
}

fn sug(f: i64, v: &ValorSugerido, origem: OrigemEvidencia, versao: &str) -> NovaSugestao {
    NovaSugestao::de_valor(f, v, origem, "", versao)
}

#[test]
fn extracao_e_sugestoes_reprocessadas_3x_nao_duplicam_e_mantem_decisao() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 1)[0];
    let r = extracao_exemplo();
    for _ in 0..3 {
        gravar_resultado_extracao(c, None, f, &r, Some(hash(1)), "v1", 1234, 10).unwrap();
    }
    assert_eq!(contar(c, "extracoes"), 1);
    let e = extracao_vigente(c, f, "v1", &hash(1)).unwrap().unwrap();
    assert_eq!(e.paginas_ocr, 2);
    assert_eq!(
        e.resultado_tipado().unwrap().unwrap().total_paginas,
        Some(300)
    );
    assert!(extracao_vigente(c, f, "v1", &hash(2)).unwrap().is_none());
    assert!(extracao_vigente(c, f, "v2", &hash(1)).unwrap().is_none());

    let titulo = valor(Campo::Titulo, "Cálculo", 0.95);
    let ids: Vec<i64> = (0..3)
        .map(|_| {
            gravar_sugestao(
                c,
                &sug(f, &titulo, OrigemEvidencia::MetadadoInterno, "v1"),
                10,
            )
            .unwrap()
        })
        .collect();
    assert!(ids.windows(2).all(|w| w[0] == w[1]));
    gravar_sugestao(
        c,
        &sug(
            f,
            &valor(Campo::Idioma, "por", 0.4),
            OrigemEvidencia::NomeArquivo,
            "v1",
        ),
        10,
    )
    .unwrap();
    assert_eq!(contar(c, "sugestoes"), 2);
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 2);

    registrar_decisao(
        c,
        &NovaDecisao {
            lote_id: None,
            alvo: AlvoDecisao::Arquivo(f),
            sugestao_id: Some(ids[0]),
            campo: Campo::Titulo,
            acao: AcaoDecisao::Aceitar,
            valor_anterior: None,
            valor_novo: Some("Cálculo".into()),
            autor: AutorAcao::Gui,
        },
        20,
    )
    .unwrap();
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 1);
    gravar_sugestao(
        c,
        &sug(f, &titulo, OrigemEvidencia::MetadadoInterno, "v1"),
        30,
    )
    .unwrap();
    assert_eq!(
        obter_sugestao(c, ids[0]).unwrap().unwrap().estado,
        EstadoSugestao::Aceita
    );
    let novo = valor(Campo::Titulo, "Cálculo I", 0.95);
    gravar_sugestao(
        c,
        &sug(f, &novo, OrigemEvidencia::MetadadoInterno, "v1"),
        40,
    )
    .unwrap();
    let s = obter_sugestao(c, ids[0]).unwrap().unwrap();
    assert_eq!(s.estado, EstadoSugestao::Pendente);
    assert_eq!(s.para_valor_sugerido().valor.as_deref(), Some("Cálculo I"));
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 2);
}

#[test]
fn desfazer_lote_restaura_sugestoes_e_guarda_historico() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 3);
    let s: Vec<i64> = f
        .iter()
        .map(|id| {
            gravar_sugestao(
                c,
                &NovaSugestao::de_valor(
                    *id,
                    &valor(Campo::Titulo, "X", 0.9),
                    OrigemEvidencia::Ia,
                    "m",
                    "v",
                ),
                1,
            )
            .unwrap()
        })
        .collect();
    let d = |lote: Option<&str>, s: i64, f: i64, acao| NovaDecisao {
        lote_id: lote.map(str::to_owned),
        alvo: AlvoDecisao::Arquivo(f),
        sugestao_id: Some(s),
        campo: Campo::Titulo,
        acao,
        valor_anterior: None,
        valor_novo: Some("Y".into()),
        autor: AutorAcao::Cli,
    };
    registrar_decisao(c, &d(Some("lote-1"), s[0], f[0], AcaoDecisao::Aceitar), 5).unwrap();
    registrar_decisao(c, &d(Some("lote-1"), s[1], f[1], AcaoDecisao::Corrigir), 5).unwrap();
    registrar_decisao(c, &d(None, s[2], f[2], AcaoDecisao::Rejeitar), 5).unwrap();
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 0);
    let sem_valor = NovaDecisao {
        valor_novo: None,
        ..d(None, s[2], f[2], AcaoDecisao::Corrigir)
    };
    assert!(
        registrar_decisao(c, &sem_valor, 5).is_err(),
        "corrigir exige valor"
    );

    assert_eq!(desfazer_lote(c, "lote-1", 9).unwrap(), 2);
    assert_eq!(desfazer_lote(c, "lote-1", 10).unwrap(), 0, "idempotente");
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 2);
    assert_eq!(
        obter_sugestao(c, s[2]).unwrap().unwrap().estado,
        EstadoSugestao::Rejeitada
    );
    let hist = decisoes_do_lote(c, "lote-1").unwrap();
    assert_eq!(hist.len(), 2, "o histórico fica");
    assert!(hist.iter().all(|h| h.desfeita_em == Some(9)));
    let avulsa = decisoes_do_alvo(c, AlvoDecisao::Arquivo(f[2])).unwrap()[0].id;
    assert!(desfazer_decisao(c, avulsa, 11).unwrap());
    assert!(!desfazer_decisao(c, avulsa, 12).unwrap());
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 3);
}

#[test]
fn revisao_pendente_pagina_por_arquivo() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 12);
    for id in &f {
        for (campo, conf) in [(Campo::Titulo, 0.9), (Campo::Autores, 0.5)] {
            gravar_sugestao(
                c,
                &sug(
                    *id,
                    &valor(campo, "v", conf),
                    OrigemEvidencia::Conteudo,
                    "v",
                ),
                1,
            )
            .unwrap();
        }
    }
    let mut total = 0;
    let mut cur = None;
    loop {
        let p = listar_revisao_pendente(c, cur, 5).unwrap();
        for i in &p.itens {
            assert_eq!(i.pendentes, 2);
            assert!(
                (i.maior_confianca - 0.9).abs() < 1e-6 && (i.menor_confianca - 0.5).abs() < 1e-6
            );
        }
        total += p.itens.len();
        match p.proximo {
            Some(n) => cur = Some(n),
            None => break,
        }
    }
    assert_eq!(total, 12);
}

#[test]
fn inferencias_cache_por_prompt() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 1)[0];
    let i = NovaInferencia {
        job_id: None,
        file_id: Some(f),
        modelo: "m".into(),
        versao_modelo: "q4".into(),
        hash_prompt: hash(5),
        tokens_entrada: 100,
        tokens_saida: 20,
        tempo_ms: 900,
        bruto_json: Some("{}".into()),
    };
    gravar_inferencia(c, &i, 1).unwrap();
    assert!(
        inferencia_em_cache(c, &hash(5), "m", "q4")
            .unwrap()
            .is_some()
    );
    assert!(
        inferencia_em_cache(c, &hash(5), "m", "q8")
            .unwrap()
            .is_none()
    );
}

fn candidato(file_id: i64, tipo: TipoDuplicidade, bytes: u64) -> NovoCandidato {
    NovoCandidato {
        file_id,
        alvo: AlvoDuplicidade::FormatoHomologado(1),
        tipo,
        evidencias_json: "[]".into(),
        risco: RiscoDuplicidade::Baixo,
        bytes,
    }
}

#[test]
fn candidatos_por_bytes_recuperaveis() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 10);
    for (i, id) in f.iter().enumerate() {
        for _ in 0..2 {
            registrar_candidato(
                c,
                &candidato(*id, TipoDuplicidade::Exata, 1000 * (i as u64 % 4 + 1)),
                5,
            )
            .unwrap();
        }
    }
    registrar_candidato(
        c,
        &candidato(f[0], TipoDuplicidade::MesmaEdicao, 999_999),
        5,
    )
    .unwrap();
    assert_eq!(contar(c, "candidatos_duplicidade"), 11);
    assert_eq!(contar_candidatos_abertos(c).unwrap(), 11);
    let esperado: u64 = (0..10u64).map(|i| 1000 * (i % 4 + 1)).sum();
    assert_eq!(bytes_recuperaveis_exatos(c).unwrap(), esperado);

    let mut bytes = Vec::new();
    let mut cur = None;
    loop {
        let p =
            listar_candidatos_por_bytes(c, EstadoCandidato::Aberto, TipoDuplicidade::Exata, cur, 4)
                .unwrap();
        bytes.extend(p.itens.iter().map(|x| x.bytes));
        match p.proximo {
            Some(n) => cur = Some(n),
            None => break,
        }
    }
    assert_eq!(bytes.len(), 10);
    assert!(bytes.windows(2).all(|w| w[0] >= w[1]));

    // Descartado como falso sai das contas, e reprocessar não o reabre.
    let id = candidatos_do_arquivo(c, f[3]).unwrap()[0].id;
    decidir_candidato(c, id, EstadoCandidato::DescartadoComoFalso, 8).unwrap();
    registrar_candidato(c, &candidato(f[3], TipoDuplicidade::Exata, 4000), 9).unwrap();
    assert_eq!(contar_candidatos_abertos(c).unwrap(), 10);
    assert_eq!(bytes_recuperaveis_exatos(c).unwrap(), esperado - 4000);
}

#[test]
fn alertas_idempotentes_vistos_e_resumo() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 1);
    let novo = |chave: &str, tipo| NovoAlerta {
        chave: Some(chave.into()),
        tipo,
        file_id: Some(f[0]),
        dados_json: "{}".into(),
    };
    let (a1, n1) = emitir_alerta(c, &novo("ja:1", TipoAlerta::JaTemEsteLivro), 100).unwrap();
    let (a2, n2) = emitir_alerta(c, &novo("ja:1", TipoAlerta::JaTemEsteLivro), 101).unwrap();
    assert!(n1 && !n2);
    assert_eq!(a1, a2);
    emitir_alerta(c, &novo("ja:2", TipoAlerta::JaTemEsteLivro), 150).unwrap();
    emitir_alerta(c, &novo("fx:1", TipoAlerta::ExtracaoFalhou), 200).unwrap();
    emitir_alerta(
        c,
        &NovoAlerta {
            chave: None,
            tipo: TipoAlerta::Outro,
            file_id: None,
            dados_json: "{}".into(),
        },
        5000,
    )
    .unwrap();
    assert_eq!(contar_nao_vistos(c).unwrap(), 4);
    let p = listar_nao_vistos(c, None, 3).unwrap();
    assert_eq!(p.itens.len(), 3);
    assert!(p.itens[0].id > p.itens[1].id);
    assert_eq!(listar_nao_vistos(c, p.proximo, 3).unwrap().itens.len(), 1);
    assert!(marcar_visto(c, a1, 300).unwrap());
    assert!(!marcar_visto(c, a1, 301).unwrap());
    silenciar_alerta(c, 3, true).unwrap();
    assert_eq!(contar_nao_vistos(c).unwrap(), 2);
    silenciar_alerta(c, 3, false).unwrap();
    assert_eq!(contar_nao_vistos(c).unwrap(), 3);
    let r = resumo_periodo(c, 0, 1000).unwrap();
    assert_eq!(r.len(), 2);
    let ja = r
        .iter()
        .find(|x| x.tipo == TipoAlerta::JaTemEsteLivro)
        .unwrap();
    assert_eq!((ja.total, ja.nao_vistos), (2, 1));
    assert_eq!(marcar_vistos_ate(c, 100, 400).unwrap(), 3);
    assert_eq!(contar_nao_vistos(c).unwrap(), 0);
}

#[test]
fn cadastros_homologacoes_e_backups() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 2);
    let h = bib(c, "/lib/h", PapelBiblioteca::Homologada);
    let ac = bib(c, "/lib/ac", PapelBiblioteca::AClassificar);
    assert!(
        registrar_cadastro(c, f[0], None, h, &hash(1), 1).is_err(),
        "só A Classificar"
    );
    let regs: Vec<_> = (0..3)
        .map(|_| registrar_cadastro(c, f[0], None, ac, &hash(1), 10).unwrap())
        .collect();
    assert_eq!(regs.iter().filter(|(_, n)| *n).count(), 1);
    assert_eq!(contar(c, "cadastros_a_classificar"), 1);
    let cad = regs[0].0.clone();
    assert_eq!(cad.chave_idempotencia, format!("dag:{}", "01".repeat(32)));

    marcar_cadastrando(c, cad.id, 11).unwrap();
    marcar_cadastro_falhou(c, cad.id, "calibredb saiu 1", 12).unwrap();
    let (r, novo) = registrar_cadastro(c, f[0], None, ac, &hash(1), 13).unwrap();
    assert!(!novo);
    assert_eq!(r.estado, EstadoCadastro::Pendente);
    marcar_cadastrando(c, cad.id, 14).unwrap();
    marcar_cadastrado(c, cad.id, 77, 15).unwrap();
    let (r, _) = registrar_cadastro(c, f[0], None, ac, &hash(1), 16).unwrap();
    assert_eq!(
        (r.estado, r.calibre_id),
        (EstadoCadastro::Cadastrado, Some(77))
    );
    registrar_cadastro(c, f[1], None, ac, &hash(2), 17).unwrap();
    assert_eq!(
        listar_cadastros(c, EstadoCadastro::Cadastrado, None, 10)
            .unwrap()
            .itens
            .len(),
        1
    );
    assert_eq!(
        listar_cadastros(c, EstadoCadastro::Pendente, None, 10)
            .unwrap()
            .itens
            .len(),
        1
    );

    assert!(propor_homologacao(c, cad.id, ac, None, None, 20).is_err());
    let hid = propor_homologacao(c, cad.id, h, Some("{}"), Some("{\"t\":1}"), 20).unwrap();
    assert!(
        registrar_aplicacao(c, hid, 5, "rb", 21).is_err(),
        "sem aprovação não aplica"
    );
    aprovar_homologacao(c, hid, "chico", AutorAcao::Gui, 22).unwrap();
    assert!(aprovar_homologacao(c, hid, "chico", AutorAcao::Gui, 23).is_err());
    registrar_aplicacao(c, hid, 5, "rb-1", 24).unwrap();
    let hom = obter_homologacao(c, hid).unwrap().unwrap();
    assert_eq!(hom.estado, EstadoHomologacao::Aplicada);
    assert_eq!(hom.aprovado_por.as_deref(), Some("chico"));
    assert_eq!(hom.rollback_id.as_deref(), Some("rb-1"));
    registrar_reversao(c, hid, 30).unwrap();
    assert!(registrar_reversao(c, hid, 31).is_err());
    assert_eq!(homologacoes_do_cadastro(c, cad.id).unwrap().len(), 1);

    let b = registrar_backup(c, h, "{\"arquivos\":3}", 40).unwrap();
    registrar_verificacao_backup(c, b, VerificacaoBackup::Ok, 41).unwrap();
    registrar_restauracao_e_cobertura(c, b, Some("{\"amostra\":10}"), Some("{\"c\":1}"), 42)
        .unwrap();
    let u = ultimo_backup(c, h).unwrap().unwrap();
    assert_eq!(u.verificacao, VerificacaoBackup::Ok);
    assert_eq!(u.restauracao_em, Some(42));
    assert_eq!(u.cobertura_json.as_deref(), Some("{\"c\":1}"));
}

#[test]
fn apagar_arquivo_do_catalogo_limpa_jobs_e_contadores() {
    let cat = cat();
    let c = cat.armazem().conexao();
    let f = arquivos(c, 2);
    enfileirar_job(c, f[0], MotivoJob::Novo, 0, None, 1).unwrap();
    enfileirar_job(c, f[1], MotivoJob::Novo, 0, None, 1).unwrap();
    gravar_sugestao(
        c,
        &sug(
            f[0],
            &valor(Campo::Titulo, "a", 0.5),
            OrigemEvidencia::Conteudo,
            "v",
        ),
        1,
    )
    .unwrap();
    assert_eq!(contagem_jobs(c).unwrap().pendentes, 2);
    c.execute("DELETE FROM files WHERE id = ?1", [f[0]])
        .unwrap();
    assert_eq!(contagem_jobs(c).unwrap().pendentes, 1);
    assert_eq!(contar_sugestoes_pendentes(c).unwrap(), 0);
}

/// Ensaio de carga: 100 mil jobs + 100 mil livros homologados (com formato e hash), medindo
/// enfileirar, pegar, buscar por hash e paginar sem carregar tudo em memória.
#[test]
fn carga_100_mil_entradas_bibliograficas() {
    use std::time::Instant;
    const N: usize = 100_000;
    let mut cat = cat();
    let ids = arquivos(cat.armazem().conexao(), N);
    let b = bib(
        cat.armazem().conexao(),
        "/lib/h",
        PapelBiblioteca::Homologada,
    );

    let t = Instant::now();
    cat.armazem_mut()
        .transacao_imediata(|tx| -> rusqlite::Result<()> {
            for (i, id) in ids.iter().enumerate() {
                enfileirar_job(
                    tx,
                    *id,
                    MotivoJob::Novo,
                    (i % 100) as i64,
                    Some("v1"),
                    1_000 + (i as i64 % 5000),
                )
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            }
            Ok(())
        })
        .unwrap();
    let t_enfileirar = t.elapsed();

    let t = Instant::now();
    cat.armazem_mut()
        .transacao_imediata(|tx| -> rusqlite::Result<()> {
            let livros = (0..N as i64)
                .map(|i| livro(i + 1, &format!("Livro {i}"), &[("PDF", 1000 + i as u64)]));
            let r =
                sincronizar_espelho(tx, b, livros, 5).map_err(|_| rusqlite::Error::InvalidQuery)?;
            assert_eq!(r.livros, N as u64);
            Ok(())
        })
        .unwrap();
    let t_espelho = t.elapsed();

    let c = cat.armazem().conexao();
    let t = Instant::now();
    c.execute_batch("BEGIN").unwrap();
    {
        let mut st = c
            .prepare("UPDATE formatos_homologados SET hash = ?2, hash_em = 9 WHERE id = ?1")
            .unwrap();
        for i in 1..=N as i64 {
            let mut h = [0u8; 32];
            h[..8].copy_from_slice(&i.to_le_bytes());
            st.execute(rusqlite::params![i, h.as_slice()]).unwrap();
        }
    }
    c.execute_batch("COMMIT").unwrap();
    let t_hashes = t.elapsed();

    let t = Instant::now();
    for i in (1..=N as i64).step_by(100) {
        let mut h = [0u8; 32];
        h[..8].copy_from_slice(&i.to_le_bytes());
        assert_eq!(
            buscar_por_hash(c, &h, Some(1000 + i as u64 - 1))
                .unwrap()
                .len(),
            1
        );
    }
    let t_busca = t.elapsed();

    let t = Instant::now();
    c.execute_batch("BEGIN").unwrap();
    for _ in 0..10_000 {
        pegar_proximo_job(c, 50_000).unwrap().unwrap();
    }
    c.execute_batch("COMMIT").unwrap();
    let t_pegar = t.elapsed();
    let ct = contagem_jobs(c).unwrap();
    assert_eq!((ct.pendentes, ct.em_andamento), (N as i64 - 10_000, 10_000));

    let t = Instant::now();
    let mut cur = None;
    let mut total = 0;
    let mut paginas = 0;
    loop {
        let p = listar_jobs(c, EstadoJob::Pendente, cur, 500).unwrap();
        total += p.itens.len();
        paginas += 1;
        match p.proximo {
            Some(n) => cur = Some(n),
            None => break,
        }
    }
    let t_pagina = t.elapsed();
    assert_eq!(total, N - 10_000);

    let t = Instant::now();
    let p = listar_livros(c, b, Some(N as i64 - 100), 50).unwrap();
    let t_fim = t.elapsed();
    assert_eq!(p.itens.len(), 50);
    assert_eq!(contar_livros(c, b).unwrap(), N as i64);

    let plano: String = c
        .prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM jobs_bibliograficos WHERE estado = 'pendente' \
             ORDER BY ordem DESC, id DESC LIMIT 1",
        )
        .unwrap()
        .query_map([], |l| l.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        plano.contains("idx_jobs_biblio_fila") && !plano.contains("TEMP B-TREE"),
        "plano: {plano}"
    );

    eprintln!(
        "CARGA {N}: enfileirar {t_enfileirar:?}; espelho {t_espelho:?}; hashes {t_hashes:?}; \
         1000 buscas por hash {t_busca:?}; pegar 10000 {t_pegar:?}; paginar fila ({paginas} páginas) {t_pagina:?}; \
         página no fim {t_fim:?}"
    );
}
