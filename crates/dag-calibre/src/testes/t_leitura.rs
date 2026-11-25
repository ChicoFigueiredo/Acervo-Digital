#![allow(clippy::unwrap_used, clippy::expect_used)]
use rusqlite::types::Value as V;
use serde_json::json;

use super::fixtures::{Sintetica, cria_coluna};
use crate::instantaneo::{Escopo, comparar, tirar};
use crate::leitura::{BibliotecaLeitura, TipoColuna, uri_somente_leitura};
use crate::perfil::{OrigemEstante, StatusClone, ler_perfil, versao_em_opf};

fn montar(dir: &std::path::Path) -> Sintetica {
    let s = Sintetica::nova(dir, "uuid-h");
    let c = &s.conn;
    let texto = cria_coluna(c, "pasta", "!1.Pasta", "text", false, "{}");
    let multi = cria_coluna(c, "tagsx", "Tags X", "text", true, "{}");
    let serie = cria_coluna(c, "serie2", "Coleção", "series", false, "{}");
    let enu = cria_coluna(
        c,
        "enu",
        "Nível",
        "enumeration",
        false,
        r#"{"enum_values":["a","b"]}"#,
    );
    let int = cria_coluna(c, "inteiro", "Pág", "int", false, "{}");
    let flt = cria_coluna(c, "flt", "Peso", "float", false, "{}");
    let boo = cria_coluna(c, "flag", "Scan", "bool", false, "{}");
    let rat = cria_coluna(c, "nota", "Nota", "rating", false, "{}");
    let dt = cria_coluna(c, "quando", "Lido", "datetime", false, "{}");
    let com = cria_coluna(c, "texto", "Resenha", "comments", false, "{}");
    let _calc = cria_coluna(
        c,
        "calc",
        "Calc",
        "composite",
        false,
        r#"{"composite_template":"{title}"}"#,
    );
    let _py = cria_coluna(
        c,
        "py",
        "Py",
        "composite",
        false,
        r#"{"composite_template":"python: return 1"}"#,
    );
    let _ref = cria_coluna(
        c,
        "ref",
        "Ref",
        "composite",
        false,
        r##"{"composite_template":"{#naoexiste}"}"##,
    );
    s.livro(1, "Cálculo — 微分", "Autor/Calc (1)");
    s.livro(2, "Dois", "Autor/Dois (2)");
    c.execute_batch(
        "INSERT INTO authors(id,name,sort) VALUES (1,'José Álvaro',' Álvaro, José'),(2,'Maria|Çelik','Çelik, Maria');
         INSERT INTO books_authors_link(book,author) VALUES (1,2),(1,1);
         INSERT INTO tags(id,name) VALUES (1,'Matemática'),(2,'!Catalogar');
         INSERT INTO books_tags_link(book,tag) VALUES (1,1),(1,2),(2,1);
         INSERT INTO series(id,name) VALUES (1,'Série Única');
         INSERT INTO books_series_link(book,series) VALUES (1,1);
         UPDATE books SET series_index=2.5, pubdate='2020-05-06 00:00:00+00:00', has_cover=1 WHERE id=1;
         INSERT INTO publishers(id,name) VALUES (1,'São Jorge');
         INSERT INTO books_publishers_link(book,publisher) VALUES (1,1);
         INSERT INTO languages(id,lang_code) VALUES (1,'por'),(2,'eng');
         INSERT INTO books_languages_link(book,lang_code,item_order) VALUES (1,2,1),(1,1,0);
         INSERT INTO identifiers(book,type,val) VALUES (1,'isbn','9788535902778'),(1,'dag','abc');
         INSERT INTO comments(book,text) VALUES (1,'<p>oi</p>');
         INSERT INTO ratings(id,rating) VALUES (1,8);
         INSERT INTO books_ratings_link(book,rating) VALUES (1,1);
         INSERT INTO data(book,format,uncompressed_size,name) VALUES (1,'EPUB',100,'Calc - Autor'),(1,'PDF',0,'Calc - Autor');",
    )
    .unwrap();
    s.valor_norm(texto, 1, V::Text("Matemática".into()), None);
    s.valor_norm(texto, 2, V::Text("Xadrez".into()), None);
    s.valor_norm(multi, 1, V::Text("um".into()), None);
    s.valor_norm(multi, 1, V::Text("dois".into()), None);
    s.valor_norm(serie, 1, V::Text("Coleção Azul".into()), Some(2.5));
    s.valor_norm(enu, 1, V::Text("b".into()), None);
    s.valor_norm(rat, 1, V::Integer(6), None);
    s.valor_direto(int, 1, V::Integer(300));
    s.valor_direto(flt, 1, V::Real(1.5));
    s.valor_direto(boo, 1, V::Integer(1));
    s.valor_direto(boo, 2, V::Integer(0));
    s.valor_direto(dt, 1, V::Text("2021-03-04 00:00:00+00:00".into()));
    s.valor_direto(com, 1, V::Text("<p>resenha</p>".into()));
    s.pref(
        "virtual_libraries",
        r##"{"Matemática":"#pasta:\"=Matemática\"","Xadrez":"#pasta:\"=Xadrez\"","Inglês+Mat":"languages:=eng and tags:=Matemática"}"##,
    );
    s.pref("saved_searches", r#"{"s1":"tags:=x"}"#);
    s.pref(
        "user_categories",
        r#"{"Minha Estante":[["Matemática","tags",0]]}"#,
    );
    s.pref("grouped_search_terms", "{}");
    s.pref("field_metadata", "{}");
    s
}

#[test]
fn uri_escapa_caracteres_especiais() {
    let u = uri_somente_leitura(std::path::Path::new("/tmp/a b?c#d%/Çé/metadata.db"));
    assert!(u.starts_with("file:/tmp/a%20b%3Fc%23d%25/"));
    assert!(u.ends_with("?mode=ro"));
}

#[test]
fn leitura_tipada_de_todos_os_datatypes() {
    let t = tempfile::tempdir().unwrap();
    let _s = montar(t.path());
    let b = BibliotecaLeitura::abrir(t.path()).unwrap();
    assert_eq!(b.total_livros().unwrap(), 2);
    let livros: Vec<_> = b.livros(1).map(Result::unwrap).collect();
    let l = &livros[0];
    assert_eq!(l.titulo, "Cálculo — 微分");
    assert_eq!(
        l.autores,
        ["Maria,Çelik", "José Álvaro"],
        "ordem do link e '|' -> ','"
    );
    assert_eq!(l.tags, ["Matemática", "!Catalogar"]);
    assert_eq!(l.serie.as_deref(), Some("Série Única"));
    assert_eq!(l.indice_serie, Some(2.5));
    assert_eq!(l.editora.as_deref(), Some("São Jorge"));
    assert_eq!(l.idiomas, ["por", "eng"], "ordem item_order");
    assert!(
        l.identificadores
            .contains(&("isbn".into(), "9788535902778".into()))
    );
    assert_eq!(
        l.data_publicacao.as_deref(),
        Some("2020-05-06T00:00:00+00:00")
    );
    assert_eq!(l.comentarios.as_deref(), Some("<p>oi</p>"));
    assert_eq!(l.avaliacao, Some(8));
    assert!(l.tem_capa);
    let p: std::collections::BTreeMap<_, _> = l.personalizados.iter().cloned().collect();
    assert_eq!(p["pasta"], json!("Matemática"));
    assert_eq!(p["tagsx"], json!(["um", "dois"]));
    assert_eq!(p["serie2"], json!({"serie":"Coleção Azul","indice":2.5}));
    assert_eq!(p["enu"], json!("b"));
    assert_eq!(p["inteiro"], json!(300));
    assert_eq!(p["flt"], json!(1.5));
    assert_eq!(p["flag"], json!(true));
    assert_eq!(p["nota"], json!(6));
    assert_eq!(p["quando"], json!("2021-03-04T00:00:00+00:00"));
    assert_eq!(p["texto"], json!("<p>resenha</p>"));
    assert!(!p.contains_key("calc"), "composite não é materializado");
    assert_eq!(l.formatos.len(), 2);
    assert!(
        l.formatos[0]
            .caminho_absoluto
            .ends_with("Autor/Calc (1)/Calc - Autor.epub")
    );
    let l2 = &livros[1];
    assert_eq!(l2.indice_serie, None);
    assert_eq!(l2.data_publicacao, None, "0101-01-01 = indefinida");
    assert!(
        l2.personalizados
            .iter()
            .any(|(r, v)| r == "flag" && *v == json!(false))
    );
    let c = b
        .colunas_personalizadas()
        .iter()
        .find(|c| c.rotulo == "calc")
        .unwrap();
    assert_eq!(c.tipo, TipoColuna::Composite);
    assert!(!c.materializada);
}

#[test]
fn conexao_e_somente_leitura() {
    let t = tempfile::tempdir().unwrap();
    let _s = montar(t.path());
    let b = BibliotecaLeitura::abrir(t.path()).unwrap();
    assert!(b.conn().execute("DELETE FROM books", []).is_err());
    assert!(b.conn().execute("CREATE TABLE x(a)", []).is_err());
    assert_eq!(b.total_livros().unwrap(), 2);
}

#[test]
fn ler_nao_altera_nada() {
    let t = tempfile::tempdir().unwrap();
    let s = montar(t.path());
    drop(s);
    let antes = tirar(t.path(), Escopo::Completo, None).unwrap();
    let b = BibliotecaLeitura::abrir(t.path()).unwrap();
    let _: Vec<_> = b.livros(1).collect();
    let _ = ler_perfil(t.path()).unwrap();
    drop(b);
    let depois = tirar(t.path(), Escopo::Completo, None).unwrap();
    assert_eq!(comparar(&antes, &depois), vec![]);
}

#[test]
fn perfil_estantes_tags_de_controle_e_compatibilidade() {
    let t = tempfile::tempdir().unwrap();
    let _s = montar(t.path());
    std::fs::create_dir_all(t.path().join("Autor/Calc (1)")).unwrap();
    std::fs::write(
        t.path().join("Autor/Calc (1)/metadata.opf"),
        r#"<dc:contributor role="bkp">calibre (9.14.0) [https://calibre-ebook.com]</dc:contributor>"#,
    )
    .unwrap();
    let p = ler_perfil(t.path()).unwrap();
    assert_eq!(p.library_id.as_deref(), Some("uuid-h"));
    assert_eq!(p.versao_esquema, 26);
    assert_eq!(p.versao_calibre_gravacao.as_ref().unwrap().versao, "9.14.0");
    assert_eq!((p.total_livros, p.total_autores, p.total_tags), (2, 2, 2));
    assert_eq!(p.vocabulario.tags_controle[0].nome, "!Catalogar");
    assert!(p.vocabulario.tags.iter().all(|t| !t.nome.starts_with('!')));
    assert_eq!(p.vocabulario.tags[0].contagem, 2);
    let maria = p
        .vocabulario
        .autores
        .iter()
        .find(|a| a.nome == "Maria,Çelik")
        .unwrap();
    assert_eq!(maria.ordenacao.as_deref(), Some("Çelik, Maria"));
    let f = p
        .vocabulario
        .formatos
        .iter()
        .find(|f| f.formato == "EPUB")
        .unwrap();
    assert_eq!((f.contagem, f.bytes), (1, 100));
    assert_eq!(p.formatos_quebrados.len(), 1);
    assert_eq!(p.formatos_quebrados[0].formato, "PDF");
    assert_eq!(p.bibliotecas_virtuais.len(), 3);
    assert_eq!(p.buscas_salvas[0].nome, "s1");
    assert_eq!(p.categorias_usuario[0].nome, "Minha Estante");
    let c0 = &p.candidatos_estante[0];
    assert_eq!(c0.origem, OrigemEstante::ColunaPersonalizada);
    assert_eq!(c0.rotulo.as_deref(), Some("#pasta"));
    assert_eq!(c0.contagem, Some(2));
    assert!(
        c0.valores
            .iter()
            .any(|v| v.valor == "Xadrez" && v.contagem == 1)
    );
    assert!(
        c0.razoes
            .iter()
            .any(|r| r.contains("biblioteca(s) virtual"))
    );
    assert!(
        p.candidatos_estante
            .iter()
            .any(|c| c.origem == OrigemEstante::CategoriaUsuario)
    );
    assert!(
        p.candidatos_estante
            .iter()
            .any(|c| c.origem == OrigemEstante::BibliotecaVirtual && c.nome == "Inglês+Mat")
    );
    let r = &p.compatibilidade;
    assert_eq!(r.status_de("coluna #pasta"), Some(&StatusClone::Clonavel));
    assert!(matches!(
        r.status_de("coluna #calc"),
        Some(StatusClone::ClonavelComAviso { .. })
    ));
    assert!(matches!(
        r.status_de("coluna #py"),
        Some(StatusClone::NaoClonavel { .. })
    ));
    assert!(matches!(
        r.status_de("coluna #ref"),
        Some(StatusClone::NaoClonavel { .. })
    ));
    assert!(
        r.itens
            .iter()
            .any(|i| i.elemento.starts_with("bibliotecas virtuais"))
    );
    assert!(
        r.itens
            .iter()
            .any(|i| i.elemento.starts_with("categorias de usuário"))
    );
}

#[test]
fn versao_em_opf_ignora_lixo() {
    assert_eq!(
        versao_em_opf("x calibre (7.6.0) y").as_deref(),
        Some("7.6.0")
    );
    assert_eq!(versao_em_opf("calibre (abc)"), None);
    assert_eq!(versao_em_opf("nada"), None);
}

#[test]
fn carga_leve_20_mil_livros_e_paginada() {
    let t = tempfile::tempdir().unwrap();
    let s = Sintetica::nova(t.path(), "uuid-grande");
    let col = cria_coluna(&s.conn, "pasta", "Pasta", "text", false, "{}");
    s.conn.execute_batch("BEGIN").unwrap();
    {
        let mut b = s
            .conn
            .prepare("INSERT INTO books(id,title,sort,series_index,author_sort,path,uuid,pubdate) VALUES (?1,?2,?2,1.0,'a',?3,?4,'2020-01-01 00:00:00+00:00')")
            .unwrap();
        let mut a = s
            .conn
            .prepare("INSERT INTO books_authors_link(book,author) VALUES (?1,1)")
            .unwrap();
        let mut tg = s
            .conn
            .prepare("INSERT INTO books_tags_link(book,tag) VALUES (?1,?2)")
            .unwrap();
        let mut d = s
            .conn
            .prepare(
                "INSERT INTO data(book,format,uncompressed_size,name) VALUES (?1,'PDF',10,'x')",
            )
            .unwrap();
        let mut cl = s
            .conn
            .prepare(&format!(
                "INSERT INTO books_custom_column_{col}_link(book,value) VALUES (?1,1)"
            ))
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO authors(id,name,sort) VALUES (1,'Autor','Autor')",
                [],
            )
            .unwrap();
        for t in 1..=50 {
            s.conn
                .execute(
                    "INSERT INTO tags(id,name) VALUES (?1,?2)",
                    rusqlite::params![t, format!("t{t}")],
                )
                .unwrap();
        }
        s.conn
            .execute(
                &format!("INSERT INTO custom_column_{col}(id,value) VALUES (1,'P')"),
                [],
            )
            .unwrap();
        for i in 1..=20_000_i64 {
            b.execute(rusqlite::params![
                i,
                format!("Livro {i}"),
                format!("A/L ({i})"),
                format!("u{i}")
            ])
            .unwrap();
            a.execute([i]).unwrap();
            tg.execute([i, i % 50 + 1]).unwrap();
            d.execute([i]).unwrap();
            cl.execute([i]).unwrap();
        }
    }
    s.conn.execute_batch("COMMIT").unwrap();
    drop(s);
    let b = BibliotecaLeitura::abrir(t.path()).unwrap();
    assert_eq!(b.total_livros().unwrap(), 20_000);
    let inicio = std::time::Instant::now();
    let mut it = b.livros(500);
    let (mut n, mut pico) = (0u64, 0usize);
    while let Some(l) = it.next() {
        let l = l.unwrap();
        pico = pico.max(it.em_memoria());
        assert_eq!(l.autores, ["Autor"]);
        assert_eq!(l.tags.len(), 1);
        assert_eq!(l.formatos.len(), 1);
        assert_eq!(l.personalizados.len(), 1);
        n += 1;
    }
    assert_eq!(n, 20_000);
    assert!(pico <= 500, "no máximo uma página na memória, pico {pico}");
    eprintln!("20 mil livros lidos em {:?}", inicio.elapsed());
    assert!(inicio.elapsed() < std::time::Duration::from_secs(60));
}
