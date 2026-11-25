//! Espaço recuperável (A-31) por grupo, pasta, Volume e computador.

use std::collections::{BTreeMap, HashMap};

use dag_core::regras::{CopiaMarcada, DivisaoEspaco, espaco_recuperavel};
use dag_core::{
    ArquivoId, DadosRelatorio, DispositivoId, EspacoItem, LIMITACOES_ESPACO, NivelEspaco, VolumeId,
};
use rusqlite::Connection;

use super::agregado;
use super::grupos::ler_cursor;
use super::{GrupoCarregado, Resultado, percorrer_grupos, representante_sql, sob_sql};

/// Divisão do espaço do grupo entre cópias marcadas para Deleção e redundantes estimadas.
pub(crate) fn dividir(g: &GrupoCarregado) -> DivisaoEspaco {
    let copias: Vec<CopiaMarcada> = g
        .copias
        .iter()
        .map(|c| CopiaMarcada {
            copia: dag_core::regras::Copia {
                arquivo: ArquivoId(c.arquivo),
                volume: VolumeId(c.volume),
                dispositivo: DispositivoId(c.dispositivo.unwrap_or(0)),
            },
            marcacao: c.marca.map(|m| (m.tipo, m.estado)),
        })
        .collect();
    espaco_recuperavel(&copias, g.principal.map(ArquivoId))
}

/// Acumulador de bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Acumulado {
    pub marcado: u64,
    pub estimado: u64,
    pub copias: u64,
}

impl Acumulado {
    fn somar(&mut self, bytes: u64, marcada: bool) {
        if marcada {
            self.marcado += bytes;
        } else {
            self.estimado += bytes;
        }
        self.copias += 1;
    }

    #[must_use]
    pub fn total(&self) -> u64 {
        self.marcado + self.estimado
    }
}

/// Espaço recuperável de todo o Catálogo, em todas as granularidades.
#[derive(Debug, Clone, Default)]
pub struct EspacoCatalogo {
    pub computador: Acumulado,
    pub por_volume: BTreeMap<i64, Acumulado>,
    /// Pasta que contém diretamente a cópia.
    pub por_pasta_direta: BTreeMap<i64, Acumulado>,
    /// `(grupo_id, nome, acumulado)`.
    pub por_grupo: Vec<(i64, String, Acumulado)>,
}

/// Espaço recuperável de todo o Catálogo, calculado em SQL (memória proporcional ao número de
/// Volumes, pastas e grupos **do resultado**, não ao Catálogo). Inclui a lista por grupo: para
/// relatórios paginados use [`relatorio_espaco`], que só carrega o necessário.
///
/// # Errors
/// Falha do SQLite.
pub fn espaco_de_grupos(conn: &Connection) -> Resultado<EspacoCatalogo> {
    espaco(conn, true, true)
}

const PAGINA_GRUPOS: usize = 5000;

fn espaco(conn: &Connection, com_pastas: bool, com_grupos: bool) -> Resultado<EspacoCatalogo> {
    let mut r = EspacoCatalogo::default();
    for (v, marcado, estimado, copias) in agregado::por_destino(conn, false)? {
        let a = Acumulado {
            marcado,
            estimado,
            copias,
        };
        r.computador.marcado += marcado;
        r.computador.estimado += estimado;
        r.computador.copias += copias;
        r.por_volume.insert(v, a);
    }
    if com_pastas {
        for (p, marcado, estimado, copias) in agregado::por_destino(conn, true)? {
            r.por_pasta_direta.insert(
                p,
                Acumulado {
                    marcado,
                    estimado,
                    copias,
                },
            );
        }
    }
    if com_grupos {
        let mut depois = None;
        loop {
            let pag = agregado::grupos_com_espaco(conn, depois, PAGINA_GRUPOS)?;
            let fim = pag.len() < PAGINA_GRUPOS;
            depois = pag.last().map(|(k, l)| (*k, l.0));
            for (_, (g, nome, marcado, estimado, copias)) in pag {
                r.por_grupo.push((
                    g,
                    nome,
                    Acumulado {
                        marcado,
                        estimado,
                        copias,
                    },
                ));
            }
            if fim {
                break;
            }
        }
        r.por_grupo.sort_by_key(|x| x.0);
    }
    marcadas_fora_de_grupo(conn, &mut r)?;
    Ok(r)
}

/// Versão de referência de [`espaco_de_grupos`]: percorre todos os grupos na memória. Só testes.
///
/// # Errors
/// Falha do SQLite.
#[doc(hidden)]
pub fn espaco_de_grupos_referencia(conn: &Connection) -> Resultado<EspacoCatalogo> {
    let mut r = EspacoCatalogo::default();
    percorrer_grupos(conn, |g| {
        let d = dividir(&g);
        let mut acc = Acumulado::default();
        for (ids, marcada) in [(&d.marcadas, true), (&d.estimadas, false)] {
            for id in ids {
                let Some(c) = g.copias.iter().find(|c| c.arquivo == id.0) else {
                    continue;
                };
                acc.somar(g.tamanho, marcada);
                r.computador.somar(g.tamanho, marcada);
                r.por_volume
                    .entry(c.volume)
                    .or_default()
                    .somar(g.tamanho, marcada);
                r.por_pasta_direta
                    .entry(c.pasta)
                    .or_default()
                    .somar(g.tamanho, marcada);
            }
        }
        if acc.total() > 0 {
            let nome = g
                .copias
                .iter()
                .find(|c| Some(c.arquivo) == g.principal)
                .or(g.copias.first())
                .map_or_else(String::new, |c| c.caminho.clone());
            r.por_grupo.push((g.id, nome, acc));
        }
        Ok(true)
    })?;
    marcadas_fora_de_grupo(conn, &mut r)?;
    Ok(r)
}

/// Arquivos com Marcação para Deleção **ativa** (manual ou de regra) que não são cópia de
/// nenhum grupo: também liberam espaço (e são Única Cópia). Entram no total do computador, dos
/// Volumes e das pastas; não pertencem a nenhum grupo.
fn marcadas_fora_de_grupo(conn: &Connection, r: &mut EspacoCatalogo) -> Resultado<()> {
    let comum = format!(
        "f.state = 'ativo' AND f.kind = 'arquivo' AND f.logical_bytes > 0
         AND f.na_lixeira = 0
         AND NOT EXISTS (SELECT 1 FROM grupo_membros gm WHERE gm.file_id = f.id)
         AND (f.hardlink_group IS NULL OR f.id = {})",
        representante_sql("f")
    );
    let sql = format!(
        "SELECT f.id, f.volume_id, f.folder_id, f.logical_bytes FROM marcacoes m
           JOIN files f ON f.id = m.file_id
          WHERE m.tipo = 'delecao' AND m.estado = 'ativa' AND {comum}
         UNION
         SELECT f.id, f.volume_id, f.folder_id, f.logical_bytes FROM marcacoes m
           JOIN folders pf ON pf.id = m.folder_id
           JOIN files f ON f.volume_id = pf.volume_id AND {sob}
          WHERE m.escopo = 'regra_pasta' AND m.tipo = 'delecao' AND {comum}
            AND NOT EXISTS (SELECT 1 FROM marcacoes o WHERE o.file_id = f.id)
         UNION
         SELECT f.id, f.volume_id, f.folder_id, f.logical_bytes FROM marcacoes m
           JOIN files f ON f.volume_id = m.volume_id
          WHERE m.escopo = 'regra_volume' AND m.tipo = 'delecao' AND {comum}
            AND NOT EXISTS (SELECT 1 FROM marcacoes o WHERE o.file_id = f.id)",
        sob = sob_sql("f.path_raw", "pf.path_raw")
    );
    let mut st = conn.prepare(&sql)?;
    let linhas = st.query_map([], |l| {
        Ok((
            l.get::<_, i64>(1)?,
            l.get::<_, i64>(2)?,
            l.get::<_, i64>(3)?,
        ))
    })?;
    for l in linhas {
        let (volume, pasta, bytes) = l?;
        let bytes = u64::try_from(bytes).unwrap_or(0);
        r.computador.somar(bytes, true);
        r.por_volume.entry(volume).or_default().somar(bytes, true);
        r.por_pasta_direta
            .entry(pasta)
            .or_default()
            .somar(bytes, true);
    }
    Ok(())
}

fn item(nivel: NivelEspaco, id: Option<i64>, nome: String, a: Acumulado) -> EspacoItem {
    EspacoItem {
        nivel,
        id,
        nome,
        conectado: None,
        bytes_marcados_delecao: a.marcado,
        bytes_redundantes_estimados: a.estimado,
        bytes_total: a.total(),
        copias: a.copias,
    }
}

/// Propaga o espaço de cada pasta para todas as ancestrais (soma recursiva). Carrega o mapa
/// pasta → pai numa só varredura (memória proporcional ao número de pastas, não de arquivos).
fn recursivo_por_pasta(
    conn: &Connection,
    diretas: &BTreeMap<i64, Acumulado>,
) -> Resultado<BTreeMap<i64, Acumulado>> {
    let mut pai: HashMap<i64, Option<i64>> = HashMap::new();
    if !diretas.is_empty() {
        let mut st = conn.prepare("SELECT id, parent_folder_id FROM folders")?;
        let linhas = st.query_map([], |l| {
            Ok((l.get::<_, i64>(0)?, l.get::<_, Option<i64>>(1)?))
        })?;
        for l in linhas {
            let (id, p) = l?;
            pai.insert(id, p);
        }
    }
    let mut r: BTreeMap<i64, Acumulado> = BTreeMap::new();
    for (pasta, acc) in diretas {
        let mut atual = Some(*pasta);
        let mut passos = 0;
        while let Some(p) = atual {
            let e = r.entry(p).or_default();
            e.marcado += acc.marcado;
            e.estimado += acc.estimado;
            e.copias += acc.copias;
            atual = pai.get(&p).copied().flatten();
            passos += 1;
            if passos > 4096 {
                break;
            }
        }
    }
    Ok(r)
}

/// Relatório de espaço recuperável no `nivel` pedido. O total do computador vem sempre.
/// Pastas (soma recursiva) e grupos saem do maior para o menor, paginados por cursor.
///
/// # Errors
/// [`ErroDuplicados::Invalido`] para cursor mal formado; falha do SQLite.
pub fn relatorio_espaco(
    conn: &Connection,
    nivel: NivelEspaco,
    limite: u32,
    cursor: Option<&str>,
) -> Resultado<(DadosRelatorio, Option<String>)> {
    let e = espaco(conn, nivel == NivelEspaco::Pasta, false)?;
    let total = item(
        NivelEspaco::Computador,
        None,
        "este computador".into(),
        e.computador,
    );
    let depois = ler_cursor(cursor)?;
    let limite = limite.max(1) as usize;
    let mut linhas: Vec<(u64, i64, EspacoItem)> = Vec::new();
    match nivel {
        NivelEspaco::Computador => {}
        NivelEspaco::Volume => {
            let mut st = conn.prepare(
                "SELECT COALESCE(rotulo, platform_id), mounted FROM volumes WHERE id = ?1",
            )?;
            for (v, a) in &e.por_volume {
                let (nome, on): (String, i64) =
                    st.query_row([v], |l| Ok((l.get(0)?, l.get(1)?)))?;
                let mut it = item(NivelEspaco::Volume, Some(*v), nome, *a);
                it.conectado = Some(on != 0);
                linhas.push((a.total(), *v, it));
            }
        }
        NivelEspaco::Pasta => {
            let rec = recursivo_por_pasta(conn, &e.por_pasta_direta)?;
            let mut st = conn.prepare("SELECT path_display FROM folders WHERE id = ?1")?;
            for (p, a) in &rec {
                let nome: String = st.query_row([p], |l| l.get(0))?;
                linhas.push((a.total(), *p, item(NivelEspaco::Pasta, Some(*p), nome, *a)));
            }
        }
        NivelEspaco::Grupo => {
            // Só a página (e uma a mais, para saber se há próxima): memória limitada.
            for (chave, (g, nome, marcado, estimado, copias)) in
                agregado::grupos_com_espaco(conn, depois, limite + 1)?
            {
                let a = Acumulado {
                    marcado,
                    estimado,
                    copias,
                };
                linhas.push((chave, g, item(NivelEspaco::Grupo, Some(g), nome, a)));
            }
        }
    }
    linhas.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut pagina: Vec<_> = linhas
        .into_iter()
        .filter(|(k, id, _)| match depois {
            None => true,
            Some((ck, ci)) => *k < ck || (*k == ck && *id > ci),
        })
        .take(limite + 1)
        .collect();
    let proximo = if pagina.len() > limite {
        pagina.truncate(limite);
        pagina.last().map(|(k, id, _)| format!("{k}:{id}"))
    } else {
        None
    };
    Ok((
        DadosRelatorio::Espaco {
            total,
            itens: pagina.into_iter().map(|x| x.2).collect(),
            limitacoes: LIMITACOES_ESPACO.iter().map(ToString::to_string).collect(),
        },
        proximo,
    ))
}
