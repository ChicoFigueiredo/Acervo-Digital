//! Marcação de Backup e Marcação para Deleção (A-27, A-29, A-30). **Só o Catálogo muda**:
//! nenhum arquivo do acervo é aberto, movido ou apagado.
//!
//! Toda mudança grava a linha de `marcacoes_historico` **na mesma transação**; o histórico é
//! somente de acréscimo (triggers do esquema recusam UPDATE e DELETE).

use dag_core::regras::{Marcacao, validar_nova_marcacao};
use dag_core::{
    AlertaUnicaCopia, Alvo, ArquivoId, Autor, EscopoMarcacao, EstadoMarcacao, GrupoId,
    HashCompleto, MarcacaoId, MarcacaoInfo, OrigemMarcacao, PastaId, ResultadoMarcacao,
    TipoMarcacao, VolumeId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::resolver::{resolver_arquivo, resolver_pasta, resolver_volume};
use super::{
    ErroDuplicados, HASH_VALIDO, MarcaEfetiva, Resultado, autor_de, autor_str, carregar_grupo,
    eh_unica_copia, estado_de, estado_str, hash_de, hash_valido, marca_efetiva, representante_sql,
    sob_sql, tipo_de, tipo_str,
};
use crate::Catalogo;

const AMOSTRA_UNICAS: usize = 20;

const SELECT_MARCACAO: &str = "SELECT m.id, m.tipo, m.escopo, m.volume_id, m.folder_id, m.file_id,
        m.grupo_id, m.estado, m.hash_na_marcacao, m.autor, m.criada_em, m.atualizada_em,
        COALESCE(f.path_display, pf.path_display, v.rotulo, v.platform_id, '')
 FROM marcacoes m
 LEFT JOIN files f ON f.id = m.file_id
 LEFT JOIN folders pf ON pf.id = m.folder_id
 LEFT JOIN volumes v ON v.id = m.volume_id";

fn info_da_linha(l: &rusqlite::Row<'_>) -> rusqlite::Result<MarcacaoInfo> {
    let tipo: String = l.get(1)?;
    let escopo: String = l.get(2)?;
    let estado: String = l.get(7)?;
    let hash: Option<Vec<u8>> = l.get(8)?;
    let autor: String = l.get(9)?;
    Ok(MarcacaoInfo {
        id: MarcacaoId(l.get(0)?),
        tipo: tipo_de(&tipo),
        estado: estado_de(&estado),
        escopo: EscopoMarcacao::de_str(&escopo).unwrap_or(EscopoMarcacao::ManualArquivo),
        volume: l.get::<_, Option<i64>>(3)?.map(VolumeId),
        pasta: l.get::<_, Option<i64>>(4)?.map(PastaId),
        arquivo: l.get::<_, Option<i64>>(5)?.map(ArquivoId),
        grupo: l.get::<_, Option<i64>>(6)?.map(GrupoId),
        alvo: l.get(12)?,
        hash_na_marcacao: hash.as_deref().and_then(hash_de),
        autor: autor_de(&autor),
        criada_em: l.get(10)?,
        atualizada_em: l.get(11)?,
    })
}

fn ler(conn: &Connection, id: i64) -> Resultado<Option<MarcacaoInfo>> {
    Ok(conn
        .query_row(
            &format!("{SELECT_MARCACAO} WHERE m.id = ?1"),
            [id],
            info_da_linha,
        )
        .optional()?)
}

#[allow(clippy::too_many_arguments)]
fn historico(
    tx: &Connection,
    evento: &str,
    m: &MarcacaoInfo,
    anterior: Option<EstadoMarcacao>,
    novo: Option<EstadoMarcacao>,
    hash: Option<&HashCompleto>,
    autor: Autor,
    agora: i64,
    detalhe: Option<&str>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO marcacoes_historico
            (marcacao_id, evento, tipo, escopo, estado_anterior, estado_novo, volume_id, folder_id,
             file_id, grupo_id, caminho_display, hash, autor, em, detalhe)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            m.id.0,
            evento,
            tipo_str(m.tipo),
            m.escopo.como_str(),
            anterior.map(estado_str),
            novo.map(estado_str),
            m.volume.map(|v| v.0),
            m.pasta.map(|p| p.0),
            m.arquivo.map(|a| a.0),
            m.grupo.map(|g| g.0),
            m.alvo,
            hash.or(m.hash_na_marcacao.as_ref()).map(|h| h.0.to_vec()),
            autor_str(autor),
            agora,
            detalhe,
        ],
    )?;
    Ok(())
}

fn nome_tipo(t: TipoMarcacao) -> &'static str {
    match t {
        TipoMarcacao::Backup => "Marcação de Backup",
        TipoMarcacao::Delecao => "Marcação para Deleção",
    }
}

fn nome_escopo(e: EscopoMarcacao) -> &'static str {
    match e {
        EscopoMarcacao::RegraVolume => "regra do Volume",
        EscopoMarcacao::RegraPasta => "regra de pasta",
        EscopoMarcacao::ManualArquivo => "manual",
        EscopoMarcacao::ManualGrupo => "manual, por grupo",
    }
}

#[allow(clippy::too_many_arguments)]
fn inserir(
    tx: &Connection,
    tipo: TipoMarcacao,
    escopo: EscopoMarcacao,
    volume: Option<i64>,
    pasta: Option<i64>,
    arquivo: Option<i64>,
    grupo: Option<i64>,
    hash: Option<&HashCompleto>,
    autor: Autor,
    agora: i64,
) -> Resultado<MarcacaoInfo> {
    let id: i64 = tx.query_row(
        // O id nunca é reaproveitado (nem o de uma marcação removida): o histórico não tem FK e
        // identifica a marcação por esse número.
        "INSERT INTO marcacoes (id, tipo, escopo, volume_id, folder_id, file_id, grupo_id, estado,
                                hash_na_marcacao, autor, criada_em, atualizada_em)
         VALUES (
            1 + MAX(COALESCE((SELECT MAX(id) FROM marcacoes), 0),
                    COALESCE((SELECT MAX(marcacao_id) FROM marcacoes_historico), 0)),
            ?1, ?2, ?3, ?4, ?5, ?6, 'ativa', ?7, ?8, ?9, ?9) RETURNING id",
        params![
            tipo_str(tipo),
            escopo.como_str(),
            volume,
            pasta,
            arquivo,
            grupo,
            hash.map(|h| h.0.to_vec()),
            autor_str(autor),
            agora
        ],
        |l| l.get(0),
    )?;
    let info =
        ler(tx, id)?.ok_or_else(|| ErroDuplicados::NaoEncontrado("marcação sumiu".into()))?;
    historico(
        tx,
        "criada",
        &info,
        None,
        Some(EstadoMarcacao::Ativa),
        None,
        autor,
        agora,
        None,
    )?;
    Ok(info)
}

/// Fixa numa cópia a Marcação para Deleção que hoje vale por regra de pasta ou de Volume,
/// criando a Marcação manual equivalente (mesmo hash). Usada ao mover para a Lixeira: fora da
/// pasta de origem a regra não alcança mais a cópia. Devolve `false` se não havia o que fixar.
///
/// # Errors
/// [`ErroDuplicados::SemHash`] se a cópia não tem Hash Completo válido.
pub fn fixar_delecao_individual(
    cat: &mut Catalogo,
    file_id: i64,
    autor: Autor,
    agora: i64,
) -> Resultado<bool> {
    cat.armazem_mut().transacao_imediata(|tx| {
        let manual: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM marcacoes WHERE file_id = ?1)",
            [file_id],
            |l| l.get::<_, i64>(0),
        )? != 0;
        if manual {
            return Ok(false);
        }
        let Some(m) = marca_efetiva(tx, file_id)? else {
            return Ok(false);
        };
        if m.tipo != TipoMarcacao::Delecao || m.estado != EstadoMarcacao::Ativa {
            return Ok(false);
        }
        let hash = hash_valido(tx, file_id)?.ok_or_else(|| {
            ErroDuplicados::SemHash("a cópia não tem Hash Completo válido".into())
        })?;
        inserir(
            tx,
            TipoMarcacao::Delecao,
            EscopoMarcacao::ManualArquivo,
            None,
            None,
            Some(file_id),
            None,
            Some(&hash),
            autor,
            agora,
        )?;
        Ok(true)
    })
}

/// Cria Marcação de Backup ou para Deleção no alvo.
///
/// * `Caminho`/`Arquivo`: Marcação manual de um arquivo (guarda o Hash Completo do momento).
/// * `Grupo`: Backup = **todas** as cópias do grupo; Deleção = todas **menos a Cópia
///   Principal**. Cópias que já têm a mesma Marcação são deixadas como estão.
/// * `Volume`/`Pasta`: regra; vale para todos os arquivos sob o alvo, inclusive futuros.
///
/// Uma cópia tem no máximo uma Marcação: qualquer combinação que daria duas é recusada com
/// [`ErroDuplicados::Conflito`], sem gravar nada. Marcação para Deleção em arquivo que não tem
/// nenhuma outra cópia vem com alerta de Única Cópia no resultado.
///
/// # Errors
/// [`ErroDuplicados`]: alvo inexistente/ambíguo, arquivo sem Hash Completo, conflito.
pub fn marcar(
    cat: &mut Catalogo,
    tipo: TipoMarcacao,
    alvo: &Alvo,
    autor: Autor,
    agora: i64,
) -> Resultado<ResultadoMarcacao> {
    cat.armazem_mut()
        .transacao_imediata(|tx| marcar_tx(tx, tipo, alvo, autor, agora))
}

fn marcar_tx(
    tx: &Transaction<'_>,
    tipo: TipoMarcacao,
    alvo: &Alvo,
    autor: Autor,
    agora: i64,
) -> Resultado<ResultadoMarcacao> {
    let mut r = ResultadoMarcacao::default();
    match alvo {
        Alvo::Caminho(_) | Alvo::Arquivo(_) => {
            let f = resolver_arquivo(tx, alvo)?;
            let caminho: String =
                tx.query_row("SELECT path_display FROM files WHERE id = ?1", [f], |l| {
                    l.get(0)
                })?;
            let hash = hash_valido(tx, f)?.ok_or_else(|| {
                ErroDuplicados::SemHash(format!(
                    "o arquivo {caminho} ainda não tem Hash Completo válido; rode `dag scan` com hash (sem --sem-hash) antes de marcar"
                ))
            })?;
            if let Some(m) = marca_efetiva(tx, f)? {
                let existentes = [Marcacao {
                    arquivo: ArquivoId(f),
                    tipo: m.tipo,
                    estado: m.estado,
                    origem: if m.escopo.eh_regra() {
                        OrigemMarcacao::Regra
                    } else {
                        OrigemMarcacao::Manual
                    },
                    hash_na_marcacao: hash,
                }];
                if let Err(e) = validar_nova_marcacao(&existentes, ArquivoId(f)) {
                    let pendente = if m.estado == EstadoMarcacao::Pendente {
                        format!(" Ela está pendente: confirme com `dag confirmar {}`.", m.id)
                    } else {
                        String::new()
                    };
                    return Err(ErroDuplicados::Conflito(format!(
                        "o arquivo {caminho} já tem {} ({}, marcação {}): uma cópia só pode ter uma Marcação. Remova-a antes com `dag desmarcar`.{pendente}",
                        nome_tipo(e.existente),
                        nome_escopo(m.escopo),
                        m.id
                    )));
                }
            }
            let info = inserir(
                tx,
                tipo,
                EscopoMarcacao::ManualArquivo,
                None,
                None,
                Some(f),
                None,
                Some(&hash),
                autor,
                agora,
            )?;
            if tipo == TipoMarcacao::Delecao && eh_unica_copia(tx, f)? {
                r.unicas_copias_total = 1;
                r.unicas_copias.push(AlertaUnicaCopia {
                    arquivo: ArquivoId(f),
                    caminho,
                });
            }
            r.marcacoes.push(info);
        }
        Alvo::Grupo(g) => {
            let grupo = carregar_grupo(tx, g.0)?
                .ok_or_else(|| ErroDuplicados::NaoEncontrado(format!("grupo {g} não existe")))?;
            let principal = grupo
                .principal
                .or_else(|| grupo.copias.first().map(|c| c.arquivo));
            let mut novos = Vec::new();
            let mut conflitos = Vec::new();
            for c in &grupo.copias {
                if tipo == TipoMarcacao::Delecao && Some(c.arquivo) == principal {
                    continue;
                }
                match c.marca {
                    Some(m) if m.tipo == tipo => r.ja_marcadas += 1,
                    Some(m) => {
                        conflitos.push(format!("{} ({} {})", c.caminho, nome_tipo(m.tipo), m.id))
                    }
                    None => novos.push(c),
                }
            }
            if !conflitos.is_empty() {
                let n = conflitos.len();
                conflitos.truncate(3);
                return Err(ErroDuplicados::Conflito(format!(
                    "{n} cópia(s) do grupo {g} já têm Marcação de outro tipo e uma cópia só pode ter uma: {}. Remova-as antes com `dag desmarcar`.",
                    conflitos.join("; ")
                )));
            }
            for c in novos {
                let hash = hash_valido(tx, c.arquivo)?.ok_or_else(|| {
                    ErroDuplicados::SemHash(format!(
                        "o arquivo {} perdeu o Hash Completo válido; refaça o scan",
                        c.caminho
                    ))
                })?;
                r.marcacoes.push(inserir(
                    tx,
                    tipo,
                    EscopoMarcacao::ManualGrupo,
                    None,
                    None,
                    Some(c.arquivo),
                    Some(g.0),
                    Some(&hash),
                    autor,
                    agora,
                )?);
            }
        }
        Alvo::Volume(ref_) => {
            let v = resolver_volume(tx, ref_)?;
            if let Some(t) = regra_existente(tx, Some(v), None)? {
                return Err(ErroDuplicados::Conflito(format!(
                    "o Volume já tem uma regra de {}; remova-a antes com `dag desmarcar --volume`",
                    nome_tipo(t)
                )));
            }
            conflito_regra(tx, v, None, tipo)?;
            let info = inserir(
                tx,
                tipo,
                EscopoMarcacao::RegraVolume,
                Some(v),
                None,
                None,
                None,
                None,
                autor,
                agora,
            )?;
            if tipo == TipoMarcacao::Delecao {
                unicas_sob(tx, v, None, &mut r)?;
            }
            r.marcacoes.push(info);
        }
        Alvo::Pasta(p) => {
            let (pasta, v) = resolver_pasta(tx, p)?;
            if let Some(t) = regra_existente(tx, None, Some(pasta))? {
                return Err(ErroDuplicados::Conflito(format!(
                    "a pasta já tem uma regra de {}; remova-a antes com `dag desmarcar --pasta`",
                    nome_tipo(t)
                )));
            }
            conflito_regra(tx, v, Some(pasta), tipo)?;
            let info = inserir(
                tx,
                tipo,
                EscopoMarcacao::RegraPasta,
                None,
                Some(pasta),
                None,
                None,
                None,
                autor,
                agora,
            )?;
            if tipo == TipoMarcacao::Delecao {
                unicas_sob(tx, v, Some(pasta), &mut r)?;
            }
            r.marcacoes.push(info);
        }
    }
    Ok(r)
}

fn regra_existente(
    tx: &Connection,
    volume: Option<i64>,
    pasta: Option<i64>,
) -> rusqlite::Result<Option<TipoMarcacao>> {
    let t: Option<String> = match (volume, pasta) {
        (Some(v), _) => tx
            .query_row(
                "SELECT tipo FROM marcacoes WHERE escopo = 'regra_volume' AND volume_id = ?1",
                [v],
                |l| l.get(0),
            )
            .optional()?,
        (_, Some(p)) => tx
            .query_row(
                "SELECT tipo FROM marcacoes WHERE escopo = 'regra_pasta' AND folder_id = ?1",
                [p],
                |l| l.get(0),
            )
            .optional()?,
        _ => None,
    };
    Ok(t.map(|t| tipo_de(&t)))
}

/// Recusa regra que daria a alguma cópia debaixo dela uma segunda Marcação de outro tipo.
fn conflito_regra(
    tx: &Connection,
    volume: i64,
    pasta: Option<i64>,
    tipo: TipoMarcacao,
) -> Resultado<()> {
    let outro = match tipo {
        TipoMarcacao::Backup => TipoMarcacao::Delecao,
        TipoMarcacao::Delecao => TipoMarcacao::Backup,
    };
    let o = tipo_str(outro);
    let erro = |onde: String| {
        ErroDuplicados::Conflito(format!(
            "a regra de {} conflita com {} em {onde}: uma cópia só pode ter uma Marcação. Remova-a antes com `dag desmarcar`.",
            nome_tipo(tipo),
            nome_tipo(outro),
        ))
    };
    // Marcações manuais de outro tipo debaixo do alvo.
    let manual = match pasta {
        None => tx
            .query_row(
                "SELECT f.path_display FROM marcacoes m JOIN files f ON f.id = m.file_id
                 WHERE m.tipo = ?1 AND f.volume_id = ?2 LIMIT 1",
                params![o, volume],
                |l| l.get::<_, String>(0),
            )
            .optional()?,
        Some(p) => tx
            .query_row(
                &format!(
                    "SELECT f.path_display FROM marcacoes m JOIN files f ON f.id = m.file_id
                     JOIN folders pf ON pf.id = ?3
                     WHERE m.tipo = ?1 AND f.volume_id = ?2 AND {} LIMIT 1",
                    sob_sql("f.path_raw", "pf.path_raw")
                ),
                params![o, volume, p],
                |l| l.get::<_, String>(0),
            )
            .optional()?,
    };
    if let Some(onde) = manual {
        return Err(erro(onde));
    }
    // Regras de pasta de outro tipo dentro (ou em volta) do alvo.
    let regra = match pasta {
        None => tx
            .query_row(
                "SELECT pf.path_display FROM marcacoes m JOIN folders pf ON pf.id = m.folder_id
                 WHERE m.escopo = 'regra_pasta' AND m.tipo = ?1 AND pf.volume_id = ?2 LIMIT 1",
                params![o, volume],
                |l| l.get::<_, String>(0),
            )
            .optional()?,
        Some(p) => tx
            .query_row(
                &format!(
                    "SELECT pf.path_display FROM marcacoes m
                     JOIN folders pf ON pf.id = m.folder_id JOIN folders alvo ON alvo.id = ?3
                     WHERE m.escopo = 'regra_pasta' AND m.tipo = ?1 AND pf.volume_id = ?2
                       AND ({} OR {}) LIMIT 1",
                    sob_sql("pf.path_raw", "alvo.path_raw"),
                    sob_sql("alvo.path_raw", "pf.path_raw")
                ),
                params![o, volume, p],
                |l| l.get::<_, String>(0),
            )
            .optional()?,
    };
    if let Some(onde) = regra {
        return Err(erro(onde));
    }
    if pasta.is_some() {
        let vol: Option<String> = tx
            .query_row(
                "SELECT COALESCE(v.rotulo, v.platform_id) FROM marcacoes m JOIN volumes v ON v.id = m.volume_id
                 WHERE m.escopo = 'regra_volume' AND m.volume_id = ?2 AND m.tipo = ?1",
                params![o, volume],
                |l| l.get(0),
            )
            .optional()?;
        if let Some(v) = vol {
            return Err(erro(format!("Volume {v}")));
        }
    }
    Ok(())
}

/// Preenche o alerta de Única Cópia para uma regra de Deleção: quantos arquivos debaixo dela
/// não têm nenhuma outra cópia no Catálogo (e uma amostra).
fn unicas_sob(
    tx: &Connection,
    volume: i64,
    pasta: Option<i64>,
    r: &mut ResultadoMarcacao,
) -> Resultado<()> {
    let sob = if pasta.is_some() {
        format!("AND {}", sob_sql("f.path_raw", "alvo.path_raw"))
    } else {
        String::new()
    };
    let juncao = if pasta.is_some() {
        "JOIN folders alvo ON alvo.id = ?2"
    } else {
        ""
    };
    let onde = format!(
        "FROM files f JOIN hashes h ON h.file_id = f.id {juncao}
         WHERE f.volume_id = ?1 AND f.state = 'ativo' AND f.kind = 'arquivo' AND f.logical_bytes > 0
           AND {HASH_VALIDO} {sob}
           AND (f.hardlink_group IS NULL OR f.id = {rep})
           AND NOT EXISTS (
               SELECT 1 FROM hashes h2 JOIN files f2 ON f2.id = h2.file_id
               WHERE h2.hash = h.hash AND h2.size_bytes = h.size_bytes
                 AND f2.state = 'ativo' AND f2.kind = 'arquivo' AND f2.na_lixeira = 0
                 AND h2.size_bytes = f2.logical_bytes AND h2.mtime_ns IS f2.mtime_ns
                 AND h2.ctime_ns IS f2.ctime_ns AND h2.native_file_id IS f2.native_file_id
                 AND NOT (f2.volume_id = f.volume_id
                          AND (f2.id = f.id OR (f.hardlink_group IS NOT NULL
                                                AND f2.hardlink_group = f.hardlink_group))))",
        rep = representante_sql("f"),
    );
    let ps: Vec<i64> = match pasta {
        Some(p) => vec![volume, p],
        None => vec![volume],
    };
    let ps: Vec<&dyn rusqlite::ToSql> = ps.iter().map(|x| x as &dyn rusqlite::ToSql).collect();
    let total: i64 = tx.query_row(&format!("SELECT COUNT(*) {onde}"), ps.as_slice(), |l| {
        l.get(0)
    })?;
    r.unicas_copias_total = u64::try_from(total).unwrap_or(0);
    let mut st = tx.prepare(&format!(
        "SELECT f.id, f.path_display {onde} ORDER BY f.id LIMIT {AMOSTRA_UNICAS}"
    ))?;
    let amostra = st
        .query_map(ps.as_slice(), |l| {
            Ok(AlertaUnicaCopia {
                arquivo: ArquivoId(l.get(0)?),
                caminho: l.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    r.unicas_copias = amostra;
    Ok(())
}

/// Remove a Marcação do alvo e devolve as removidas.
///
/// * `Caminho`/`Arquivo`: a Marcação manual do arquivo. Se a cópia é marcada por uma regra, o
///   erro diz qual regra desmarcar.
/// * `Grupo`: todas as Marcações criadas a partir do grupo.
/// * `Volume`/`Pasta`: a regra.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`] se não há o que remover.
pub fn desmarcar(
    cat: &mut Catalogo,
    alvo: &Alvo,
    autor: Autor,
    agora: i64,
) -> Resultado<Vec<MarcacaoInfo>> {
    cat.armazem_mut().transacao_imediata(|tx| {
        let ids: Vec<i64> = match alvo {
            Alvo::Caminho(_) | Alvo::Arquivo(_) => {
                let f = resolver_arquivo(tx, alvo)?;
                let proprio: Option<i64> = tx
                    .query_row("SELECT id FROM marcacoes WHERE file_id = ?1", [f], |l| l.get(0))
                    .optional()?;
                match proprio {
                    Some(id) => vec![id],
                    None => {
                        return Err(match marca_efetiva(tx, f)? {
                            Some(MarcaEfetiva { id, escopo, tipo, .. }) => {
                                ErroDuplicados::Conflito(format!(
                                    "a cópia é marcada pela {} ({}, marcação {id}), não individualmente; desmarque a regra (`dag desmarcar --volume`/`--pasta`)",
                                    nome_escopo(escopo),
                                    nome_tipo(tipo)
                                ))
                            }
                            None => ErroDuplicados::NaoEncontrado(
                                "o arquivo não tem Marcação".into(),
                            ),
                        });
                    }
                }
            }
            Alvo::Grupo(g) => {
                let mut st = tx.prepare(
                    "SELECT id FROM marcacoes WHERE escopo = 'manual_grupo' AND grupo_id = ?1 ORDER BY id",
                )?;
                let v = st
                    .query_map([g.0], |l| l.get(0))?
                    .collect::<Result<Vec<i64>, _>>()?;
                if v.is_empty() {
                    return Err(ErroDuplicados::NaoEncontrado(format!(
                        "o grupo {g} não tem Marcações de grupo"
                    )));
                }
                v
            }
            Alvo::Volume(ref_) => {
                let v = resolver_volume(tx, ref_)?;
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM marcacoes WHERE escopo = 'regra_volume' AND volume_id = ?1",
                        [v],
                        |l| l.get(0),
                    )
                    .optional()?;
                vec![id.ok_or_else(|| {
                    ErroDuplicados::NaoEncontrado("o Volume não tem regra de Marcação".into())
                })?]
            }
            Alvo::Pasta(p) => {
                let (pasta, _) = resolver_pasta(tx, p)?;
                let id: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM marcacoes WHERE escopo = 'regra_pasta' AND folder_id = ?1",
                        [pasta],
                        |l| l.get(0),
                    )
                    .optional()?;
                vec![id.ok_or_else(|| {
                    ErroDuplicados::NaoEncontrado("a pasta não tem regra de Marcação".into())
                })?]
            }
        };
        let mut removidas = Vec::new();
        for id in ids {
            let Some(info) = ler(tx, id)? else { continue };
            historico(
                tx,
                "removida",
                &info,
                Some(info.estado),
                None,
                None,
                autor,
                agora,
                None,
            )?;
            tx.execute("DELETE FROM marcacoes WHERE id = ?1", [id])?;
            removidas.push(info);
        }
        Ok(removidas)
    })
}

/// Converte Marcações de grupo (`manual_grupo`) que casam com `cond` (SQL sobre o alias `m`)
/// em Marcações de arquivo, para não serem apagadas junto com o grupo. Registra `alterada`.
pub(crate) fn converter_marcas_de_grupo(
    tx: &Connection,
    cond: &str,
    autor: Autor,
    agora: i64,
) -> Resultado<u64> {
    let infos: Vec<MarcacaoInfo> = {
        let mut st = tx.prepare(&format!(
            "{SELECT_MARCACAO} WHERE m.escopo = 'manual_grupo' AND ({cond})"
        ))?;
        st.query_map([], info_da_linha)?.collect::<Result<_, _>>()?
    };
    for i in &infos {
        tx.execute(
            "UPDATE marcacoes SET escopo = 'manual_arquivo', grupo_id = NULL, atualizada_em = ?2
             WHERE id = ?1",
            params![i.id.0, agora],
        )?;
        let detalhe = format!(
            "a cópia deixou o Grupo de Duplicados {}; a Marcação segue como Marcação do arquivo",
            i.grupo.map_or(0, |g| g.0)
        );
        historico(
            tx,
            "alterada",
            i,
            Some(i.estado),
            Some(i.estado),
            None,
            autor,
            agora,
            Some(&detalhe),
        )?;
    }
    Ok(infos.len() as u64)
}

/// Reavalia as Marcações manuais ativas contra o Hash Completo atual (A-30): se o hash do
/// arquivo mudou desde a marcação, ela vira Marcação Pendente e sai de todas as contas até ser
/// confirmada. Arquivo sem hash válido (ainda não relido) não é julgado. Devolve quantas
/// viraram pendentes.
///
/// # Errors
/// Falha do SQLite.
pub fn reavaliar_marcacoes(cat: &mut Catalogo, autor: Autor, agora: i64) -> Resultado<u64> {
    cat.armazem_mut().transacao_imediata(|tx| {
        let candidatos: Vec<(i64, Vec<u8>)> = {
            let mut st = tx.prepare(&format!(
                "SELECT m.id, h.hash FROM marcacoes m
                 JOIN files f ON f.id = m.file_id JOIN hashes h ON h.file_id = f.id
                 WHERE m.estado = 'ativa' AND m.hash_na_marcacao IS NOT NULL AND {HASH_VALIDO}
                 ORDER BY m.id"
            ))?;
            st.query_map([], |l| Ok((l.get(0)?, l.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut n = 0;
        for (id, atual) in candidatos {
            let (Some(info), Some(atual)) = (ler(tx, id)?, hash_de(&atual)) else {
                continue;
            };
            let (Some(arquivo), Some(guardado)) = (info.arquivo, info.hash_na_marcacao) else {
                continue;
            };
            let mut m = Marcacao {
                arquivo,
                tipo: info.tipo,
                estado: info.estado,
                origem: OrigemMarcacao::Manual,
                hash_na_marcacao: guardado,
            };
            m.reavaliar(&atual);
            if m.conta() {
                continue;
            }
            tx.execute(
                "UPDATE marcacoes SET estado = 'pendente', atualizada_em = ?2 WHERE id = ?1",
                params![id, agora],
            )?;
            let detalhe = format!("o Hash Completo mudou: era {guardado}, agora {atual}");
            historico(
                tx,
                "pendente",
                &info,
                Some(EstadoMarcacao::Ativa),
                Some(EstadoMarcacao::Pendente),
                Some(&atual),
                autor,
                agora,
                Some(&detalhe),
            )?;
            n += 1;
        }
        Ok(n)
    })
}

/// Confirma de novo uma Marcação Pendente: adota o Hash Completo atual do arquivo e volta a
/// `ativa`.
///
/// # Errors
/// [`ErroDuplicados::NaoEncontrado`], [`ErroDuplicados::Invalido`] se não está pendente,
/// [`ErroDuplicados::SemHash`] se o arquivo não tem hash válido.
pub fn confirmar(
    cat: &mut Catalogo,
    id: MarcacaoId,
    autor: Autor,
    agora: i64,
) -> Resultado<ResultadoMarcacao> {
    cat.armazem_mut().transacao_imediata(|tx| {
        let info = ler(tx, id.0)?
            .ok_or_else(|| ErroDuplicados::NaoEncontrado(format!("marcação {id} não existe")))?;
        let Some(arquivo) = info.arquivo.filter(|_| !info.escopo.eh_regra()) else {
            return Err(ErroDuplicados::Invalido(format!(
                "a marcação {id} é uma regra e nunca fica pendente"
            )));
        };
        if info.estado != EstadoMarcacao::Pendente {
            return Err(ErroDuplicados::Invalido(format!(
                "a marcação {id} não está pendente"
            )));
        }
        let atual = hash_valido(tx, arquivo.0)?.ok_or_else(|| {
            ErroDuplicados::SemHash(format!(
                "o arquivo {} não tem Hash Completo válido; refaça o scan antes de confirmar",
                info.alvo
            ))
        })?;
        let mut m = Marcacao {
            arquivo,
            tipo: info.tipo,
            estado: info.estado,
            origem: OrigemMarcacao::Manual,
            hash_na_marcacao: info.hash_na_marcacao.unwrap_or(atual),
        };
        m.confirmar(atual);
        tx.execute(
            "UPDATE marcacoes SET estado = 'ativa', hash_na_marcacao = ?2, atualizada_em = ?3
             WHERE id = ?1",
            params![id.0, atual.0.to_vec(), agora],
        )?;
        let novo =
            ler(tx, id.0)?.ok_or_else(|| ErroDuplicados::NaoEncontrado("marcação sumiu".into()))?;
        historico(
            tx,
            "confirmada",
            &novo,
            Some(EstadoMarcacao::Pendente),
            Some(EstadoMarcacao::Ativa),
            Some(&atual),
            autor,
            agora,
            None,
        )?;
        let mut r = ResultadoMarcacao::default();
        if novo.tipo == TipoMarcacao::Delecao && eh_unica_copia(tx, arquivo.0)? {
            r.unicas_copias_total = 1;
            r.unicas_copias.push(AlertaUnicaCopia {
                arquivo,
                caminho: novo.alvo.clone(),
            });
        }
        r.marcacoes.push(novo);
        Ok(r)
    })
}

/// Marcações existentes, por id crescente (keyset: `depois` = último id da página anterior).
///
/// # Errors
/// Falha do SQLite.
pub fn listar_marcacoes(
    conn: &Connection,
    depois: Option<i64>,
    limite: u32,
) -> Resultado<(Vec<MarcacaoInfo>, Option<i64>)> {
    let limite = i64::from(limite.max(1));
    let mut st = conn.prepare(&format!(
        "{SELECT_MARCACAO} WHERE m.id > ?1 ORDER BY m.id LIMIT ?2"
    ))?;
    let mut v = st
        .query_map(params![depois.unwrap_or(0), limite + 1], info_da_linha)?
        .collect::<Result<Vec<_>, _>>()?;
    let proximo = if v.len() as i64 > limite {
        v.truncate(limite as usize);
        v.last().map(|m| m.id.0)
    } else {
        None
    };
    Ok((v, proximo))
}

/// Linha do histórico de Marcações.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricoMarcacao {
    pub id: i64,
    pub marcacao_id: Option<i64>,
    pub evento: String,
    pub tipo: TipoMarcacao,
    pub escopo: EscopoMarcacao,
    pub estado_anterior: Option<EstadoMarcacao>,
    pub estado_novo: Option<EstadoMarcacao>,
    pub arquivo: Option<i64>,
    pub caminho: Option<String>,
    pub hash: Option<HashCompleto>,
    pub autor: Autor,
    pub em: i64,
    pub detalhe: Option<String>,
}

/// Histórico (de uma marcação, ou todo) em ordem cronológica.
///
/// # Errors
/// Falha do SQLite.
pub fn historico_da_marcacao(
    conn: &Connection,
    marcacao: Option<MarcacaoId>,
) -> Resultado<Vec<HistoricoMarcacao>> {
    let mut st = conn.prepare(
        "SELECT id, marcacao_id, evento, tipo, escopo, estado_anterior, estado_novo, file_id,
                caminho_display, hash, autor, em, detalhe
         FROM marcacoes_historico WHERE (?1 IS NULL OR marcacao_id = ?1) ORDER BY id",
    )?;
    let v = st
        .query_map([marcacao.map(|m| m.0)], |l| {
            let tipo: String = l.get(3)?;
            let escopo: String = l.get(4)?;
            let ant: Option<String> = l.get(5)?;
            let novo: Option<String> = l.get(6)?;
            let hash: Option<Vec<u8>> = l.get(9)?;
            let autor: String = l.get(10)?;
            Ok(HistoricoMarcacao {
                id: l.get(0)?,
                marcacao_id: l.get(1)?,
                evento: l.get(2)?,
                tipo: tipo_de(&tipo),
                escopo: EscopoMarcacao::de_str(&escopo).unwrap_or(EscopoMarcacao::ManualArquivo),
                estado_anterior: ant.as_deref().map(estado_de),
                estado_novo: novo.as_deref().map(estado_de),
                arquivo: l.get(7)?,
                caminho: l.get(8)?,
                hash: hash.as_deref().and_then(hash_de),
                autor: autor_de(&autor),
                em: l.get(11)?,
                detalhe: l.get(12)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}
