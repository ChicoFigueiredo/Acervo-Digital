//! Erros esperados e aceitos: regras do usuário, exclusões, classificação das linhas de
//! `scan_errors`, desfecho de uma execução e resumo para `dag erros`.
//!
//! A classificação em si é pura e fica em [`dag_core::erros`]; aqui só se lê e grava.

use std::collections::HashMap;

use dag_core::erros::{
    AmostraErro, ClasseErro, ExclusaoUsuario, GrupoErros, GrupoPasta, RegraAceita, ResumoErros,
    classificar, dica_do_codigo, rotulo_exibicao,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Resultado;

/// Pasta do usuário (para expandir `~` nos padrões).
#[must_use]
pub fn pasta_home() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
}

// ------------------------------------------------------------------ regras do usuário

/// Cria (ou atualiza o motivo de) uma regra "Erros aceitos". Devolve o id.
///
/// # Errors
/// Falha do SQLite.
pub fn adicionar_regra(
    conn: &Connection,
    padrao: &str,
    codigo: Option<&str>,
    motivo: &str,
    agora: i64,
) -> Resultado<i64> {
    let codigo = codigo
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or("*");
    conn.execute(
        "INSERT INTO erros_aceitos (padrao, codigo, motivo, criada_em) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(padrao, codigo) DO UPDATE SET motivo = excluded.motivo",
        params![padrao, codigo, motivo, agora],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM erros_aceitos WHERE padrao = ?1 AND codigo = ?2",
        params![padrao, codigo],
        |l| l.get(0),
    )?)
}

/// Regras "Erros aceitos", da mais antiga para a mais nova.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_regras(conn: &Connection) -> Resultado<Vec<RegraAceita>> {
    let mut st = conn
        .prepare("SELECT id, padrao, codigo, motivo, criada_em FROM erros_aceitos ORDER BY id")?;
    Ok(st
        .query_map([], |l| {
            Ok(RegraAceita {
                id: l.get(0)?,
                padrao: l.get(1)?,
                codigo: l.get(2)?,
                motivo: l.get(3)?,
                criada_em: l.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

/// Remove uma regra; `false` se não existia.
///
/// # Errors
/// Falha do SQLite.
pub fn remover_regra(conn: &Connection, id: i64) -> Resultado<bool> {
    Ok(conn.execute("DELETE FROM erros_aceitos WHERE id = ?1", [id])? > 0)
}

// ------------------------------------------------------------------ exclusões (scan_exclusions)

/// Cria uma exclusão global (glob de caminho). Reaproveita `scan_exclusions` (A-19). Repetir o
/// mesmo padrão só reativa e atualiza o motivo.
///
/// # Errors
/// Falha do SQLite.
pub fn adicionar_exclusao(conn: &Connection, padrao: &str, motivo: &str) -> Resultado<i64> {
    let existente: Option<i64> = conn
        .query_row(
            "SELECT id FROM scan_exclusions WHERE scope = 'global' AND matcher = ?1",
            [padrao],
            |l| l.get(0),
        )
        .optional()?;
    if let Some(id) = existente {
        conn.execute(
            "UPDATE scan_exclusions SET reason = ?2, enabled = 1 WHERE id = ?1",
            params![id, motivo],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO scan_exclusions (scope, matcher, reason, enabled) VALUES ('global', ?1, ?2, 1)",
        params![padrao, motivo],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Exclusões ativas.
///
/// # Errors
/// Falha do SQLite.
pub fn listar_exclusoes(conn: &Connection) -> Resultado<Vec<ExclusaoUsuario>> {
    let mut st = conn.prepare(
        "SELECT id, matcher, COALESCE(reason, '') FROM scan_exclusions
         WHERE enabled = 1 ORDER BY id",
    )?;
    Ok(st
        .query_map([], |l| {
            Ok(ExclusaoUsuario {
                id: l.get(0)?,
                padrao: l.get(1)?,
                motivo: l.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

/// Remove uma exclusão; `false` se não existia.
///
/// # Errors
/// Falha do SQLite.
pub fn remover_exclusao(conn: &Connection, id: i64) -> Resultado<bool> {
    Ok(conn.execute("DELETE FROM scan_exclusions WHERE id = ?1", [id])? > 0)
}

// ------------------------------------------------------------------ classificação gravada

/// Classifica as linhas da execução que ainda não têm `classe` (as novas), pelas regras de agora.
/// Devolve quantas linhas foram classificadas.
///
/// # Errors
/// Falha do SQLite.
pub fn classificar_execucao(conn: &Connection, run_id: i64, home: Option<&str>) -> Resultado<u64> {
    let regras = listar_regras(conn)?;
    let mut decisoes: Vec<(i64, &'static str, Option<String>)> = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT id, error_code, path_raw FROM scan_errors
             WHERE run_id = ?1 AND classe IS NULL",
        )?;
        let mut linhas = st.query([run_id])?;
        while let Some(l) = linhas.next()? {
            let id: i64 = l.get(0)?;
            let codigo: String = l.get(1)?;
            let caminho: Option<Vec<u8>> = l.get(2)?;
            let c = classificar(&codigo, caminho.as_deref(), &regras, home);
            decisoes.push((id, c.classe.como_str(), c.regra));
        }
    }
    let mut up =
        conn.prepare_cached("UPDATE scan_errors SET classe = ?2, regra = ?3 WHERE id = ?1")?;
    for (id, classe, regra) in &decisoes {
        up.execute(params![id, classe, regra])?;
    }
    Ok(decisoes.len() as u64)
}

/// Marca uma linha como esperada por decisão do reteste do fim da etapa de hash.
///
/// # Errors
/// Falha do SQLite.
pub fn marcar_esperado(conn: &Connection, id: i64, regra: &str, motivo: &str) -> Resultado<()> {
    conn.execute(
        "UPDATE scan_errors SET classe = 'esperado', regra = ?2, reason = ?3 WHERE id = ?1",
        params![id, format!("embutida:{regra}"), motivo],
    )?;
    Ok(())
}

/// Remove uma linha de problema (o reteste conseguiu ler o arquivo: nenhum erro a registrar).
///
/// # Errors
/// Falha do SQLite.
pub fn apagar_problema(conn: &Connection, id: i64) -> Resultado<()> {
    conn.execute("DELETE FROM scan_errors WHERE id = ?1", [id])?;
    Ok(())
}

/// Contagem que decide o estado final de uma execução.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContagemDesfecho {
    /// Erros reais (severidade erro ou impeditivo, sem regra que os aceite).
    pub reais: u64,
    /// Avisos, esperados e aceitos (fora os informativos).
    pub avisos: u64,
}

/// Conta, nas linhas já classificadas (use [`classificar_execucao`] antes), o que decide entre
/// `concluida`, `concluida_com_avisos` e `concluida_com_erros`.
///
/// # Errors
/// Falha do SQLite.
pub fn contar_desfecho(conn: &Connection, run_id: i64) -> Resultado<ContagemDesfecho> {
    let (reais, avisos): (i64, i64) = conn.query_row(
        "SELECT
            COALESCE(SUM(severity IN ('erro','impeditivo') AND COALESCE(classe,'erro') = 'erro'), 0),
            COALESCE(SUM(NOT (severity IN ('erro','impeditivo') AND COALESCE(classe,'erro') = 'erro')), 0)
         FROM scan_errors WHERE run_id = ?1 AND severity <> 'info'",
        [run_id],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    Ok(ContagemDesfecho {
        reais: reais as u64,
        avisos: avisos as u64,
    })
}

// ------------------------------------------------------------------ resumo

/// Última execução terminada (a mais recente com `ended_at`).
///
/// # Errors
/// Falha do SQLite.
pub fn ultima_execucao_terminada(conn: &Connection) -> Resultado<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM scan_runs WHERE ended_at IS NOT NULL ORDER BY id DESC LIMIT 1",
            [],
            |l| l.get(0),
        )
        .optional()?)
}

/// Filtros e limites de [`resumir`].
#[derive(Debug, Clone, Default)]
pub struct OpcoesResumo<'a> {
    pub codigo: Option<&'a str>,
    /// Prefixo de pasta.
    pub pasta: Option<&'a str>,
    /// Amostras por grupo (padrão 3).
    pub amostras: Option<usize>,
    /// Pastas no ranking (padrão 15).
    pub pastas: Option<usize>,
}

/// Profundidade (componentes de caminho) usada para agrupar por pasta.
const PROFUNDIDADE_PASTA: usize = 4;

/// Total de problemas de uma pasta e a contagem por código.
type ContagemPasta = (u64, HashMap<String, u64>);

fn pasta_do_grupo(caminho: &str) -> String {
    let componentes: Vec<&str> = caminho.split('/').filter(|c| !c.is_empty()).collect();
    if componentes.len() <= 1 {
        return "/".to_owned();
    }
    // Sem o nome do próprio arquivo; no máximo `PROFUNDIDADE_PASTA` níveis.
    let n = (componentes.len() - 1).min(PROFUNDIDADE_PASTA);
    format!("/{}", componentes[..n].join("/"))
}

/// Resumo dos problemas da execução, classificados agora (regras embutidas e do usuário de hoje;
/// o que o reteste marcou como esperado continua esperado).
///
/// # Errors
/// Falha do SQLite.
pub fn resumir(
    conn: &Connection,
    run_id: i64,
    opcoes: &OpcoesResumo<'_>,
    home: Option<&str>,
) -> Resultado<ResumoErros> {
    let regras = listar_regras(conn)?;
    let estado: String = conn
        .query_row(
            "SELECT status FROM scan_runs WHERE id = ?1",
            [run_id],
            |l| l.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let por_grupo_max = opcoes.amostras.unwrap_or(3);
    let pastas_max = opcoes.pastas.unwrap_or(15);

    let mut resumo = ResumoErros {
        execucao: run_id,
        agrupar: dag_core::erros::AgruparErros::Codigo,
        estado,
        total: 0,
        reais: 0,
        avisos: 0,
        esperados: 0,
        aceitos: 0,
        informativos: 0,
        por_codigo: Vec::new(),
        por_pasta: Vec::new(),
        amostras: Vec::new(),
    };
    type Chave = (String, String, String, &'static str);
    let mut grupos: HashMap<Chave, u64> = HashMap::new();
    let mut amostras: HashMap<Chave, Vec<AmostraErro>> = HashMap::new();
    let mut pastas: HashMap<String, ContagemPasta> = HashMap::new();

    let mut st = conn.prepare(
        "SELECT phase, error_code, severity, classe, path_raw, path_display, reason
         FROM scan_errors WHERE run_id = ?1",
    )?;
    let mut linhas = st.query([run_id])?;
    while let Some(l) = linhas.next()? {
        let fase: String = l.get(0)?;
        let codigo: String = l.get(1)?;
        let severidade: String = l.get(2)?;
        let guardada: Option<String> = l.get(3)?;
        let bruto: Option<Vec<u8>> = l.get(4)?;
        let exibicao: Option<String> = l.get(5)?;
        let motivo: Option<String> = l.get(6)?;
        if opcoes.codigo.is_some_and(|c| c != codigo) {
            continue;
        }
        if let Some(p) = opcoes.pasta {
            let casa = exibicao.as_deref().is_some_and(|c| {
                let p = p.trim_end_matches('/');
                c == p || c.starts_with(&format!("{p}/"))
            });
            if !casa {
                continue;
            }
        }
        let classe = if guardada.as_deref() == Some("esperado") {
            ClasseErro::Esperado
        } else {
            classificar(&codigo, bruto.as_deref(), &regras, home).classe
        };
        let rotulo = rotulo_exibicao(classe, &severidade);
        resumo.total += 1;
        match rotulo {
            "esperado" => resumo.esperados += 1,
            "aceito" => resumo.aceitos += 1,
            "info" => resumo.informativos += 1,
            "aviso" => resumo.avisos += 1,
            _ => resumo.reais += 1,
        }
        let chave: Chave = (fase.clone(), codigo.clone(), severidade.clone(), rotulo);
        *grupos.entry(chave.clone()).or_default() += 1;
        let a = amostras.entry(chave).or_default();
        if a.len() < por_grupo_max {
            a.push(AmostraErro {
                fase,
                codigo: codigo.clone(),
                rotulo: rotulo.to_owned(),
                caminho: exibicao.clone(),
                motivo,
            });
        }
        if rotulo != "info" {
            let pasta = exibicao
                .as_deref()
                .map_or_else(|| "(sem caminho)".to_owned(), pasta_do_grupo);
            let e = pastas.entry(pasta).or_default();
            e.0 += 1;
            *e.1.entry(codigo).or_default() += 1;
        }
    }

    let ordem_rotulo = |r: &str| match r {
        "erro" => 0,
        "aviso" => 1,
        "aceito" => 2,
        "esperado" => 3,
        _ => 4,
    };
    let mut lista: Vec<(Chave, u64)> = grupos.into_iter().collect();
    lista.sort_by(|a, b| {
        ordem_rotulo(a.0.3)
            .cmp(&ordem_rotulo(b.0.3))
            .then(b.1.cmp(&a.1))
            .then(a.0.cmp(&b.0))
    });
    for (chave, total) in lista {
        if let Some(a) = amostras.remove(&chave) {
            resumo.amostras.extend(a);
        }
        let dica = (chave.3 == "erro" || chave.3 == "aviso")
            .then(|| dica_do_codigo(&chave.1))
            .flatten()
            .map(str::to_owned);
        resumo.por_codigo.push(GrupoErros {
            fase: chave.0,
            codigo: chave.1,
            severidade: chave.2,
            rotulo: chave.3.to_owned(),
            total,
            dica,
        });
    }
    let mut pl: Vec<(String, ContagemPasta)> = pastas.into_iter().collect();
    pl.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    pl.truncate(pastas_max);
    for (pasta, (total, codigos)) in pl {
        let mut c: Vec<(String, u64)> = codigos.into_iter().collect();
        c.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        c.truncate(4);
        resumo.por_pasta.push(GrupoPasta {
            pasta,
            total,
            codigos: c,
        });
    }
    Ok(resumo)
}
