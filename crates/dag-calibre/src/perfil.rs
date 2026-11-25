//! Perfil da biblioteca (B-01, B-02): inventário completo, vocabulário, estantes candidatas e
//! relatório de compatibilidade para clonar a estrutura na A Classificar.
//!
//! Tudo é lido de `metadata.db` em `mode=ro`. A "estante" NÃO é presumida como tag: o perfil
//! devolve candidatos ranqueados por evidência (nome da coluna, uso em bibliotecas virtuais,
//! cobertura) e quem decide é o dono.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::erro::{ErroCalibre, Resultado};
use crate::leitura::{BibliotecaLeitura, ColunaPersonalizada, TipoColuna};

/// Item do vocabulário com contagem de livros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemContado {
    pub nome: String,
    pub contagem: u64,
}

/// Autor com `author_sort` e contagem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutorContado {
    pub nome: String,
    pub ordenacao: Option<String>,
    pub contagem: u64,
}

/// Formato com contagem de arquivos e bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoContado {
    pub formato: String,
    pub contagem: u64,
    pub bytes: u64,
}

/// Formato de tamanho zero no `metadata.db`: não indexar por hash, tratar como quebrado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoQuebrado {
    pub livro_id: i64,
    pub formato: String,
    pub caminho_absoluto: String,
}

/// Vocabulário da biblioteca.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Vocabulario {
    /// Tags de assunto (sem as de controle), ordenadas por contagem decrescente.
    pub tags: Vec<ItemContado>,
    /// Tags que começam com `!` (ex.: `!Catalogar`): fila de trabalho, não assunto.
    pub tags_controle: Vec<ItemContado>,
    pub autores: Vec<AutorContado>,
    pub series: Vec<ItemContado>,
    pub editoras: Vec<ItemContado>,
    pub idiomas: Vec<ItemContado>,
    pub formatos: Vec<FormatoContado>,
}

/// Busca nomeada (biblioteca virtual ou busca salva).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuscaNomeada {
    pub nome: String,
    pub busca: String,
}

/// Categoria de usuário (`user_categories`): nome e itens crus `[valor, categoria, ...]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoriaUsuario {
    pub nome: String,
    pub itens: Vec<Value>,
}

/// De onde veio um candidato a estante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrigemEstante {
    ColunaPersonalizada,
    CategoriaUsuario,
    BibliotecaVirtual,
    TagHierarquica,
}

/// Valor de estante com a quantidade de livros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValorContado {
    pub valor: String,
    pub contagem: u64,
}

/// Candidato a "estante", com a razão da detecção. Nunca é uma decisão: o dono confirma.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidatoEstante {
    pub origem: OrigemEstante,
    /// Nome de exibição (coluna, categoria ou biblioteca virtual).
    pub nome: String,
    /// Rótulo `#x` quando for coluna personalizada.
    pub rotulo: Option<String>,
    pub valores: Vec<ValorContado>,
    /// Livros distintos com algum valor; `None` quando só o motor de busca do Calibre sabe.
    pub contagem: Option<u64>,
    /// Pontuação (maior = mais provável). Soma das evidências em `razoes`.
    pub pontuacao: u32,
    pub razoes: Vec<String>,
}

/// Situação de um elemento no clone para a A Classificar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "estado", rename_all = "snake_case")]
pub enum StatusClone {
    Clonavel,
    ClonavelComAviso { avisos: Vec<String> },
    NaoClonavel { motivos: Vec<String> },
}

/// Linha do relatório de compatibilidade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCompatibilidade {
    /// `coluna #pasta`, `bibliotecas virtuais`, ...
    pub elemento: String,
    pub status: StatusClone,
}

/// O que é clonável para a A Classificar e o que não é (sem perda silenciosa).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatorioCompatibilidade {
    pub itens: Vec<ItemCompatibilidade>,
}

impl RelatorioCompatibilidade {
    /// Itens não clonáveis.
    #[must_use]
    pub fn nao_clonaveis(&self) -> Vec<&ItemCompatibilidade> {
        self.itens
            .iter()
            .filter(|i| matches!(i.status, StatusClone::NaoClonavel { .. }))
            .collect()
    }

    /// Estado de um elemento pelo nome exato.
    #[must_use]
    pub fn status_de(&self, elemento: &str) -> Option<&StatusClone> {
        self.itens
            .iter()
            .find(|i| i.elemento == elemento)
            .map(|i| &i.status)
    }
}

/// Versão do Calibre que gravou a biblioteca e como foi detectada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersaoGravacao {
    /// Ex.: `7.6.0`.
    pub versao: String,
    /// Onde foi lido (`metadata.opf` de um livro recente).
    pub origem: String,
}

/// Perfil completo de uma biblioteca Calibre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfilBiblioteca {
    pub caminho: PathBuf,
    /// UUID da biblioteca: cópias/espelhos antigos compartilham este valor.
    pub library_id: Option<String>,
    /// `PRAGMA user_version` do `metadata.db`.
    pub versao_esquema: i64,
    /// Versão do Calibre que gravou por último (se detectável; o `metadata.db` não a guarda).
    pub versao_calibre_gravacao: Option<VersaoGravacao>,
    pub total_livros: u64,
    pub total_autores: u64,
    pub total_tags: u64,
    pub total_series: u64,
    pub total_editoras: u64,
    pub total_arquivos_formato: u64,
    pub colunas: Vec<ColunaPersonalizada>,
    /// Formatos com `uncompressed_size = 0` (a Homologada do Chico tem 3).
    pub formatos_quebrados: Vec<FormatoQuebrado>,
    pub vocabulario: Vocabulario,
    pub bibliotecas_virtuais: Vec<BuscaNomeada>,
    pub buscas_salvas: Vec<BuscaNomeada>,
    pub categorias_usuario: Vec<CategoriaUsuario>,
    pub termos_agrupados: BTreeMap<String, Vec<String>>,
    /// Todas as preferências do banco, exceto `field_metadata` (redundante com as colunas).
    pub preferencias_brutas: BTreeMap<String, Value>,
    /// Candidatos a estante, do mais provável ao menos.
    pub candidatos_estante: Vec<CandidatoEstante>,
    pub compatibilidade: RelatorioCompatibilidade,
}

/// Lê o perfil completo (somente leitura).
///
/// # Errors
/// Falha de abertura/consulta.
pub fn ler_perfil(raiz: &Path) -> Resultado<PerfilBiblioteca> {
    let bib = BibliotecaLeitura::abrir(raiz)?;
    perfil_de(&bib)
}

fn contar(bib: &BibliotecaLeitura, sql: &str) -> Resultado<u64> {
    bib.conn()
        .query_row(sql, [], |r| r.get::<_, i64>(0))
        .map(|n| u64::try_from(n).unwrap_or(0))
        .map_err(|e| ErroCalibre::sqlite(sql.to_owned(), e))
}

fn itens_contados(bib: &BibliotecaLeitura, sql: &str) -> Resultado<Vec<ItemContado>> {
    let mut st = bib
        .conn()
        .prepare(sql)
        .map_err(|e| ErroCalibre::sqlite(sql.to_owned(), e))?;
    let v = st
        .query_map([], |r| {
            Ok(ItemContado {
                nome: r.get(0)?,
                contagem: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
            })
        })
        .map_err(|e| ErroCalibre::sqlite(sql.to_owned(), e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ErroCalibre::sqlite(sql.to_owned(), e))?;
    Ok(v)
}

/// Monta o perfil a partir de uma biblioteca já aberta.
///
/// # Errors
/// Falha de consulta.
pub fn perfil_de(bib: &BibliotecaLeitura) -> Resultado<PerfilBiblioteca> {
    let tags = itens_contados(
        bib,
        "SELECT t.name, COUNT(l.book) FROM tags t LEFT JOIN books_tags_link l ON l.tag=t.id \
         GROUP BY t.id ORDER BY 2 DESC, t.name",
    )?;
    let (tags_controle, tags): (Vec<_>, Vec<_>) =
        tags.into_iter().partition(|t| t.nome.starts_with('!'));
    let autores = {
        let mut st = bib
            .conn()
            .prepare(
                "SELECT a.name, a.sort, COUNT(l.book) FROM authors a \
                 LEFT JOIN books_authors_link l ON l.author=a.id GROUP BY a.id ORDER BY 3 DESC, a.name",
            )
            .map_err(|e| ErroCalibre::sqlite("autores", e))?;
        st.query_map([], |r| {
            Ok(AutorContado {
                nome: r.get::<_, String>(0)?.replace('|', ","),
                ordenacao: r.get(1)?,
                contagem: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            })
        })
        .map_err(|e| ErroCalibre::sqlite("autores", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ErroCalibre::sqlite("autores", e))?
    };
    let series = itens_contados(
        bib,
        "SELECT s.name, COUNT(l.book) FROM series s LEFT JOIN books_series_link l ON l.series=s.id \
         GROUP BY s.id ORDER BY 2 DESC, s.name",
    )?;
    let editoras = itens_contados(
        bib,
        "SELECT p.name, COUNT(l.book) FROM publishers p LEFT JOIN books_publishers_link l ON l.publisher=p.id \
         GROUP BY p.id ORDER BY 2 DESC, p.name",
    )?;
    let idiomas = itens_contados(
        bib,
        "SELECT g.lang_code, COUNT(l.book) FROM languages g LEFT JOIN books_languages_link l ON l.lang_code=g.id \
         GROUP BY g.id ORDER BY 2 DESC, g.lang_code",
    )?;
    let formatos = {
        let mut st = bib
            .conn()
            .prepare(
                "SELECT UPPER(format), COUNT(*), COALESCE(SUM(uncompressed_size),0) FROM data \
                 GROUP BY UPPER(format) ORDER BY 3 DESC",
            )
            .map_err(|e| ErroCalibre::sqlite("formatos", e))?;
        st.query_map([], |r| {
            Ok(FormatoContado {
                formato: r.get(0)?,
                contagem: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                bytes: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            })
        })
        .map_err(|e| ErroCalibre::sqlite("formatos", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ErroCalibre::sqlite("formatos", e))?
    };

    let mut prefs = bib.preferencias()?;
    prefs.remove("field_metadata");
    let bibliotecas_virtuais = buscas_de(prefs.get("virtual_libraries"));
    let buscas_salvas = buscas_de(prefs.get("saved_searches"));
    let categorias_usuario = match prefs.get("user_categories") {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| CategoriaUsuario {
                nome: k.clone(),
                itens: v.as_array().cloned().unwrap_or_default(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let termos_agrupados = match prefs.get("grouped_search_terms") {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            })
            .collect(),
        _ => BTreeMap::new(),
    };

    let colunas = bib.colunas_personalizadas().to_vec();
    let total_livros = bib.total_livros()?;
    let candidatos_estante = detectar_estantes(
        bib,
        &colunas,
        &bibliotecas_virtuais,
        &categorias_usuario,
        &vocabulario_tags(&tags, &tags_controle),
        total_livros,
    )?;
    let compatibilidade = relatorio_compatibilidade(
        &colunas,
        &bibliotecas_virtuais,
        &buscas_salvas,
        &categorias_usuario,
        &termos_agrupados,
        &prefs,
    );

    Ok(PerfilBiblioteca {
        caminho: bib.raiz().to_path_buf(),
        library_id: bib.library_id()?,
        versao_esquema: bib.versao_esquema()?,
        versao_calibre_gravacao: detectar_versao_gravacao(bib),
        total_livros,
        total_autores: contar(bib, "SELECT COUNT(*) FROM authors")?,
        total_tags: contar(bib, "SELECT COUNT(*) FROM tags")?,
        total_series: contar(bib, "SELECT COUNT(*) FROM series")?,
        total_editoras: contar(bib, "SELECT COUNT(*) FROM publishers")?,
        total_arquivos_formato: contar(bib, "SELECT COUNT(*) FROM data")?,
        colunas,
        formatos_quebrados: formatos_quebrados(bib)?,
        vocabulario: Vocabulario {
            tags,
            tags_controle,
            autores,
            series,
            editoras,
            idiomas,
            formatos,
        },
        bibliotecas_virtuais,
        buscas_salvas,
        categorias_usuario,
        termos_agrupados,
        preferencias_brutas: prefs,
        candidatos_estante,
        compatibilidade,
    })
}

fn formatos_quebrados(bib: &BibliotecaLeitura) -> Resultado<Vec<FormatoQuebrado>> {
    let mut st = bib
        .conn()
        .prepare(
            "SELECT d.book, d.format, d.name, b.path FROM data d JOIN books b ON b.id=d.book \
             WHERE d.uncompressed_size = 0 ORDER BY d.book LIMIT 5000",
        )
        .map_err(|e| ErroCalibre::sqlite("formatos quebrados", e))?;
    let v = st
        .query_map([], |r| {
            let (nome, path, fmt): (String, String, String) = (r.get(2)?, r.get(3)?, r.get(1)?);
            Ok(FormatoQuebrado {
                livro_id: r.get(0)?,
                caminho_absoluto: bib
                    .raiz()
                    .join(path)
                    .join(format!("{nome}.{}", fmt.to_ascii_lowercase()))
                    .to_string_lossy()
                    .into_owned(),
                formato: fmt.to_ascii_uppercase(),
            })
        })
        .map_err(|e| ErroCalibre::sqlite("formatos quebrados", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ErroCalibre::sqlite("formatos quebrados", e))?;
    Ok(v)
}

fn vocabulario_tags(a: &[ItemContado], b: &[ItemContado]) -> Vec<String> {
    a.iter().chain(b).map(|t| t.nome.clone()).collect()
}

fn buscas_de(v: Option<&Value>) -> Vec<BuscaNomeada> {
    match v {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| BuscaNomeada {
                nome: k.clone(),
                busca: v.as_str().map_or_else(|| v.to_string(), str::to_owned),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Procura `calibre (X.Y.Z)` nos `metadata.opf` dos livros mais recentemente modificados
/// (o Calibre grava a sua versão em cada OPF de backup). Só lê arquivos pequenos.
fn detectar_versao_gravacao(bib: &BibliotecaLeitura) -> Option<VersaoGravacao> {
    let mut st = bib
        .conn()
        .prepare("SELECT path FROM books ORDER BY last_modified DESC LIMIT 20")
        .ok()?;
    let caminhos: Vec<String> = st
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .filter_map(Result::ok)
        .collect();
    let mut melhor: Option<(Vec<u32>, String, String)> = None;
    for c in caminhos {
        let opf = bib.raiz().join(&c).join("metadata.opf");
        let Ok(txt) = std::fs::read_to_string(&opf) else {
            continue;
        };
        if let Some(v) = versao_em_opf(&txt) {
            let chave: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
            if melhor.as_ref().is_none_or(|(k, _, _)| chave > *k) {
                melhor = Some((chave, v, format!("{c}/metadata.opf")));
            }
        }
    }
    melhor.map(|(_, versao, origem)| VersaoGravacao { versao, origem })
}

pub(crate) fn versao_em_opf(txt: &str) -> Option<String> {
    let i = txt.find("calibre (")?;
    let resto = &txt[i + "calibre (".len()..];
    let f = resto.find(')')?;
    let v = &resto[..f];
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Some(v.to_owned())
    } else {
        None
    }
}

/// Remove acentos comuns e deixa só `a-z0-9` minúsculos, para comparar nomes.
pub(crate) fn normalizar(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'Ê' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'Í' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ô' | 'Õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' => 'u',
            'ç' | 'Ç' => 'c',
            c => c.to_ascii_lowercase(),
        })
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// Filtros de biblioteca virtual da forma `#rotulo:"=Valor"` (ou `#rotulo:=Valor`).
pub(crate) fn filtro_por_coluna(busca: &str) -> Option<(String, String)> {
    let b = busca.trim();
    let b = b.strip_prefix('#')?;
    let (rot, resto) = b.split_once(':')?;
    if rot.is_empty() || !rot.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let resto = resto.trim();
    let valor = resto.strip_prefix('"').map_or_else(
        || resto.strip_prefix('=').map(str::to_owned),
        |r| {
            r.strip_suffix('"')
                .and_then(|r| r.strip_prefix('='))
                .map(str::to_owned)
        },
    )?;
    if valor.is_empty() || valor.contains('"') {
        return None;
    }
    Some((rot.to_owned(), valor))
}

fn valores_da_coluna(
    bib: &BibliotecaLeitura,
    col: &ColunaPersonalizada,
) -> Resultado<(Vec<ValorContado>, u64)> {
    if !col.materializada {
        return Ok((Vec::new(), 0));
    }
    let tabela = format!("custom_column_{}", col.id);
    let link = format!("books_custom_column_{}_link", col.id);
    let (sql_v, sql_n) = if col.normalizada {
        (
            format!(
                "SELECT CAST(c.value AS TEXT), COUNT(l.book) FROM {tabela} c \
                 LEFT JOIN {link} l ON l.value=c.id GROUP BY c.id ORDER BY 2 DESC, 1"
            ),
            format!("SELECT COUNT(DISTINCT book) FROM {link}"),
        )
    } else {
        (
            format!(
                "SELECT CAST(value AS TEXT), COUNT(*) FROM {tabela} WHERE value IS NOT NULL \
                 GROUP BY value ORDER BY 2 DESC, 1 LIMIT 500"
            ),
            format!("SELECT COUNT(*) FROM {tabela} WHERE value IS NOT NULL"),
        )
    };
    let mut st = bib
        .conn()
        .prepare(&sql_v)
        .map_err(|e| ErroCalibre::sqlite(format!("valores de #{}", col.rotulo), e))?;
    let valores = st
        .query_map([], |r| {
            Ok(ValorContado {
                valor: r.get(0)?,
                contagem: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
            })
        })
        .map_err(|e| ErroCalibre::sqlite(format!("valores de #{}", col.rotulo), e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ErroCalibre::sqlite(format!("valores de #{}", col.rotulo), e))?;
    let n = contar(bib, &sql_n)?;
    Ok((valores, n))
}

fn detectar_estantes(
    bib: &BibliotecaLeitura,
    colunas: &[ColunaPersonalizada],
    vls: &[BuscaNomeada],
    categorias: &[CategoriaUsuario],
    tags: &[String],
    total_livros: u64,
) -> Resultado<Vec<CandidatoEstante>> {
    const FORTES: [&str; 4] = ["estante", "shelf", "shelves", "prateleira"];
    const FRACAS: [&str; 5] = ["pasta", "folder", "colecao", "bookshelf", "armario"];
    let mut out: Vec<CandidatoEstante> = Vec::new();

    // Colunas personalizadas.
    for col in colunas {
        let mut pontos = 0u32;
        let mut razoes = Vec::new();
        let nomes = [normalizar(&col.nome), normalizar(&col.rotulo)];
        if let Some(k) = FORTES
            .iter()
            .find(|k| nomes.iter().any(|n| n.contains(**k)))
        {
            pontos += 100;
            razoes.push(format!("nome/rótulo lembra \"{k}\""));
        } else if let Some(k) = FRACAS
            .iter()
            .find(|k| nomes.iter().any(|n| n.contains(**k)))
        {
            pontos += 40;
            razoes.push(format!("nome/rótulo lembra \"{k}\""));
        }
        let usos: Vec<&BuscaNomeada> = vls
            .iter()
            .filter(|v| filtro_por_coluna(&v.busca).is_some_and(|(r, _)| r == col.rotulo))
            .collect();
        if !usos.is_empty() {
            pontos += 20 + 5 * u32::try_from(usos.len().min(20)).unwrap_or(20);
            razoes.push(format!(
                "{} biblioteca(s) virtual(is) filtram por esta coluna",
                usos.len()
            ));
        }
        if pontos == 0 {
            continue;
        }
        let (valores, n) = valores_da_coluna(bib, col)?;
        if col.tipo == TipoColuna::Text && !col.e_multipla {
            pontos += 10;
            razoes.push("texto de valor único (um livro, uma estante)".into());
        }
        if total_livros > 0 && n * 10 >= total_livros * 8 {
            pontos += 15;
            razoes.push(format!("cobre {n} de {total_livros} livros (>= 80%)"));
        }
        if !col.materializada {
            razoes.push("composite: calculada, sem valores armazenados".into());
        }
        out.push(CandidatoEstante {
            origem: OrigemEstante::ColunaPersonalizada,
            nome: col.nome.clone(),
            rotulo: Some(format!("#{}", col.rotulo)),
            valores,
            contagem: Some(n),
            pontuacao: pontos,
            razoes,
        });
    }

    // Categorias de usuário: sempre candidatas (são agrupamentos escolhidos pelo dono).
    for c in categorias {
        let valores = c
            .itens
            .iter()
            .filter_map(|i| i.get(0).and_then(Value::as_str))
            .map(|v| ValorContado {
                valor: v.to_owned(),
                contagem: 0,
            })
            .collect();
        out.push(CandidatoEstante {
            origem: OrigemEstante::CategoriaUsuario,
            nome: c.nome.clone(),
            rotulo: None,
            valores,
            contagem: None,
            pontuacao: 30,
            razoes: vec!["categoria de usuário definida no Calibre".into()],
        });
    }

    // Bibliotecas virtuais: as que filtram uma coluna já contam como evidência da coluna;
    // as demais entram como candidatas próprias (a busca não é avaliável sem o Calibre).
    for v in vls {
        if filtro_por_coluna(&v.busca).is_some() {
            continue;
        }
        out.push(CandidatoEstante {
            origem: OrigemEstante::BibliotecaVirtual,
            nome: v.nome.clone(),
            rotulo: None,
            valores: vec![ValorContado {
                valor: v.busca.clone(),
                contagem: 0,
            }],
            contagem: None,
            pontuacao: 15,
            razoes: vec![
                "biblioteca virtual (busca arbitrária; contagem só pelo motor de busca do Calibre)"
                    .into(),
            ],
        });
    }

    // Tags hierárquicas "Estante.X".
    let prefixos = ["estante", "shelf", "prateleira"];
    let mut hier: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in tags {
        if let Some((raiz, resto)) = t.split_once('.') {
            if prefixos.contains(&normalizar(raiz).as_str()) {
                hier.entry(raiz.to_owned())
                    .or_default()
                    .push(resto.to_owned());
            }
        }
    }
    for (raiz, filhos) in hier {
        out.push(CandidatoEstante {
            origem: OrigemEstante::TagHierarquica,
            nome: raiz.clone(),
            rotulo: None,
            valores: filhos
                .into_iter()
                .map(|v| ValorContado {
                    valor: v,
                    contagem: 0,
                })
                .collect(),
            contagem: None,
            pontuacao: 50,
            razoes: vec![format!("tags hierárquicas \"{raiz}.*\"")],
        });
    }

    out.sort_by(|a, b| b.pontuacao.cmp(&a.pontuacao).then(a.nome.cmp(&b.nome)));
    Ok(out)
}

/// Verifica se um template composite é seguro e autossuficiente para clonar.
fn avaliar_composite(col: &ColunaPersonalizada, todas: &[ColunaPersonalizada]) -> StatusClone {
    let tpl = col
        .display
        .get("composite_template")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut motivos = Vec::new();
    let mut avisos =
        vec!["valor calculado pelo Calibre: o DAG não o materializa nem o grava".to_owned()];
    if tpl.trim_start().starts_with("python:") {
        motivos.push("template em Python: código arbitrário não é copiado".to_owned());
    }
    let mut resto = tpl;
    while let Some(i) = resto.find("#") {
        let nome: String = resto[i + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !nome.is_empty()
            && !todas
                .iter()
                .any(|c| c.rotulo == nome && c.rotulo != col.rotulo)
        {
            motivos.push(format!(
                "o template usa #{nome}, que não existe nesta biblioteca"
            ));
        }
        resto = &resto[i + 1..];
    }
    if tpl.contains("book_db_get") || tpl.contains("template_functions") {
        avisos.push("o template usa funções que podem depender de plugins".to_owned());
    }
    if motivos.is_empty() {
        StatusClone::ClonavelComAviso { avisos }
    } else {
        StatusClone::NaoClonavel { motivos }
    }
}

fn relatorio_compatibilidade(
    colunas: &[ColunaPersonalizada],
    vls: &[BuscaNomeada],
    buscas: &[BuscaNomeada],
    categorias: &[CategoriaUsuario],
    termos: &BTreeMap<String, Vec<String>>,
    prefs: &BTreeMap<String, Value>,
) -> RelatorioCompatibilidade {
    let mut itens = Vec::new();
    for c in colunas {
        let status = match &c.tipo {
            TipoColuna::Composite => avaliar_composite(c, colunas),
            TipoColuna::Desconhecido(t) => StatusClone::NaoClonavel {
                motivos: vec![format!("tipo de dado desconhecido: {t}")],
            },
            t => {
                let mut avisos = Vec::new();
                if !c.editavel {
                    avisos.push("coluna não editável no original".to_owned());
                }
                if *t == TipoColuna::Enumeration
                    && c.display.get("enum_values").is_none_or(|v| !v.is_array())
                {
                    avisos.push("enumeração sem enum_values no display".to_owned());
                }
                if avisos.is_empty() {
                    StatusClone::Clonavel
                } else {
                    StatusClone::ClonavelComAviso { avisos }
                }
            }
        };
        itens.push(ItemCompatibilidade {
            elemento: format!("coluna #{}", c.rotulo),
            status,
        });
    }
    let nc = |motivo: &str| StatusClone::NaoClonavel {
        motivos: vec![motivo.to_owned()],
    };
    if !vls.is_empty() {
        itens.push(ItemCompatibilidade {
            elemento: format!("bibliotecas virtuais ({})", vls.len()),
            status: nc(
                "o calibredb não tem comando para criar bibliotecas virtuais; ficam só no perfil do DAG",
            ),
        });
    }
    if !buscas.is_empty() {
        itens.push(ItemCompatibilidade {
            elemento: format!("buscas salvas ({})", buscas.len()),
            status: StatusClone::Clonavel,
        });
    }
    if !categorias.is_empty() {
        itens.push(ItemCompatibilidade {
            elemento: format!("categorias de usuário ({})", categorias.len()),
            status: nc("sem comando no calibredb; só pela interface gráfica"),
        });
    }
    if !termos.is_empty() {
        itens.push(ItemCompatibilidade {
            elemento: format!("termos de busca agrupados ({})", termos.len()),
            status: nc("sem comando no calibredb; só pela interface gráfica"),
        });
    }
    for chave in [
        "column_display",
        "tag_browser_hidden_categories",
        "categories_using_hierarchy",
        "tag_browser_category_order",
    ] {
        if prefs.get(chave).is_some_and(|v| match v {
            Value::Object(m) => !m.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Null => false,
            _ => true,
        }) {
            itens.push(ItemCompatibilidade {
                elemento: format!("preferência {chave}"),
                status: nc("preferência de interface sem comando no calibredb"),
            });
        }
    }
    itens.push(ItemCompatibilidade {
        elemento: "configurações de interface (gui.json, ícones, plugins)".into(),
        status: nc("ficam fora da biblioteca (no perfil do usuário do Calibre); não são lidas nem copiadas"),
    });
    itens.push(ItemCompatibilidade {
        elemento: "livros, capas, anotações, notas e índice de texto".into(),
        status: nc("não se clona conteúdo: a A Classificar nasce vazia"),
    });
    RelatorioCompatibilidade { itens }
}
