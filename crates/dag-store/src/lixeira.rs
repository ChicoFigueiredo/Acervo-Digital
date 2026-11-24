//! Lixeira em dois níveis, lado Catálogo (ADR 0005). **Nada aqui toca o disco**: o `rename(2)` é
//! do agente (`dag-agent::lixeira`); este módulo planeja o que pode ir, registra o resultado e
//! reconcilia o que as varreduras viram dentro de `DAG-Lixeira`.
//!
//! * Planejamento: resolve alvos (arquivo, pasta, grupo) em cópias, confere Marcação para
//!   Deleção, Única Cópia, hard links.
//! * Registro: depois do `rename`, a linha de `files` muda de caminho (mantendo id, hash e
//!   Marcações) e ganha `na_lixeira`/origem; o inverso na restauração.
//! * Reconciliação (varredura): o que o usuário põe à mão na Lixeira passa a ter Marcação para
//!   Deleção e ficar Na Lixeira; o que ele tira volta a ser cópia comum, sem marcação; o que
//!   estava Na Lixeira e some vira registro Descartado.
//! * Cópia Na Lixeira nunca conta como outra cópia (filtros `na_lixeira = 0` em grupos, Única
//!   Cópia e relatórios).

use std::collections::BTreeMap;

use dag_core::lixeira::{
    destino_na_lixeira, esta_na_lixeira, origem_a_partir_da_lixeira, raiz_da_lixeira, relativo_a,
};
use dag_core::{
    Alvo, ArquivoId, Autor, EstadoMarcacao, ItemDescartado, ItemLixeira, TipoMarcacao, VolumeId,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Catalogo;
use crate::duplicados::{
    self, ErroDuplicados, Resultado, eh_unica_copia, marca_efetiva, resolver_arquivo,
    resolver_pasta, resolver_volume,
};

fn u(x: i64) -> u64 {
    u64::try_from(x).unwrap_or(0)
}

fn exibir(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Situação da Marcação que vale para uma cópia, para decidir se pode ir para a Lixeira.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SituacaoMarca {
    /// Marcação para Deleção ativa, manual (arquivo ou grupo).
    Delecao,
    /// Marcação para Deleção ativa vinda de regra de pasta ou de Volume.
    DelecaoPorRegra,
    /// Marcação para Deleção pendente (o conteúdo mudou depois de marcada).
    Pendente,
    /// A cópia tem Marcação de Backup.
    Backup,
    Nenhuma,
}

impl SituacaoMarca {
    /// Pode ser movida para a Lixeira.
    #[must_use]
    pub fn pode_mover(self) -> bool {
        matches!(self, Self::Delecao | Self::DelecaoPorRegra)
    }
}

/// Uma cópia candidata, com tudo o que o agente precisa para conferir o disco antes do `rename`.
#[derive(Debug, Clone)]
pub struct Candidato {
    pub id: i64,
    pub volume_id: i64,
    pub volume_nome: String,
    pub path_raw: Vec<u8>,
    pub montagem_raw: Vec<u8>,
    pub dev: Option<u64>,
    pub ino: Option<u64>,
    pub bytes: u64,
    pub mtime_ns: Option<i64>,
    pub ctime_ns: Option<i64>,
    /// Há outros caminhos para o mesmo conteúdo físico: mover um só nome não remove nada.
    pub hard_link: bool,
    pub na_lixeira: bool,
    pub origem_raw: Option<Vec<u8>>,
    pub marca: SituacaoMarca,
    pub unica_copia: bool,
    pub hash: Option<Vec<u8>>,
}

impl Candidato {
    #[must_use]
    pub fn caminho(&self) -> String {
        exibir(&self.path_raw)
    }

    /// Lixeira do Volume deste arquivo (`<ponto de montagem>/DAG-Lixeira`).
    #[must_use]
    pub fn raiz_da_lixeira_absoluta(&self) -> Vec<u8> {
        raiz_da_lixeira(&self.montagem_raw)
    }
}

/// Um alvo resolvido em cópias.
#[derive(Debug, Clone)]
pub struct AlvoResolvido {
    pub descricao: String,
    pub eh_pasta: bool,
    pub arquivos: Vec<Candidato>,
}

fn montagem_do_caminho(
    conn: &Connection,
    volume: i64,
    caminho: &[u8],
) -> Resultado<Option<Vec<u8>>> {
    let mut st = conn.prepare_cached(
        "SELECT path_raw FROM mount_points WHERE volume_id = ?1 ORDER BY length(path_raw) DESC",
    )?;
    let mps: Vec<Vec<u8>> = st
        .query_map([volume], |l| l.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(mps
        .into_iter()
        .find(|m| relativo_a(m, caminho).is_some() || m == caminho))
}

fn nome_do_volume(conn: &Connection, volume: i64) -> String {
    conn.query_row(
        "SELECT COALESCE(NULLIF(v.rotulo, ''),
                (SELECT path_display FROM mount_points m WHERE m.volume_id = v.id
                 ORDER BY length(m.path_raw), m.id LIMIT 1), 'Volume ' || v.id)
         FROM volumes v WHERE v.id = ?1",
        [volume],
        |l| l.get(0),
    )
    .unwrap_or_else(|_| format!("Volume {volume}"))
}

fn situacao(conn: &Connection, id: i64) -> Resultado<SituacaoMarca> {
    Ok(match marca_efetiva(conn, id)? {
        None => SituacaoMarca::Nenhuma,
        Some(m) => match (m.tipo, m.estado) {
            (TipoMarcacao::Backup, _) => SituacaoMarca::Backup,
            (TipoMarcacao::Delecao, EstadoMarcacao::Pendente) => SituacaoMarca::Pendente,
            (TipoMarcacao::Delecao, EstadoMarcacao::Ativa) if m.escopo.eh_regra() => {
                SituacaoMarca::DelecaoPorRegra
            }
            (TipoMarcacao::Delecao, EstadoMarcacao::Ativa) => SituacaoMarca::Delecao,
        },
    })
}

const COLUNAS: &str = "f.id, f.volume_id, f.path_raw, f.native_dev, f.native_file_id,
    f.logical_bytes, f.mtime_ns, f.ctime_ns, f.hardlink_group IS NOT NULL OR COALESCE(f.nlink, 1) > 1,
    f.na_lixeira, f.lixeira_origem_raw, (SELECT h.hash FROM hashes h WHERE h.file_id = f.id)";

fn carregar(conn: &Connection, id: i64) -> Resultado<Option<Candidato>> {
    let linha = conn
        .query_row(
            &format!("SELECT {COLUNAS} FROM files f WHERE f.id = ?1 AND f.kind = 'arquivo'"),
            [id],
            ler_linha,
        )
        .optional()?;
    linha.map(|l| completar(conn, l)).transpose()
}

type Linha = (
    i64,
    i64,
    Vec<u8>,
    Option<i64>,
    Option<i64>,
    i64,
    Option<i64>,
    Option<i64>,
    bool,
    bool,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

fn ler_linha(l: &rusqlite::Row<'_>) -> rusqlite::Result<Linha> {
    Ok((
        l.get(0)?,
        l.get(1)?,
        l.get(2)?,
        l.get(3)?,
        l.get(4)?,
        l.get(5)?,
        l.get(6)?,
        l.get(7)?,
        l.get::<_, i64>(8)? != 0,
        l.get::<_, i64>(9)? != 0,
        l.get(10)?,
        l.get(11)?,
    ))
}

fn completar(conn: &Connection, l: Linha) -> Resultado<Candidato> {
    let montagem = montagem_do_caminho(conn, l.1, &l.2)?.unwrap_or_default();
    Ok(Candidato {
        id: l.0,
        volume_id: l.1,
        volume_nome: nome_do_volume(conn, l.1),
        montagem_raw: montagem,
        dev: l.3.map(u),
        ino: l.4.map(u),
        bytes: u(l.5),
        mtime_ns: l.6,
        ctime_ns: l.7,
        hard_link: l.8,
        na_lixeira: l.9,
        origem_raw: l.10,
        marca: situacao(conn, l.0)?,
        unica_copia: !l.9 && eh_unica_copia(conn, l.0)?,
        hash: l.11,
        path_raw: l.2,
    })
}

/// Faixa `[raiz/, raiz0)` que cobre tudo sob `raiz` em ordem de bytes.
fn faixa(raiz: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let base = if raiz.len() > 1 && raiz.ends_with(b"/") {
        &raiz[..raiz.len() - 1]
    } else {
        raiz
    };
    let mut ini = base.to_vec();
    if ini != b"/" {
        ini.push(b'/');
    }
    let mut fim = base.to_vec();
    if fim == b"/" {
        fim = vec![b'/' + 1];
    } else {
        fim.push(b'/' + 1);
    }
    (ini, fim)
}

fn arquivos_sob(
    conn: &Connection,
    volume: i64,
    pasta_raw: &[u8],
    na_lixeira: bool,
) -> Resultado<Vec<Candidato>> {
    let (ini, fim) = faixa(pasta_raw);
    let mut st = conn.prepare(&format!(
        "SELECT {COLUNAS} FROM files f
         WHERE f.volume_id = ?1 AND f.path_raw >= ?2 AND f.path_raw < ?3
           AND f.kind = 'arquivo' AND f.state = 'ativo' AND f.na_lixeira = ?4
         ORDER BY f.path_raw"
    ))?;
    let linhas: Vec<Linha> = st
        .query_map(params![volume, ini, fim, i64::from(na_lixeira)], ler_linha)?
        .collect::<Result<_, _>>()?;
    linhas.into_iter().map(|l| completar(conn, l)).collect()
}

fn pasta_raw(conn: &Connection, pasta_id: i64) -> Resultado<Vec<u8>> {
    Ok(conn.query_row(
        "SELECT path_raw FROM folders WHERE id = ?1",
        [pasta_id],
        |l| l.get(0),
    )?)
}

/// Resolve um alvo para mover (`na_lixeira = false`: cópias ainda no lugar) ou para restaurar
/// (`true`: itens Na Lixeira).
///
/// * `Arquivo` / `Caminho`: um arquivo (se o caminho for de pasta, a pasta).
/// * `Pasta`: todos os arquivos catalogados dentro dela.
/// * `Grupo` (só para mover): as cópias do grupo que têm Marcação para Deleção.
/// * `Volume` (só para restaurar): tudo o que está Na Lixeira do Volume.
///
/// # Errors
/// [`ErroDuplicados`] se o alvo não existe ou não serve para a ação.
pub fn resolver_alvo(conn: &Connection, alvo: &Alvo, restaurar: bool) -> Resultado<AlvoResolvido> {
    match alvo {
        Alvo::Arquivo(_) => um_arquivo(conn, alvo, restaurar),
        Alvo::Caminho(c) => match resolver_arquivo(conn, alvo) {
            Ok(_) => um_arquivo(conn, alvo, restaurar),
            Err(ErroDuplicados::NaoEncontrado(_)) => {
                let (pasta, volume) = resolver_pasta(conn, c)?;
                pasta_resolvida(conn, pasta, volume, c, restaurar)
            }
            Err(e) => Err(e),
        },
        Alvo::Pasta(p) => {
            let (pasta, volume) = resolver_pasta(conn, p)?;
            pasta_resolvida(conn, pasta, volume, p, restaurar)
        }
        Alvo::Grupo(g) if !restaurar => {
            let membros: Vec<i64> = conn
                .prepare("SELECT file_id FROM grupo_membros WHERE grupo_id = ?1 ORDER BY file_id")?
                .query_map([g.0], |l| l.get(0))?
                .collect::<Result<_, _>>()?;
            if membros.is_empty() {
                return Err(ErroDuplicados::NaoEncontrado(format!(
                    "o grupo {g} não existe ou não tem cópias"
                )));
            }
            let mut arquivos = Vec::new();
            for m in membros {
                if let Some(c) = carregar(conn, m)?
                    && c.marca.pode_mover()
                {
                    arquivos.push(c);
                }
            }
            Ok(AlvoResolvido {
                descricao: format!("grupo {g}"),
                eh_pasta: false,
                arquivos,
            })
        }
        Alvo::Volume(v) if restaurar => {
            let id = resolver_volume(conn, v)?;
            let mut st = conn.prepare(&format!(
                "SELECT {COLUNAS} FROM files f
                 WHERE f.volume_id = ?1 AND f.na_lixeira = 1 AND f.state = 'ativo' ORDER BY f.id"
            ))?;
            let linhas: Vec<Linha> = st.query_map([id], ler_linha)?.collect::<Result<_, _>>()?;
            let arquivos = linhas
                .into_iter()
                .map(|l| completar(conn, l))
                .collect::<Resultado<Vec<_>>>()?;
            Ok(AlvoResolvido {
                descricao: format!("Lixeira de {}", nome_do_volume(conn, id)),
                eh_pasta: true,
                arquivos,
            })
        }
        Alvo::Grupo(_) => Err(ErroDuplicados::Invalido(
            "restaurar não aceita grupo: indique o arquivo ou a pasta Na Lixeira".into(),
        )),
        Alvo::Volume(_) => Err(ErroDuplicados::Invalido(
            "mover para a Lixeira não aceita um Volume inteiro: indique arquivos, pastas ou grupos"
                .into(),
        )),
    }
}

fn um_arquivo(conn: &Connection, alvo: &Alvo, restaurar: bool) -> Resultado<AlvoResolvido> {
    let id = resolver_arquivo(conn, alvo)?;
    let c = carregar(conn, id)?
        .ok_or_else(|| ErroDuplicados::NaoEncontrado(format!("arquivo {id} não existe")))?;
    if restaurar && !c.na_lixeira {
        return Err(ErroDuplicados::Invalido(format!(
            "{} não está Na Lixeira",
            c.caminho()
        )));
    }
    Ok(AlvoResolvido {
        descricao: c.caminho(),
        eh_pasta: false,
        arquivos: vec![c],
    })
}

fn pasta_resolvida(
    conn: &Connection,
    pasta: i64,
    volume: i64,
    nome: &str,
    restaurar: bool,
) -> Resultado<AlvoResolvido> {
    let raw = pasta_raw(conn, pasta)?;
    Ok(AlvoResolvido {
        descricao: nome.to_string(),
        eh_pasta: true,
        arquivos: arquivos_sob(conn, volume, &raw, restaurar)?,
    })
}

// ---------------------------------------------------------------------- registro do movimento

/// Uma pasta do caminho de destino, como o disco a mostra depois do `mkdir`/`rename`.
#[derive(Debug, Clone)]
pub struct PastaDoDisco {
    pub path_raw: Vec<u8>,
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: Option<i64>,
}

/// Onde o arquivo ficou.
#[derive(Debug, Clone)]
pub struct NovoLugar {
    pub path_raw: Vec<u8>,
    /// Pastas do destino, da primeira sob o ponto de montagem até a que contém o arquivo.
    pub pastas: Vec<PastaDoDisco>,
    /// ctime depois do `rename` (o `rename` altera o ctime sem alterar o conteúdo).
    pub ctime_ns: Option<i64>,
}

fn garantir_pastas(
    tx: &Connection,
    volume: i64,
    montagem: &[u8],
    pastas: &[PastaDoDisco],
) -> Resultado<i64> {
    let mut pai: Option<i64> = tx
        .query_row(
            "SELECT id FROM folders WHERE volume_id = ?1 AND path_raw = ?2",
            params![volume, montagem],
            |l| l.get(0),
        )
        .optional()?;
    for p in pastas {
        let nome = p
            .path_raw
            .rsplit(|b| *b == b'/')
            .next()
            .unwrap_or_default()
            .to_vec();
        tx.execute(
            "INSERT INTO folders (volume_id, parent_folder_id, path_raw, path_display, name_raw,
                                  name_display, native_dev, native_file_id, mtime_ns, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'ativo')
             ON CONFLICT(volume_id, path_raw) DO UPDATE SET state = 'ativo'",
            params![
                volume,
                pai,
                p.path_raw,
                exibir(&p.path_raw),
                nome,
                exibir(&nome),
                p.dev as i64,
                p.ino as i64,
                p.mtime_ns
            ],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM folders WHERE volume_id = ?1 AND path_raw = ?2",
            params![volume, p.path_raw],
            |l| l.get(0),
        )?;
        pai = Some(id);
    }
    pai.ok_or_else(|| ErroDuplicados::Invalido("destino sem pasta".into()))
}

fn aplicar_lugar(tx: &Connection, c: &Candidato, lugar: &NovoLugar) -> Resultado<()> {
    let pasta = garantir_pastas(tx, c.volume_id, &c.montagem_raw, &lugar.pastas)?;
    let nome = lugar
        .path_raw
        .rsplit(|b| *b == b'/')
        .next()
        .unwrap_or_default()
        .to_vec();
    let antiga: Option<i64> = tx
        .query_row("SELECT folder_id FROM files WHERE id = ?1", [c.id], |l| {
            l.get(0)
        })
        .optional()?;
    tx.execute(
        "UPDATE files SET folder_id = ?2, path_raw = ?3, path_display = ?4, name_raw = ?5,
             name_display = ?6, ctime_ns = ?7 WHERE id = ?1",
        params![
            c.id,
            pasta,
            lugar.path_raw,
            exibir(&lugar.path_raw),
            nome,
            exibir(&nome),
            lugar.ctime_ns
        ],
    )?;
    // O hash continua valendo: o conteúdo é o mesmo; só o ctime mudou com o rename.
    tx.execute(
        "UPDATE hashes SET ctime_ns = ?2 WHERE file_id = ?1",
        params![c.id, lugar.ctime_ns],
    )?;
    for p in [antiga, Some(pasta)].into_iter().flatten() {
        tx.execute(
            "UPDATE folder_stats SET aggregate_state = 'desatualizado' WHERE folder_id = ?1
             AND aggregate_state = 'completo'",
            [p],
        )?;
    }
    Ok(())
}

/// Registra que `c` foi movido para a Lixeira em `lugar`: a linha assume o novo caminho (id,
/// hash e Marcações preservados) e guarda a origem.
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_movido(
    cat: &mut Catalogo,
    c: &Candidato,
    lugar: &NovoLugar,
    agora: i64,
) -> Resultado<()> {
    cat.armazem_mut().transacao_imediata(|tx| -> Resultado<()> {
        aplicar_lugar(tx, c, lugar)?;
        tx.execute(
            "UPDATE files SET na_lixeira = 1, lixeira_origem_raw = ?2, lixeira_origem_display = ?3,
                 lixeira_movido_em = ?4 WHERE id = ?1",
            params![c.id, c.path_raw, exibir(&c.path_raw), agora],
        )?;
        Ok(())
    })
}

/// Registra a restauração de `c` para `lugar`: sai de Na Lixeira e perde a Marcação para
/// Deleção manual (a cópia volta a ser comum).
///
/// # Errors
/// Falha do SQLite.
pub fn registrar_restaurado(
    cat: &mut Catalogo,
    c: &Candidato,
    lugar: &NovoLugar,
    autor: Autor,
    agora: i64,
) -> Resultado<()> {
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Resultado<()> {
            aplicar_lugar(tx, c, lugar)?;
            tx.execute(
                "UPDATE files SET na_lixeira = 0, lixeira_origem_raw = NULL,
                 lixeira_origem_display = NULL, lixeira_movido_em = NULL WHERE id = ?1",
                [c.id],
            )?;
            Ok(())
        })?;
    // Restaurar encerra a candidatura a descarte (só a marcação individual; regras ficam).
    let _ = duplicados::desmarcar(cat, &Alvo::Arquivo(ArquivoId(c.id)), autor, agora);
    Ok(())
}

// ---------------------------------------------------------------------- listagem

/// Totais Na Lixeira.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TotaisLixeira {
    pub itens: u64,
    pub bytes: u64,
    pub descartados: u64,
}

/// Itens Na Lixeira, por id, a partir de `depois` (cursor keyset).
///
/// # Errors
/// Falha do SQLite.
pub fn listar(
    conn: &Connection,
    depois: Option<i64>,
    limite: u32,
) -> Resultado<(Vec<ItemLixeira>, Option<i64>, TotaisLixeira)> {
    let totais = totais(conn)?;
    let mut st = conn.prepare(
        "SELECT id, volume_id, path_display, COALESCE(lixeira_origem_display, ''), logical_bytes,
                lixeira_movido_em
         FROM files WHERE na_lixeira = 1 AND id > ?1 ORDER BY id LIMIT ?2",
    )?;
    let mut itens: Vec<ItemLixeira> = st
        .query_map(params![depois.unwrap_or(0), i64::from(limite) + 1], |l| {
            Ok(ItemLixeira {
                arquivo: ArquivoId(l.get(0)?),
                volume: VolumeId(l.get(1)?),
                caminho: l.get(2)?,
                origem: l.get(3)?,
                bytes: u(l.get(4)?),
                movido_em: l.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let proximo = if itens.len() > limite as usize {
        itens.truncate(limite as usize);
        itens.last().map(|i| i.arquivo.0)
    } else {
        None
    };
    Ok((itens, proximo, totais))
}

/// Contagens da Lixeira e do histórico Descartado.
///
/// # Errors
/// Falha do SQLite.
pub fn totais(conn: &Connection) -> Resultado<TotaisLixeira> {
    let (itens, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(logical_bytes), 0) FROM files WHERE na_lixeira = 1",
        [],
        |l| Ok((l.get(0)?, l.get(1)?)),
    )?;
    let d: i64 = conn.query_row("SELECT COUNT(*) FROM descartados", [], |l| l.get(0))?;
    Ok(TotaisLixeira {
        itens: u(itens),
        bytes: u(bytes),
        descartados: u(d),
    })
}

/// Os últimos registros Descartados.
///
/// # Errors
/// Falha do SQLite.
pub fn ultimos_descartados(conn: &Connection, n: u32) -> Resultado<Vec<ItemDescartado>> {
    let mut st = conn.prepare(
        "SELECT caminho_display, origem_display, hash, bytes, movido_em, descartado_em
         FROM descartados ORDER BY descartado_em DESC, id DESC LIMIT ?1",
    )?;
    Ok(st
        .query_map([i64::from(n)], |l| {
            let h: Option<Vec<u8>> = l.get(2)?;
            Ok(ItemDescartado {
                caminho: l.get(0)?,
                origem: l.get(1)?,
                hash: h.map(|b| b.iter().map(|x| format!("{x:02x}")).collect()),
                bytes: u(l.get(3)?),
                movido_em: l.get(4)?,
                descartado_em: l.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?)
}

// ---------------------------------------------------------------------- reconciliação

/// O que a reconciliação com o disco mudou.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumoLixeira {
    /// Arquivos que o usuário pôs à mão na Lixeira (passaram a Na Lixeira).
    pub postos_na_lixeira: u64,
    /// Arquivos que o usuário tirou da Lixeira (voltaram a ser cópia comum, sem marcação).
    pub retirados: u64,
    /// Arquivos Na Lixeira que sumiram (viraram Descartado).
    pub descartados: u64,
    /// Marcações para Deleção criadas nos que o usuário pôs à mão.
    pub marcados: u64,
}

impl ResumoLixeira {
    /// Algo mudou o que conta como cópia: os Grupos de Duplicados precisam ser refeitos.
    #[must_use]
    pub fn mudou_copias(&self) -> bool {
        self.postos_na_lixeira + self.retirados + self.descartados > 0
    }
}

fn montagens(conn: &Connection) -> Resultado<Vec<(i64, Vec<u8>)>> {
    let mut st = conn.prepare("SELECT volume_id, path_raw FROM mount_points ORDER BY volume_id")?;
    Ok(st
        .query_map([], |l| Ok((l.get(0)?, l.get(1)?)))?
        .collect::<Result<_, _>>()?)
}

/// Reconcilia o Catálogo com o que as varreduras viram em `DAG-Lixeira` (regras da ADR 0005).
/// Só muda o Catálogo; idempotente.
///
/// # Errors
/// Falha do SQLite.
pub fn reconciliar(cat: &mut Catalogo, autor: Autor, agora: i64) -> Resultado<ResumoLixeira> {
    let mut r = ResumoLixeira::default();
    let mps = montagens(cat.armazem().conexao())?;

    // 1. Postos à mão: arquivos ativos sob `<montagem>/DAG-Lixeira/` ainda sem o estado.
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Resultado<()> {
            for (volume, mp) in &mps {
                let (ini, fim) = faixa(&raiz_da_lixeira(mp));
                let achados: Vec<(i64, Vec<u8>)> = tx
                    .prepare(
                        "SELECT id, path_raw FROM files
                     WHERE volume_id = ?1 AND path_raw >= ?2 AND path_raw < ?3
                       AND kind = 'arquivo' AND state = 'ativo' AND na_lixeira = 0",
                    )?
                    .query_map(params![volume, ini, fim], |l| Ok((l.get(0)?, l.get(1)?)))?
                    .collect::<Result<_, _>>()?;
                for (id, caminho) in achados {
                    let Some(origem) = origem_a_partir_da_lixeira(mp, &caminho) else {
                        continue;
                    };
                    tx.execute(
                        "UPDATE files SET na_lixeira = 1, lixeira_origem_raw = ?2,
                         lixeira_origem_display = ?3, lixeira_movido_em = ?4 WHERE id = ?1",
                        params![id, origem, exibir(&origem), agora],
                    )?;
                    r.postos_na_lixeira += 1;
                }
            }
            Ok(())
        })?;

    // 2. Tirados à mão: Na Lixeira, ativos, fora de qualquer `DAG-Lixeira` do Volume.
    let na_lixeira: Vec<(i64, i64, Vec<u8>)> = {
        let conn = cat.armazem().conexao();
        let mut st = conn.prepare(
            "SELECT id, volume_id, path_raw FROM files
             WHERE na_lixeira = 1 AND state = 'ativo' ORDER BY id",
        )?;
        st.query_map([], |l| Ok((l.get(0)?, l.get(1)?, l.get(2)?)))?
            .collect::<Result<_, _>>()?
    };
    let por_volume: BTreeMap<i64, Vec<&Vec<u8>>> =
        mps.iter().fold(BTreeMap::new(), |mut m, (v, p)| {
            m.entry(*v).or_default().push(p);
            m
        });
    let mut tirados = Vec::new();
    for (id, volume, caminho) in &na_lixeira {
        let dentro = por_volume
            .get(volume)
            .is_some_and(|ms| ms.iter().any(|m| esta_na_lixeira(m, caminho)));
        if !dentro {
            tirados.push(*id);
        }
    }
    for id in tirados {
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Resultado<()> {
                tx.execute(
                    "UPDATE files SET na_lixeira = 0, lixeira_origem_raw = NULL,
                     lixeira_origem_display = NULL, lixeira_movido_em = NULL WHERE id = ?1",
                    [id],
                )?;
                Ok(())
            })?;
        // Volta a ser cópia comum, sem marcação (só a individual; regras não são desfeitas).
        let _ = duplicados::desmarcar(cat, &Alvo::Arquivo(ArquivoId(id)), autor, agora);
        r.retirados += 1;
    }

    // 3. Sumidos: estavam Na Lixeira e a linha não está mais ativa (esvaziada pelo usuário).
    cat.armazem_mut()
        .transacao_imediata(|tx| -> Resultado<()> {
            let n = tx.execute(
            "INSERT INTO descartados (file_id, volume_id, caminho_display, origem_display, hash,
                                      bytes, mtime_ns, movido_em, descartado_em)
             SELECT f.id, f.volume_id, f.path_display, f.lixeira_origem_display,
                    (SELECT h.hash FROM hashes h WHERE h.file_id = f.id), f.logical_bytes,
                    f.mtime_ns, f.lixeira_movido_em, ?1
             FROM files f WHERE f.na_lixeira = 1 AND f.state <> 'ativo'",
            [agora],
        )?;
            tx.execute(
                "UPDATE files SET na_lixeira = 0, lixeira_origem_raw = NULL,
                 lixeira_origem_display = NULL, lixeira_movido_em = NULL
             WHERE na_lixeira = 1 AND state <> 'ativo'",
                [],
            )?;
            r.descartados += n as u64;
            Ok(())
        })?;

    // 4. Quem está Na Lixeira passa a ter Marcação para Deleção (os postos à mão, e qualquer
    // cópia Na Lixeira que ainda não tenha: o hash pode ter chegado depois).
    let sem_marca: Vec<i64> = {
        let conn = cat.armazem().conexao();
        let mut st = conn
            .prepare("SELECT id FROM files WHERE na_lixeira = 1 AND state = 'ativo' ORDER BY id")?;
        let ids: Vec<i64> = st.query_map([], |l| l.get(0))?.collect::<Result<_, _>>()?;
        let mut v = Vec::new();
        for id in ids {
            if !matches!(
                situacao(conn, id)?,
                SituacaoMarca::Delecao | SituacaoMarca::DelecaoPorRegra | SituacaoMarca::Pendente
            ) {
                v.push(id);
            }
        }
        v
    };
    for id in sem_marca {
        let alvo = Alvo::Arquivo(ArquivoId(id));
        let mut res = duplicados::marcar(cat, TipoMarcacao::Delecao, &alvo, autor, agora);
        if matches!(res, Err(ErroDuplicados::Conflito(_))) {
            // Tinha Marcação de Backup: o que está Na Lixeira é candidato a descarte.
            if duplicados::desmarcar(cat, &alvo, autor, agora).is_ok() {
                res = duplicados::marcar(cat, TipoMarcacao::Delecao, &alvo, autor, agora);
            }
        }
        if res.is_ok() {
            r.marcados += 1;
        }
        // Sem hash válido ainda: tenta de novo na próxima varredura.
    }
    Ok(r)
}

/// Chaves `(tamanho, hash)` de arquivos, para refazer só os grupos afetados.
///
/// # Errors
/// Falha do SQLite.
pub fn chaves_de_hash(conn: &Connection, ids: &[i64]) -> Resultado<Vec<(u64, [u8; 32])>> {
    let mut v = Vec::new();
    let mut st = conn.prepare("SELECT size_bytes, hash FROM hashes WHERE file_id = ?1")?;
    // Se algum for Contêiner, os Membros dele mudam de situação junto (não contam Na Lixeira).
    let mut sm = conn.prepare(
        "SELECT DISTINCT tamanho, hash FROM membros
         WHERE conteiner_file_id = ?1 AND hash IS NOT NULL AND tipo = 'arquivo' AND tamanho > 0",
    )?;
    for id in ids {
        for (t, h) in sm
            .query_map([id], |l| Ok((l.get::<_, i64>(0)?, l.get::<_, Vec<u8>>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?
        {
            if let Ok(h) = <[u8; 32]>::try_from(h.as_slice()) {
                v.push((u(t), h));
            }
        }
        if let Some((t, h)) = st
            .query_row([id], |l| Ok((l.get::<_, i64>(0)?, l.get::<_, Vec<u8>>(1)?)))
            .optional()?
            && let Ok(h) = <[u8; 32]>::try_from(h.as_slice())
            && t > 0
        {
            v.push((u(t), h));
        }
    }
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// Destino previsto de um candidato, sem tocar no disco.
#[must_use]
pub fn destino_previsto(c: &Candidato) -> Option<Vec<u8>> {
    destino_na_lixeira(&c.montagem_raw, &c.path_raw)
}
