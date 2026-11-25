//! Extrações, sugestões, decisões humanas (com desfazer em lote) e inferências de modelo.

use dag_biblio::{
    Campo, EstadoLeitura, Evidencia, FormatoLivro, OrigemEvidencia, ResultadoExtracao,
    ValorSugerido,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{AutorAcao, Pagina, enum_de_texto, hash_de_blob, invalido, limite_de, texto_enum};
use crate::Resultado;

// ---------------------------------------------------------------------------------------------
// Extrações
// ---------------------------------------------------------------------------------------------

/// Desfecho da leitura gravado na extração (espelha `dag_biblio::EstadoLeitura`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoExtracao {
    Lido,
    Drm,
    Senha,
    Corrompido,
    NaoSuportado,
    LimiteExcedido,
}

impl From<&EstadoLeitura> for EstadoExtracao {
    fn from(e: &EstadoLeitura) -> Self {
        match e {
            EstadoLeitura::Lido => Self::Lido,
            EstadoLeitura::Drm => Self::Drm,
            EstadoLeitura::Senha => Self::Senha,
            EstadoLeitura::Corrompido { .. } => Self::Corrompido,
            EstadoLeitura::NaoSuportado { .. } => Self::NaoSuportado,
            EstadoLeitura::LimiteExcedido { .. } => Self::LimiteExcedido,
        }
    }
}

/// Como o `resultado` está gravado.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compressao {
    Nenhuma,
    Zstd,
    Gzip,
}

/// Extração a gravar. `resultado` são os bytes já prontos (JSON puro ou comprimido por quem
/// chama): o Catálogo não impõe um compressor.
#[derive(Debug, Clone)]
pub struct NovaExtracao<'a> {
    pub job_id: Option<i64>,
    pub file_id: i64,
    pub formato: FormatoLivro,
    pub estado: EstadoExtracao,
    pub resultado: Option<&'a [u8]>,
    pub compressao: Compressao,
    pub paginas_total: Option<u32>,
    pub paginas_ocr: u32,
    pub tempo_ms: u64,
    pub hash_arquivo: Option<[u8; 32]>,
    pub versao_regras: &'a str,
}

/// Extração gravada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Extracao {
    pub id: i64,
    pub job_id: Option<i64>,
    pub file_id: i64,
    pub formato: String,
    pub estado: EstadoExtracao,
    #[serde(skip)]
    pub resultado: Option<Vec<u8>>,
    pub compressao: Compressao,
    pub paginas_total: Option<i64>,
    pub paginas_ocr: i64,
    pub tempo_ms: i64,
    pub hash_arquivo: Option<[u8; 32]>,
    pub versao_regras: String,
    pub criado_em: i64,
}

impl Extracao {
    /// Decodifica o `ResultadoExtracao` (só quando gravado sem compressão).
    ///
    /// # Errors
    /// JSON inválido, ou resultado comprimido (o chamador descomprime e usa `serde_json`).
    pub fn resultado_tipado(&self) -> Result<Option<ResultadoExtracao>, crate::Erro> {
        match (&self.resultado, self.compressao) {
            (None, _) => Ok(None),
            (Some(b), Compressao::Nenhuma) => Ok(Some(serde_json::from_slice(b)?)),
            _ => Err(invalido(
                "resultado comprimido: descomprima antes de decodificar",
            )),
        }
    }
}

/// Grava a extração (uma por arquivo e versão de regras: reprocessar substitui).
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_extracao(conn: &Connection, e: &NovaExtracao<'_>, agora: i64) -> Resultado<i64> {
    Ok(conn.query_row(
        "INSERT INTO extracoes (job_id, file_id, formato, estado, resultado, compressao, paginas_total,
                paginas_ocr, tempo_ms, hash_arquivo, versao_regras, criado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(file_id, versao_regras) DO UPDATE SET job_id = excluded.job_id,
            formato = excluded.formato, estado = excluded.estado, resultado = excluded.resultado,
            compressao = excluded.compressao, paginas_total = excluded.paginas_total,
            paginas_ocr = excluded.paginas_ocr, tempo_ms = excluded.tempo_ms,
            hash_arquivo = excluded.hash_arquivo, criado_em = excluded.criado_em
         RETURNING id",
        params![
            e.job_id,
            e.file_id,
            texto_enum(&e.formato),
            texto_enum(&e.estado),
            e.resultado,
            texto_enum(&e.compressao),
            e.paginas_total.map(i64::from),
            i64::from(e.paginas_ocr),
            e.tempo_ms as i64,
            e.hash_arquivo.as_ref().map(<[u8; 32]>::as_slice),
            e.versao_regras,
            agora
        ],
        |l| l.get(0),
    )?)
}

/// Atalho: serializa o `ResultadoExtracao` (JSON, sem compressão) e grava.
///
/// # Errors
/// Falha do SQLite ou da serialização.
#[allow(clippy::too_many_arguments)]
pub fn gravar_resultado_extracao(
    conn: &Connection,
    job_id: Option<i64>,
    file_id: i64,
    r: &ResultadoExtracao,
    hash_arquivo: Option<[u8; 32]>,
    versao_regras: &str,
    tempo_ms: u64,
    agora: i64,
) -> Resultado<i64> {
    let json = serde_json::to_vec(r)?;
    gravar_extracao(
        conn,
        &NovaExtracao {
            job_id,
            file_id,
            formato: r.formato,
            estado: EstadoExtracao::from(&r.estado),
            resultado: Some(&json),
            compressao: Compressao::Nenhuma,
            paginas_total: r.total_paginas,
            paginas_ocr: u32::try_from(r.paginas_ocr.len()).unwrap_or(u32::MAX),
            tempo_ms,
            hash_arquivo,
            versao_regras,
        },
        agora,
    )
}

pub(super) fn extracao_de_linha(l: &Row<'_>) -> rusqlite::Result<Extracao> {
    Ok(Extracao {
        id: l.get(0)?,
        job_id: l.get(1)?,
        file_id: l.get(2)?,
        formato: l.get(3)?,
        estado: enum_de_texto(&l.get::<_, String>(4)?)?,
        resultado: l.get(5)?,
        compressao: enum_de_texto(&l.get::<_, String>(6)?)?,
        paginas_total: l.get(7)?,
        paginas_ocr: l.get(8)?,
        tempo_ms: l.get(9)?,
        hash_arquivo: hash_de_blob(l.get(10)?),
        versao_regras: l.get(11)?,
        criado_em: l.get(12)?,
    })
}

/// A extração vigente do arquivo para a versão de regras, **se** o hash do arquivo na extração
/// é o informado (reaproveitamento válido: CA-B11). Hash diferente = arquivo mudou = `None`.
///
/// # Errors
/// Falha do SQLite.
pub fn extracao_vigente(
    conn: &Connection,
    file_id: i64,
    versao_regras: &str,
    hash_atual: &[u8; 32],
) -> Resultado<Option<Extracao>> {
    Ok(conn
        .query_row(
            "SELECT id, job_id, file_id, formato, estado, resultado, compressao, paginas_total, paginas_ocr,
                    tempo_ms, hash_arquivo, versao_regras, criado_em
             FROM extracoes WHERE file_id = ?1 AND versao_regras = ?2 AND hash_arquivo = ?3",
            params![file_id, versao_regras, hash_atual.as_slice()],
            extracao_de_linha,
        )
        .optional()?)
}

// ---------------------------------------------------------------------------------------------
// Sugestões
// ---------------------------------------------------------------------------------------------

/// Estado de uma sugestão na revisão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoSugestao {
    Pendente,
    Aceita,
    Corrigida,
    Rejeitada,
}

/// Sugestão a gravar.
#[derive(Debug, Clone, PartialEq)]
pub struct NovaSugestao {
    pub file_id: i64,
    pub edicao_id: Option<i64>,
    pub campo: Campo,
    pub valor: Option<String>,
    pub lista: Vec<String>,
    pub confianca: f32,
    pub evidencias: Vec<Evidencia>,
    pub origem: OrigemEvidencia,
    /// Modelo de IA, ou vazio para regras determinísticas.
    pub modelo: String,
    pub versao_regras: String,
}

impl NovaSugestao {
    /// A partir de um `ValorSugerido` do contrato compartilhado.
    #[must_use]
    pub fn de_valor(
        file_id: i64,
        v: &ValorSugerido,
        origem: OrigemEvidencia,
        modelo: &str,
        versao_regras: &str,
    ) -> Self {
        Self {
            file_id,
            edicao_id: None,
            campo: v.campo,
            valor: v.valor.clone(),
            lista: v.lista.clone(),
            confianca: v.confianca,
            evidencias: v.evidencias.clone(),
            origem,
            modelo: modelo.to_owned(),
            versao_regras: versao_regras.to_owned(),
        }
    }
}

/// Sugestão gravada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sugestao {
    pub id: i64,
    pub file_id: i64,
    pub edicao_id: Option<i64>,
    pub campo: Campo,
    pub valor: Option<String>,
    pub lista: Vec<String>,
    pub confianca: f32,
    pub evidencias: Vec<Evidencia>,
    pub origem: OrigemEvidencia,
    pub modelo: String,
    pub versao_regras: String,
    pub estado: EstadoSugestao,
}

impl Sugestao {
    /// De volta ao contrato compartilhado.
    #[must_use]
    pub fn para_valor_sugerido(&self) -> ValorSugerido {
        ValorSugerido {
            campo: self.campo,
            valor: self.valor.clone(),
            lista: self.lista.clone(),
            confianca: self.confianca,
            evidencias: self.evidencias.clone(),
        }
    }
}

/// Grava a sugestão (upsert por arquivo, campo, origem, modelo e versão de regras). Se o valor
/// proposto não mudou, a decisão anterior é mantida; se mudou, volta a `pendente`.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_sugestao(conn: &Connection, s: &NovaSugestao, agora: i64) -> Resultado<i64> {
    let lista = serde_json::to_string(&s.lista)?;
    Ok(conn
        .prepare_cached(
            "INSERT INTO sugestoes (file_id, edicao_id, campo, valor, lista_json, confianca, evidencias_json,
                    origem, modelo, versao_regras, criado_em, atualizado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT(file_id, campo, origem, modelo, versao_regras) DO UPDATE SET
                edicao_id = COALESCE(excluded.edicao_id, sugestoes.edicao_id),
                estado = CASE WHEN sugestoes.valor IS excluded.valor AND sugestoes.lista_json = excluded.lista_json
                              THEN sugestoes.estado ELSE 'pendente' END,
                valor = excluded.valor, lista_json = excluded.lista_json, confianca = excluded.confianca,
                evidencias_json = excluded.evidencias_json, atualizado_em = excluded.atualizado_em
             RETURNING id",
        )?
        .query_row(
            params![
                s.file_id,
                s.edicao_id,
                texto_enum(&s.campo),
                s.valor,
                lista,
                f64::from(s.confianca),
                serde_json::to_string(&s.evidencias)?,
                texto_enum(&s.origem),
                s.modelo,
                s.versao_regras,
                agora
            ],
            |l| l.get(0),
        )?)
}

pub(super) fn sugestao_de_linha(l: &Row<'_>) -> rusqlite::Result<Sugestao> {
    Ok(Sugestao {
        id: l.get(0)?,
        file_id: l.get(1)?,
        edicao_id: l.get(2)?,
        campo: enum_de_texto(&l.get::<_, String>(3)?)?,
        valor: l.get(4)?,
        lista: serde_json::from_str(&l.get::<_, String>(5)?).unwrap_or_default(),
        confianca: l.get::<_, f64>(6)? as f32,
        evidencias: serde_json::from_str(&l.get::<_, String>(7)?).unwrap_or_default(),
        origem: enum_de_texto(&l.get::<_, String>(8)?)?,
        modelo: l.get(9)?,
        versao_regras: l.get(10)?,
        estado: enum_de_texto(&l.get::<_, String>(11)?)?,
    })
}

pub(super) const COLUNAS_SUGESTAO: &str = "id, file_id, edicao_id, campo, valor, lista_json, confianca, evidencias_json, origem, modelo, versao_regras, estado";

/// Sugestões de um arquivo (todas as origens), por campo e confiança decrescente.
///
/// # Errors
/// Falha do SQLite.
pub fn sugestoes_do_arquivo(conn: &Connection, file_id: i64) -> Resultado<Vec<Sugestao>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_SUGESTAO} FROM sugestoes WHERE file_id = ?1 ORDER BY campo, confianca DESC, id"
    ))?;
    let v = st
        .query_map([file_id], sugestao_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Uma sugestão pelo id.
///
/// # Errors
/// Falha do SQLite.
pub fn obter_sugestao(conn: &Connection, id: i64) -> Resultado<Option<Sugestao>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUNAS_SUGESTAO} FROM sugestoes WHERE id = ?1"),
            [id],
            sugestao_de_linha,
        )
        .optional()?)
}

/// Um arquivo com sugestões à espera de revisão.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemRevisao {
    pub file_id: i64,
    pub pendentes: i64,
    pub maior_confianca: f64,
    pub menor_confianca: f64,
}

/// Fila de revisão: arquivos com sugestões pendentes, por cursor (`file_id`).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_revisao_pendente(
    conn: &Connection,
    apos_file_id: Option<i64>,
    limite: usize,
) -> Resultado<Pagina<ItemRevisao, i64>> {
    let limite = limite_de(limite);
    let mut st = conn.prepare(
        "SELECT file_id, COUNT(*), MAX(confianca), MIN(confianca) FROM sugestoes
         WHERE estado = 'pendente' AND file_id > ?1 GROUP BY file_id ORDER BY file_id LIMIT ?2",
    )?;
    let linhas = st
        .query_map(
            params![apos_file_id.unwrap_or(0), (limite + 1) as i64],
            |l| {
                Ok(ItemRevisao {
                    file_id: l.get(0)?,
                    pendentes: l.get(1)?,
                    maior_confianca: l.get(2)?,
                    menor_confianca: l.get(3)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Pagina::de_linhas(linhas, limite, |i| i.file_id))
}

/// Total de sugestões pendentes (contador mantido).
///
/// # Errors
/// Falha do SQLite.
pub fn contar_sugestoes_pendentes(conn: &Connection) -> Resultado<i64> {
    super::contador(conn, "biblio_sugestoes_pendentes")
}

// ---------------------------------------------------------------------------------------------
// Decisões humanas
// ---------------------------------------------------------------------------------------------

/// O que a decisão afeta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "id", rename_all = "snake_case")]
pub enum AlvoDecisao {
    Arquivo(i64),
    Edicao(i64),
    Obra(i64),
}

impl AlvoDecisao {
    fn partes(self) -> (&'static str, i64) {
        match self {
            Self::Arquivo(i) => ("arquivo", i),
            Self::Edicao(i) => ("edicao", i),
            Self::Obra(i) => ("obra", i),
        }
    }
}

/// Ação do humano.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcaoDecisao {
    Aceitar,
    Corrigir,
    Rejeitar,
}

/// Decisão a registrar.
#[derive(Debug, Clone)]
pub struct NovaDecisao {
    /// Identificador do lote (para desfazer em bloco), ou `None` para decisão avulsa.
    pub lote_id: Option<String>,
    pub alvo: AlvoDecisao,
    pub sugestao_id: Option<i64>,
    pub campo: Campo,
    pub acao: AcaoDecisao,
    pub valor_anterior: Option<String>,
    pub valor_novo: Option<String>,
    pub autor: AutorAcao,
}

/// Decisão gravada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisaoHumana {
    pub id: i64,
    pub lote_id: Option<String>,
    pub alvo: AlvoDecisao,
    pub sugestao_id: Option<i64>,
    pub campo: Campo,
    pub acao: AcaoDecisao,
    pub valor_anterior: Option<String>,
    pub valor_novo: Option<String>,
    pub autor: AutorAcao,
    pub quando: i64,
    pub desfeita_em: Option<i64>,
}

/// Registra a decisão e, havendo sugestão, a move para `aceita`/`corrigida`/`rejeitada`.
/// `Corrigir` exige `valor_novo`.
///
/// # Errors
/// Falha do SQLite ou `Corrigir` sem valor.
pub fn registrar_decisao(conn: &Connection, d: &NovaDecisao, agora: i64) -> Resultado<i64> {
    if d.acao == AcaoDecisao::Corrigir && d.valor_novo.is_none() {
        return Err(invalido("corrigir exige o valor corrigido"));
    }
    let (tipo, alvo_id) = d.alvo.partes();
    conn.execute(
        "INSERT INTO decisoes_humanas (lote_id, alvo_tipo, alvo_id, sugestao_id, campo, acao, valor_anterior,
                valor_novo, autor, quando)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            d.lote_id,
            tipo,
            alvo_id,
            d.sugestao_id,
            texto_enum(&d.campo),
            texto_enum(&d.acao),
            d.valor_anterior,
            d.valor_novo,
            texto_enum(&d.autor),
            agora
        ],
    )?;
    let id = conn.last_insert_rowid();
    if let Some(s) = d.sugestao_id {
        let estado = match d.acao {
            AcaoDecisao::Aceitar => "aceita",
            AcaoDecisao::Corrigir => "corrigida",
            AcaoDecisao::Rejeitar => "rejeitada",
        };
        conn.execute(
            "UPDATE sugestoes SET estado = ?2, atualizado_em = ?3 WHERE id = ?1",
            params![s, estado, agora],
        )?;
    }
    Ok(id)
}

pub(super) fn decisao_de_linha(l: &Row<'_>) -> rusqlite::Result<DecisaoHumana> {
    let tipo: String = l.get(2)?;
    let alvo_id: i64 = l.get(3)?;
    let alvo = match tipo.as_str() {
        "arquivo" => AlvoDecisao::Arquivo(alvo_id),
        "edicao" => AlvoDecisao::Edicao(alvo_id),
        _ => AlvoDecisao::Obra(alvo_id),
    };
    Ok(DecisaoHumana {
        id: l.get(0)?,
        lote_id: l.get(1)?,
        alvo,
        sugestao_id: l.get(4)?,
        campo: enum_de_texto(&l.get::<_, String>(5)?)?,
        acao: enum_de_texto(&l.get::<_, String>(6)?)?,
        valor_anterior: l.get(7)?,
        valor_novo: l.get(8)?,
        autor: enum_de_texto(&l.get::<_, String>(9)?)?,
        quando: l.get(10)?,
        desfeita_em: l.get(11)?,
    })
}

pub(super) const COLUNAS_DECISAO: &str = "id, lote_id, alvo_tipo, alvo_id, sugestao_id, campo, acao, valor_anterior, valor_novo, autor, quando, desfeita_em";

/// Decisões de um lote.
///
/// # Errors
/// Falha do SQLite.
pub fn decisoes_do_lote(conn: &Connection, lote_id: &str) -> Resultado<Vec<DecisaoHumana>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_DECISAO} FROM decisoes_humanas WHERE lote_id = ?1 ORDER BY id"
    ))?;
    let v = st
        .query_map([lote_id], decisao_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Histórico de decisões de um alvo (mais nova primeiro), inclusive as desfeitas.
///
/// # Errors
/// Falha do SQLite.
pub fn decisoes_do_alvo(conn: &Connection, alvo: AlvoDecisao) -> Resultado<Vec<DecisaoHumana>> {
    let (tipo, id) = alvo.partes();
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS_DECISAO} FROM decisoes_humanas WHERE alvo_tipo = ?1 AND alvo_id = ?2 ORDER BY id DESC"
    ))?;
    let v = st
        .query_map(params![tipo, id], decisao_de_linha)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Desfaz um lote: marca as decisões como desfeitas (o histórico fica) e devolve a `pendente`
/// as sugestões que ficaram sem nenhuma decisão válida. Idempotente. Devolve quantas decisões
/// foram desfeitas agora.
///
/// # Errors
/// Falha do SQLite.
pub fn desfazer_lote(conn: &Connection, lote_id: &str, agora: i64) -> Resultado<u64> {
    let n = conn.execute(
        "UPDATE decisoes_humanas SET desfeita_em = ?2 WHERE lote_id = ?1 AND desfeita_em IS NULL",
        params![lote_id, agora],
    )?;
    conn.execute(
        "UPDATE sugestoes SET estado = 'pendente', atualizado_em = ?2
         WHERE id IN (SELECT sugestao_id FROM decisoes_humanas WHERE lote_id = ?1 AND sugestao_id IS NOT NULL)
           AND NOT EXISTS (SELECT 1 FROM decisoes_humanas d
                           WHERE d.sugestao_id = sugestoes.id AND d.desfeita_em IS NULL)",
        params![lote_id, agora],
    )?;
    Ok(n as u64)
}

/// Desfaz uma decisão avulsa (mesma regra de [`desfazer_lote`]). Devolve se desfez.
///
/// # Errors
/// Falha do SQLite.
pub fn desfazer_decisao(conn: &Connection, decisao_id: i64, agora: i64) -> Resultado<bool> {
    let n = conn.execute(
        "UPDATE decisoes_humanas SET desfeita_em = ?2 WHERE id = ?1 AND desfeita_em IS NULL",
        params![decisao_id, agora],
    )?;
    conn.execute(
        "UPDATE sugestoes SET estado = 'pendente', atualizado_em = ?2
         WHERE id = (SELECT sugestao_id FROM decisoes_humanas WHERE id = ?1)
           AND NOT EXISTS (SELECT 1 FROM decisoes_humanas d
                           WHERE d.sugestao_id = sugestoes.id AND d.desfeita_em IS NULL)",
        params![decisao_id, agora],
    )?;
    Ok(n == 1)
}

// ---------------------------------------------------------------------------------------------
// Inferências
// ---------------------------------------------------------------------------------------------

/// Chamada a um modelo, para auditoria e cache por prompt.
#[derive(Debug, Clone)]
pub struct NovaInferencia {
    pub job_id: Option<i64>,
    pub file_id: Option<i64>,
    pub modelo: String,
    pub versao_modelo: String,
    pub hash_prompt: [u8; 32],
    pub tokens_entrada: u32,
    pub tokens_saida: u32,
    pub tempo_ms: u64,
    pub bruto_json: Option<String>,
}

/// Inferência gravada.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Inferencia {
    pub id: i64,
    pub job_id: Option<i64>,
    pub file_id: Option<i64>,
    pub modelo: String,
    pub versao_modelo: String,
    pub tokens_entrada: i64,
    pub tokens_saida: i64,
    pub tempo_ms: i64,
    pub bruto_json: Option<String>,
    pub criado_em: i64,
}

/// Grava a inferência e devolve o id.
///
/// # Errors
/// Falha do SQLite.
pub fn gravar_inferencia(conn: &Connection, i: &NovaInferencia, agora: i64) -> Resultado<i64> {
    conn.execute(
        "INSERT INTO inferencias (job_id, file_id, modelo, versao_modelo, hash_prompt, tokens_entrada,
                tokens_saida, tempo_ms, bruto_json, criado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            i.job_id,
            i.file_id,
            i.modelo,
            i.versao_modelo,
            i.hash_prompt.as_slice(),
            i64::from(i.tokens_entrada),
            i64::from(i.tokens_saida),
            i.tempo_ms as i64,
            i.bruto_json,
            agora
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A inferência mais recente com este prompt (hash), modelo e versão: evita repetir a chamada.
///
/// # Errors
/// Falha do SQLite.
pub fn inferencia_em_cache(
    conn: &Connection,
    hash_prompt: &[u8; 32],
    modelo: &str,
    versao_modelo: &str,
) -> Resultado<Option<Inferencia>> {
    Ok(conn
        .query_row(
            "SELECT id, job_id, file_id, modelo, versao_modelo, tokens_entrada, tokens_saida, tempo_ms,
                    bruto_json, criado_em FROM inferencias
             WHERE hash_prompt = ?1 AND modelo = ?2 AND versao_modelo = ?3 ORDER BY id DESC LIMIT 1",
            params![hash_prompt.as_slice(), modelo, versao_modelo],
            |l| {
                Ok(Inferencia {
                    id: l.get(0)?,
                    job_id: l.get(1)?,
                    file_id: l.get(2)?,
                    modelo: l.get(3)?,
                    versao_modelo: l.get(4)?,
                    tokens_entrada: l.get(5)?,
                    tokens_saida: l.get(6)?,
                    tempo_ms: l.get(7)?,
                    bruto_json: l.get(8)?,
                    criado_em: l.get(9)?,
                })
            },
        )
        .optional()?)
}
