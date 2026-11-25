//! Leitura somente-leitura de `metadata.db` (B-01).
//!
//! A conexão é aberta com `file:...?mode=ro` (nunca `immutable`, porque o Calibre pode estar
//! aberto) e ainda recebe `PRAGMA query_only = ON`. Nada aqui chama o `calibredb`: abrir uma
//! biblioteca com o Calibre pode gravar (cópia de preferências, `.calnotes`, migração de
//! esquema), o que violaria a regra de nunca alterar a Homologada.
//!
//! Esquema lido (Calibre 7.x, `user_version` 26; 9.x mantém as mesmas tabelas):
//! `books`, `authors`+`books_authors_link` (ordem do link), `tags`, `series`, `publishers`,
//! `languages` (ordem `item_order`), `identifiers`, `comments`, `ratings`, `data` (formatos),
//! `custom_columns` e, por coluna `N`, `custom_column_N` e `books_custom_column_N_link`.
//! Colunas normalizadas (text, series, enumeration, rating) usam tabela de valores + link;
//! as demais (int, float, bool, datetime, comments, composite) guardam `(book, value)`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dag_biblio::{FormatoCalibre, LivroCalibre};
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::erro::{ErroCalibre, Resultado};

/// Tipo de dado de uma coluna personalizada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoColuna {
    Text,
    Comments,
    Series,
    Enumeration,
    Int,
    Float,
    Bool,
    Rating,
    Datetime,
    /// Calculada pelo Calibre a partir de um template: não é armazenada.
    Composite,
    Desconhecido(String),
}

impl TipoColuna {
    /// Interpreta `custom_columns.datatype`.
    #[must_use]
    pub fn de_texto(t: &str) -> Self {
        match t {
            "text" => Self::Text,
            "comments" => Self::Comments,
            "series" => Self::Series,
            "enumeration" => Self::Enumeration,
            "int" => Self::Int,
            "float" => Self::Float,
            "bool" => Self::Bool,
            "rating" => Self::Rating,
            "datetime" => Self::Datetime,
            "composite" => Self::Composite,
            outro => Self::Desconhecido(outro.to_owned()),
        }
    }

    /// Nome aceito por `calibredb add_custom_column`.
    #[must_use]
    pub fn nome_calibre(&self) -> &str {
        match self {
            Self::Text => "text",
            Self::Comments => "comments",
            Self::Series => "series",
            Self::Enumeration => "enumeration",
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Rating => "rating",
            Self::Datetime => "datetime",
            Self::Composite => "composite",
            Self::Desconhecido(s) => s,
        }
    }
}

/// Coluna personalizada (`custom_columns`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColunaPersonalizada {
    pub id: i64,
    /// Rótulo sem o `#` (ex.: `pasta`).
    pub rotulo: String,
    /// Nome de exibição (ex.: `!1.Pasta`).
    pub nome: String,
    pub tipo: TipoColuna,
    pub e_multipla: bool,
    /// JSON de `display` (enum_values, composite_template, number_format...).
    pub display: Value,
    pub editavel: bool,
    /// Se os valores ficam em tabela própria + link (text, series, enumeration, rating).
    pub normalizada: bool,
    /// `false` para composite: o valor é calculado pelo Calibre e não existe em disco.
    pub materializada: bool,
}

/// Monta o URI `file:...?mode=ro` com escape de percentagem.
#[must_use]
pub fn uri_somente_leitura(caminho: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file:");
    for &b in caminho.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s.push_str("?mode=ro");
    s
}

/// Abre um SQLite qualquer em modo somente-leitura.
pub(crate) fn abrir_ro(db: &Path) -> Resultado<Connection> {
    let conn = Connection::open_with_flags(
        uri_somente_leitura(db),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| ErroCalibre::sqlite(format!("abrir {}", db.display()), e))?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|e| ErroCalibre::sqlite("busy_timeout", e))?;
    conn.pragma_update(None, "query_only", "ON")
        .map_err(|e| ErroCalibre::sqlite("query_only", e))?;
    Ok(conn)
}

fn norm_data(s: &str) -> String {
    s.replacen(' ', "T", 1)
}

/// Data indefinida do Calibre (`0101-01-01`) vira `None`.
fn data_definida(s: Option<String>) -> Option<String> {
    let s = s?;
    let ano: i32 = s.get(..4)?.parse().ok()?;
    if ano <= 101 {
        None
    } else {
        Some(norm_data(&s))
    }
}

/// Biblioteca Calibre aberta só para leitura.
#[derive(Debug)]
pub struct BibliotecaLeitura {
    raiz: PathBuf,
    conn: Connection,
    colunas: Vec<ColunaPersonalizada>,
}

impl BibliotecaLeitura {
    /// Abre `<raiz>/metadata.db` em `mode=ro`.
    ///
    /// # Errors
    /// Falha se não houver `metadata.db` ou se o esquema não for do Calibre.
    pub fn abrir(raiz: &Path) -> Resultado<Self> {
        let db = raiz.join("metadata.db");
        if !db.is_file() {
            return Err(ErroCalibre::BibliotecaInexistente(raiz.to_path_buf()));
        }
        let raiz = raiz
            .canonicalize()
            .map_err(|e| ErroCalibre::io(format!("canonicalizar {}", raiz.display()), e))?;
        let conn = abrir_ro(&db)?;
        let existe: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('books','custom_columns','preferences')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| ErroCalibre::sqlite("verificar esquema", e))?;
        if existe < 3 {
            return Err(ErroCalibre::EsquemaInesperado(
                "faltam as tabelas books/custom_columns/preferences".into(),
            ));
        }
        let mut b = Self {
            raiz,
            conn,
            colunas: Vec::new(),
        };
        b.colunas = b.ler_colunas()?;
        Ok(b)
    }

    /// Raiz canônica da biblioteca.
    #[must_use]
    pub fn raiz(&self) -> &Path {
        &self.raiz
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Colunas personalizadas (sem as marcadas para exclusão).
    #[must_use]
    pub fn colunas_personalizadas(&self) -> &[ColunaPersonalizada] {
        &self.colunas
    }

    fn ler_colunas(&self) -> Resultado<Vec<ColunaPersonalizada>> {
        let mut st = self
            .conn
            .prepare(
                "SELECT id,label,name,datatype,mark_for_delete,editable,display,is_multiple,normalized \
                 FROM custom_columns ORDER BY id",
            )
            .map_err(|e| ErroCalibre::sqlite("custom_columns", e))?;
        let linhas = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                ))
            })
            .map_err(|e| ErroCalibre::sqlite("custom_columns", e))?;
        let mut out = Vec::new();
        for l in linhas {
            let (id, rotulo, nome, dt, apagar, editavel, display, multipla, normalizada) =
                l.map_err(|e| ErroCalibre::sqlite("custom_columns", e))?;
            if apagar != 0 {
                continue;
            }
            let tipo = TipoColuna::de_texto(&dt);
            let display = match display.as_deref() {
                None | Some("") => Value::Object(serde_json::Map::new()),
                Some(s) => serde_json::from_str(s)
                    .map_err(|e| ErroCalibre::json(format!("display da coluna #{rotulo}"), e))?,
            };
            out.push(ColunaPersonalizada {
                id,
                rotulo,
                nome,
                materializada: tipo != TipoColuna::Composite,
                tipo,
                e_multipla: multipla != 0,
                display,
                editavel: editavel != 0,
                normalizada: normalizada != 0,
            });
        }
        Ok(out)
    }

    /// `library_id` (UUID da biblioteca), se a tabela existir.
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn library_id(&self) -> Resultado<Option<String>> {
        match self
            .conn
            .query_row("SELECT uuid FROM library_id LIMIT 1", [], |r| r.get(0))
        {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(rusqlite::Error::SqliteFailure(_, Some(m))) if m.contains("no such table") => {
                Ok(None)
            }
            Err(e) => Err(ErroCalibre::sqlite("library_id", e)),
        }
    }

    /// `PRAGMA user_version` (versão do esquema do Calibre).
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn versao_esquema(&self) -> Resultado<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| ErroCalibre::sqlite("user_version", e))
    }

    /// Número de livros.
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn total_livros(&self) -> Resultado<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM books", [], |r| r.get::<_, i64>(0))
            .map(|n| u64::try_from(n).unwrap_or(0))
            .map_err(|e| ErroCalibre::sqlite("contar livros", e))
    }

    /// Todas as preferências cruas (`preferences.key` -> JSON). Valores que não são JSON
    /// válido são devolvidos como string.
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn preferencias(&self) -> Resultado<BTreeMap<String, Value>> {
        let mut st = self
            .conn
            .prepare("SELECT key,val FROM preferences")
            .map_err(|e| ErroCalibre::sqlite("preferences", e))?;
        let it = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| ErroCalibre::sqlite("preferences", e))?;
        let mut out = BTreeMap::new();
        for l in it {
            let (k, v) = l.map_err(|e| ErroCalibre::sqlite("preferences", e))?;
            let v = serde_json::from_str(&v).unwrap_or(Value::String(v));
            out.insert(k, v);
        }
        Ok(out)
    }

    /// Procura livros pelo identificador (`tipo:valor`, comparação sem caixa no tipo).
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn livros_por_identificador(&self, tipo: &str, valor: &str) -> Resultado<Vec<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT book FROM identifiers WHERE type=?1 AND val=?2 ORDER BY book")
            .map_err(|e| ErroCalibre::sqlite("identifiers", e))?;
        let v = st
            .query_map(params![tipo, valor], |r| r.get::<_, i64>(0))
            .map_err(|e| ErroCalibre::sqlite("identifiers", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ErroCalibre::sqlite("identifiers", e))?;
        Ok(v)
    }

    /// Itera todos os livros em páginas de `tamanho_pagina` (paginação por `books.id`).
    /// Só uma página fica na memória.
    #[must_use]
    pub fn livros(&self, tamanho_pagina: usize) -> IteradorLivros<'_> {
        IteradorLivros {
            bib: self,
            ultimo_id: 0,
            tamanho: tamanho_pagina.max(1),
            buffer: VecDeque::new(),
            fim: false,
        }
    }

    /// Um livro pelo id.
    ///
    /// # Errors
    /// Falha de SQLite.
    pub fn livro(&self, id: i64) -> Resultado<Option<LivroCalibre>> {
        Ok(self.ler_pagina(id - 1, 1)?.into_iter().find(|l| l.id == id))
    }

    /// Lê uma página: livros com `id > depois_de`, até `limite`.
    fn ler_pagina(&self, depois_de: i64, limite: usize) -> Resultado<Vec<LivroCalibre>> {
        let c = &self.conn;
        let mut livros: Vec<LivroCalibre> = Vec::new();
        {
            let mut st = c
                .prepare(
                    "SELECT id,uuid,title,sort,author_sort,series_index,path,has_cover,pubdate \
                     FROM books WHERE id>?1 ORDER BY id LIMIT ?2",
                )
                .map_err(|e| ErroCalibre::sqlite("books", e))?;
            let it = st
                .query_map(
                    params![depois_de, i64::try_from(limite).unwrap_or(i64::MAX)],
                    |r| {
                        Ok(LivroCalibre {
                            id: r.get(0)?,
                            uuid: r.get(1)?,
                            titulo: r.get(2)?,
                            titulo_ordenacao: r.get(3)?,
                            autor_ordenacao: r.get(4)?,
                            indice_serie: r.get(5)?,
                            caminho: r.get(6)?,
                            tem_capa: r.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
                            data_publicacao: data_definida(r.get(8)?),
                            ..LivroCalibre::default()
                        })
                    },
                )
                .map_err(|e| ErroCalibre::sqlite("books", e))?;
            for l in it {
                livros.push(l.map_err(|e| ErroCalibre::sqlite("books", e))?);
            }
        }
        let (Some(primeiro), Some(ultimo)) = (livros.first(), livros.last()) else {
            return Ok(livros);
        };
        let (lo, hi) = (primeiro.id, ultimo.id);
        let pos: HashMap<i64, usize> = livros.iter().enumerate().map(|(i, l)| (l.id, i)).collect();

        macro_rules! consulta {
            ($ctx:expr, $sql:expr, |$r:ident| $corpo:expr, |$livro:ident, $dado:ident| $aplica:expr) => {{
                let mut st = c.prepare($sql).map_err(|e| ErroCalibre::sqlite($ctx, e))?;
                let it = st
                    .query_map(params![lo, hi], |$r| Ok((($r).get::<_, i64>(0)?, $corpo)))
                    .map_err(|e| ErroCalibre::sqlite($ctx, e))?;
                for l in it {
                    let (b, $dado) = l.map_err(|e| ErroCalibre::sqlite($ctx, e))?;
                    if let Some(&i) = pos.get(&b) {
                        let $livro = &mut livros[i];
                        $aplica;
                    }
                }
            }};
        }

        consulta!(
            "autores",
            "SELECT l.book, a.name FROM books_authors_link l JOIN authors a ON a.id=l.author \
             WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.id",
            |r| r.get::<_, String>(1)?,
            |l, d| l.autores.push(d.replace('|', ","))
        );
        consulta!(
            "tags",
            "SELECT l.book, t.name FROM books_tags_link l JOIN tags t ON t.id=l.tag \
             WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.id",
            |r| r.get::<_, String>(1)?,
            |l, d| l.tags.push(d)
        );
        consulta!(
            "série",
            "SELECT l.book, s.name FROM books_series_link l JOIN series s ON s.id=l.series \
             WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.id",
            |r| r.get::<_, String>(1)?,
            |l, d| if l.serie.is_none() {
                l.serie = Some(d)
            }
        );
        consulta!(
            "editora",
            "SELECT l.book, p.name FROM books_publishers_link l JOIN publishers p ON p.id=l.publisher \
             WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.id",
            |r| r.get::<_, String>(1)?,
            |l, d| if l.editora.is_none() {
                l.editora = Some(d)
            }
        );
        consulta!(
            "idiomas",
            "SELECT l.book, g.lang_code FROM books_languages_link l JOIN languages g ON g.id=l.lang_code \
             WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.item_order, l.id",
            |r| r.get::<_, String>(1)?,
            |l, d| l.idiomas.push(d)
        );
        consulta!(
            "identificadores",
            "SELECT book, type, val FROM identifiers WHERE book BETWEEN ?1 AND ?2 ORDER BY book, id",
            |r| (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            |l, d| l.identificadores.push(d)
        );
        consulta!(
            "comentários",
            "SELECT book, text FROM comments WHERE book BETWEEN ?1 AND ?2",
            |r| r.get::<_, String>(1)?,
            |l, d| l.comentarios = Some(d)
        );
        consulta!(
            "avaliação",
            "SELECT l.book, r.rating FROM books_ratings_link l JOIN ratings r ON r.id=l.rating \
             WHERE l.book BETWEEN ?1 AND ?2",
            |r| r.get::<_, Option<i64>>(1)?,
            |l, d| l.avaliacao = d.filter(|v| *v > 0)
        );

        // Formatos: caminho absoluto = raiz/books.path/data.name.ext (extensão minúscula).
        {
            let mut st = c
                .prepare(
                    "SELECT book, format, name, uncompressed_size FROM data \
                     WHERE book BETWEEN ?1 AND ?2 ORDER BY book, id",
                )
                .map_err(|e| ErroCalibre::sqlite("data", e))?;
            let it = st
                .query_map(params![lo, hi], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(|e| ErroCalibre::sqlite("data", e))?;
            for l in it {
                let (b, formato, nome, tam) = l.map_err(|e| ErroCalibre::sqlite("data", e))?;
                if let Some(&i) = pos.get(&b) {
                    let livro = &mut livros[i];
                    let caminho = self
                        .raiz
                        .join(&livro.caminho)
                        .join(format!("{nome}.{}", formato.to_ascii_lowercase()));
                    livro.formatos.push(FormatoCalibre {
                        formato: formato.to_ascii_uppercase(),
                        nome,
                        tamanho: u64::try_from(tam).unwrap_or(0),
                        caminho_absoluto: caminho.to_string_lossy().into_owned(),
                    });
                }
            }
        }

        // Colunas personalizadas, valor tipado por datatype.
        for col in &self.colunas {
            if !col.materializada {
                continue; // composite: calculado pelo Calibre, não materializado
            }
            let tabela = format!("custom_column_{}", col.id);
            let link = format!("books_custom_column_{}_link", col.id);
            if col.normalizada {
                let extra = if col.tipo == TipoColuna::Series {
                    ", l.extra"
                } else {
                    ", NULL"
                };
                let sql = format!(
                    "SELECT l.book, c.value{extra} FROM {link} l JOIN {tabela} c ON c.id=l.value \
                     WHERE l.book BETWEEN ?1 AND ?2 ORDER BY l.book, l.id"
                );
                let mut st = c
                    .prepare(&sql)
                    .map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                let it = st
                    .query_map(params![lo, hi], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, rusqlite::types::Value>(1)?,
                            r.get::<_, Option<f64>>(2)?,
                        ))
                    })
                    .map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                let mut multiplos: HashMap<i64, Vec<Value>> = HashMap::new();
                for l in it {
                    let (b, v, extra) =
                        l.map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                    let Some(&i) = pos.get(&b) else { continue };
                    let valor = valor_tipado(&col.tipo, v);
                    match (&col.tipo, col.e_multipla) {
                        (TipoColuna::Series, _) => {
                            let mut o = serde_json::Map::new();
                            o.insert("serie".into(), valor);
                            o.insert("indice".into(), extra.map_or(Value::Null, Value::from));
                            livros[i]
                                .personalizados
                                .push((col.rotulo.clone(), Value::Object(o)));
                        }
                        (_, true) => multiplos.entry(b).or_default().push(valor),
                        _ => livros[i].personalizados.push((col.rotulo.clone(), valor)),
                    }
                }
                for (b, v) in multiplos {
                    if let Some(&i) = pos.get(&b) {
                        livros[i]
                            .personalizados
                            .push((col.rotulo.clone(), Value::Array(v)));
                    }
                }
            } else {
                let sql = format!(
                    "SELECT book, value FROM {tabela} WHERE book BETWEEN ?1 AND ?2 AND value IS NOT NULL"
                );
                let mut st = c
                    .prepare(&sql)
                    .map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                let it = st
                    .query_map(params![lo, hi], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, rusqlite::types::Value>(1)?))
                    })
                    .map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                for l in it {
                    let (b, v) =
                        l.map_err(|e| ErroCalibre::sqlite(format!("coluna #{}", col.rotulo), e))?;
                    if let Some(&i) = pos.get(&b) {
                        livros[i]
                            .personalizados
                            .push((col.rotulo.clone(), valor_tipado(&col.tipo, v)));
                    }
                }
            }
        }
        // Estabiliza a ordem dos personalizados (ordem das colunas).
        let ordem: HashMap<&str, usize> = self
            .colunas
            .iter()
            .enumerate()
            .map(|(i, c)| (c.rotulo.as_str(), i))
            .collect();
        for l in &mut livros {
            l.personalizados
                .sort_by_key(|(r, _)| ordem.get(r.as_str()).copied().unwrap_or(usize::MAX));
            // `indice_serie` só faz sentido com série.
            if l.serie.is_none() {
                l.indice_serie = None;
            }
        }
        Ok(livros)
    }
}

/// Converte o valor SQLite no JSON do tipo da coluna.
fn valor_tipado(tipo: &TipoColuna, v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match (tipo, v) {
        (_, V::Null) => Value::Null,
        (TipoColuna::Bool, V::Integer(i)) => Value::Bool(i != 0),
        (TipoColuna::Int | TipoColuna::Rating, V::Integer(i)) => Value::from(i),
        (TipoColuna::Int | TipoColuna::Rating, V::Real(f)) => Value::from(f as i64),
        (TipoColuna::Float, V::Real(f)) => Value::from(f),
        (TipoColuna::Float, V::Integer(i)) => Value::from(i as f64),
        (TipoColuna::Datetime, V::Text(s)) => Value::String(norm_data(&s)),
        (_, V::Text(s)) => Value::String(s),
        (_, V::Integer(i)) => Value::from(i),
        (_, V::Real(f)) => Value::from(f),
        (_, V::Blob(b)) => Value::String(String::from_utf8_lossy(&b).into_owned()),
    }
}

/// Iterador paginado sobre os livros.
#[derive(Debug)]
pub struct IteradorLivros<'a> {
    bib: &'a BibliotecaLeitura,
    ultimo_id: i64,
    tamanho: usize,
    buffer: VecDeque<LivroCalibre>,
    fim: bool,
}

impl IteradorLivros<'_> {
    /// Quantos livros estão carregados na memória neste momento (no máximo uma página).
    #[must_use]
    pub fn em_memoria(&self) -> usize {
        self.buffer.len()
    }
}

impl Iterator for IteradorLivros<'_> {
    type Item = Resultado<LivroCalibre>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buffer.is_empty() && !self.fim {
            match self.bib.ler_pagina(self.ultimo_id, self.tamanho) {
                Ok(p) => {
                    if p.len() < self.tamanho {
                        self.fim = true;
                    }
                    if let Some(u) = p.last() {
                        self.ultimo_id = u.id;
                    }
                    self.buffer = p.into();
                }
                Err(e) => {
                    self.fim = true;
                    return Some(Err(e));
                }
            }
        }
        self.buffer.pop_front().map(Ok)
    }
}
