//! Reconciliação direcionada de diretórios (usada pelo monitor, spec §12.3–4).
//!
//! Eventos do filesystem são **dicas**, não verdade: dado um conjunto de diretórios "sujos", este
//! módulo relê cada um do disco (`read_dir` + `lstat`), compara com o Catálogo e aplica a
//! diferença, tudo pelo escritor único. Dentro do lote:
//!
//! * nome novo → linha nova; nome alterado (tamanho, mtime, ctime, inode) → linha atualizada;
//! * nome que sumiu de uma listagem **completa** → candidato a remoção;
//! * candidato sumido + nome novo com o mesmo `(dev, inode, tamanho, mtime)` no mesmo volume →
//!   renomeação (a linha antiga muda de caminho: id, hash e Marcações preservados);
//! * pasta nova com o `(dev, inode)` de uma pasta sumida → pasta renomeada; a pasta é relida
//!   para casar os filhos pelo mesmo critério;
//! * o que sobra sumido vira `ausente`, **exceto** se a raiz estiver indisponível (volume
//!   desmontado, raiz renomeada/removida, `st_dev` diferente), caso em que nada é removido;
//! * diretório sem permissão, listagem interrompida ou `lstat` com erro: nada é removido ali.
//!
//! Arquivos modificados há menos de `estabilidade` voltam em [`ResultadoLote::instaveis`]
//! (ainda sendo gravados): o chamador reconcilia de novo depois do debounce e só então faz hash.

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dag_store::Catalogo;
use dag_store::inventario::{self, NovaPasta, NovoArquivo, TipoEntrada};
use dag_store::reconciliacao::{
    self, Cand, Efeito, NovoEvento, Tabela, TipoEvento, emparelhar, emparelhar_pastas,
};
use rusqlite::{Connection, params};

use crate::classificador::Classificador;
use crate::config::Exclusao;
use crate::erro::ErroScan;
use crate::visita::{self, Entrada, Natureza};

/// Eventos `criado`/`removido`/`alterado` individuais por lote (renomeações sempre).
const MAX_EVENTOS_LOTE: u64 = 10_000;
const MAX_CANDIDATOS_SUBARVORE: usize = 50_000;

fn agora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn agora_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

/// Raiz sob vigilância: volume, caminho e `st_dev` esperado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaizMonitorada {
    pub volume_id: i64,
    pub caminho: Vec<u8>,
    pub dev: u64,
}

/// Diretório sujo e quando a primeira dica chegou (segundos Unix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirSujo {
    pub caminho: Vec<u8>,
    pub observado_em: i64,
}

/// Resultado de um lote.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResultadoLote {
    pub diretorios: u64,
    pub criados: u64,
    pub alterados: u64,
    pub removidos: u64,
    pub renomeados: u64,
    /// Diretórios com arquivos mexidos há pouco tempo: reconciliar de novo depois do debounce.
    pub instaveis: Vec<Vec<u8>>,
    /// Problemas (caminho, motivo); nada foi removido sob eles.
    pub problemas: Vec<(Vec<u8>, String)>,
    /// Índices (em `raizes`) das raízes indisponíveis: o lote não removeu nada ali.
    pub raizes_indisponiveis: Vec<usize>,
    /// Diretórios descartados por estarem fora de qualquer raiz ou em outro filesystem.
    pub ignorados: u64,
    pub eventos_omitidos: u64,
}

impl ResultadoLote {
    /// Algum arquivo mudou de conteúdo/posição (vale rodar hash e duplicados).
    #[must_use]
    pub fn mudou(&self) -> bool {
        self.criados + self.alterados + self.removidos + self.renomeados > 0
    }
}

/// Reconcilia `dirs` com o disco. `run_id` é a execução `monitor` que carimba as linhas
/// novas/alteradas (`seen_run_id`), o que permite à etapa de hash achá-las depois.
///
/// # Errors
/// Falha do Catálogo. Problemas de acesso ao disco não são erros: vão em
/// [`ResultadoLote::problemas`].
#[allow(clippy::too_many_arguments)]
pub fn reconciliar_diretorios(
    cat: &mut Catalogo,
    run_id: i64,
    raizes: &[RaizMonitorada],
    dirs: &[DirSujo],
    classificador: &dyn Classificador,
    exclusoes: &[Exclusao],
    estabilidade: Duration,
    fonte: &str,
) -> Result<ResultadoLote, ErroScan> {
    let mut e = Estado {
        run_id,
        raizes,
        classificador,
        exclusoes,
        estabilidade_ns: i64::try_from(estabilidade.as_nanos()).unwrap_or(i64::MAX),
        fonte,
        t: agora(),
        fila: VecDeque::new(),
        visto: HashSet::new(),
        disponivel: HashMap::new(),
        vel_arq: Vec::new(),
        vel_pastas: Vec::new(),
        novos_arq: Vec::new(),
        novas_pastas: Vec::new(),
        tipos: HashMap::new(),
        eventos: 0,
        res: ResultadoLote::default(),
    };
    let mut ordenados: Vec<&DirSujo> = dirs.iter().collect();
    ordenados.sort_by_key(|a| a.caminho.len());
    for d in ordenados {
        e.agendar(d.caminho.clone(), d.observado_em);
    }
    loop {
        while let Some((dir, obs)) = e.fila.pop_front() {
            if !e.visto.insert(dir.clone()) {
                continue;
            }
            cat.armazem_mut()
                .transacao_imediata(|tx| e.processar_dir(tx, &dir, obs))?;
        }
        // Pastas renomeadas: a nova assume a linha da antiga e é relida para casar os filhos.
        let pares = emparelhar_pastas(
            &e.vel_pastas
                .iter()
                .map(|v| v.cand.clone())
                .collect::<Vec<_>>(),
            &e.novas_pastas
                .iter()
                .map(|v| v.cand.clone())
                .collect::<Vec<_>>(),
        );
        if pares.is_empty() {
            break;
        }
        let mut relistar = Vec::new();
        cat.armazem_mut()
            .transacao_imediata(|tx| -> Result<(), ErroScan> {
                for (velha, nova) in &pares {
                    reconciliacao::aplicar_renomeacao_pasta(
                        tx, *velha, *nova, e.run_id, e.fonte, e.t,
                    )?;
                    e.res.renomeados += 1;
                    if let Some(n) = e.novas_pastas.iter().find(|n| n.cand.id == *nova) {
                        relistar.push((n.caminho.clone(), n.observado_em));
                    }
                }
                Ok(())
            })?;
        let (velhas, novas): (HashSet<i64>, HashSet<i64>) = pares.iter().copied().unzip();
        e.vel_pastas.retain(|v| !velhas.contains(&v.cand.id));
        e.novas_pastas.retain(|n| !novas.contains(&n.cand.id));
        for (caminho, obs) in relistar {
            e.visto.remove(&caminho);
            e.agendar(caminho, obs);
        }
        if e.fila.is_empty() {
            break;
        }
    }
    cat.armazem_mut().transacao_imediata(|tx| e.finalizar(tx))?;
    e.res.instaveis.sort();
    e.res.instaveis.dedup();
    Ok(e.res)
}

#[derive(Debug)]
struct Rastro {
    cand: Cand,
    caminho: Vec<u8>,
    observado_em: i64,
}

struct Estado<'a> {
    run_id: i64,
    raizes: &'a [RaizMonitorada],
    classificador: &'a dyn Classificador,
    exclusoes: &'a [Exclusao],
    estabilidade_ns: i64,
    fonte: &'a str,
    t: i64,
    fila: VecDeque<(Vec<u8>, i64)>,
    visto: HashSet<Vec<u8>>,
    disponivel: HashMap<usize, bool>,
    vel_arq: Vec<Rastro>,
    vel_pastas: Vec<Rastro>,
    novos_arq: Vec<Rastro>,
    novas_pastas: Vec<Rastro>,
    tipos: HashMap<(Option<String>, Option<String>, &'static str), i64>,
    eventos: u64,
    res: ResultadoLote,
}

fn pai_de(caminho: &[u8]) -> Option<&[u8]> {
    match caminho.iter().rposition(|b| *b == b'/') {
        Some(0) if caminho.len() > 1 => Some(&caminho[..1]),
        Some(i) if i > 0 => Some(&caminho[..i]),
        _ => None,
    }
}

fn nome_de(caminho: &[u8]) -> &[u8] {
    match caminho.iter().rposition(|b| *b == b'/') {
        Some(i) if caminho.len() > 1 => &caminho[i + 1..],
        _ => caminho,
    }
}

impl Estado<'_> {
    fn agendar(&mut self, caminho: Vec<u8>, obs: i64) {
        self.fila.push_back((caminho, obs));
    }

    fn raiz_de(&self, caminho: &[u8]) -> Option<usize> {
        self.raizes
            .iter()
            .enumerate()
            .filter(|(_, r)| reconciliacao::dentro_de(caminho, &r.caminho))
            .max_by_key(|(_, r)| r.caminho.len())
            .map(|(i, _)| i)
    }

    /// A raiz continua sendo o mesmo diretório no mesmo filesystem? Senão nada é removido.
    fn raiz_ok(&mut self, i: usize) -> bool {
        if let Some(ok) = self.disponivel.get(&i) {
            return *ok;
        }
        let r = &self.raizes[i];
        let ok = visita::lstat(&r.caminho).is_ok_and(|m| m.is_dir() && m.dev() == r.dev);
        self.disponivel.insert(i, ok);
        if !ok {
            self.res.raizes_indisponiveis.push(i);
        }
        ok
    }

    fn problema(&mut self, caminho: &[u8], motivo: String) {
        self.res.problemas.push((caminho.to_vec(), motivo));
    }

    #[allow(clippy::too_many_arguments)]
    fn evento(
        &mut self,
        tx: &Connection,
        tipo: TipoEvento,
        volume_id: i64,
        caminho: &[u8],
        arquivo_id: Option<i64>,
        obs: i64,
        detalhe: Option<&str>,
    ) -> Result<(), ErroScan> {
        if self.eventos >= MAX_EVENTOS_LOTE {
            self.res.eventos_omitidos += 1;
            return Ok(());
        }
        self.eventos += 1;
        reconciliacao::registrar_evento(
            tx,
            &NovoEvento {
                volume_id: Some(volume_id),
                caminho: Some(caminho),
                arquivo_id,
                execucao_id: Some(self.run_id),
                observado_em: obs,
                detalhe,
                ..NovoEvento::processado(tipo, self.fonte, self.t)
            },
        )?;
        Ok(())
    }

    /// Pasta do Catálogo para `caminho`, criando-a (e os ancestrais) se preciso.
    fn garantir_pasta(
        &mut self,
        tx: &Connection,
        ri: usize,
        caminho: &[u8],
    ) -> Result<i64, ErroScan> {
        let raiz = self.raizes[ri].clone();
        if let Ok(id) = tx.query_row(
            "SELECT id FROM folders WHERE volume_id = ?1 AND path_raw = ?2",
            params![raiz.volume_id, caminho],
            |l| l.get::<_, i64>(0),
        ) {
            return Ok(id);
        }
        let m = visita::lstat(caminho)
            .map_err(|e| ErroScan::Interno(format!("pasta sumiu durante a reconciliação: {e}")))?;
        let pai = if caminho == raiz.caminho.as_slice() {
            None
        } else {
            match pai_de(caminho) {
                Some(p) => Some(self.garantir_pasta(tx, ri, p)?),
                None => None,
            }
        };
        let nome = nome_de(caminho);
        let entrada = visita::entrada_de(caminho.to_vec(), caminho.len() - nome.len(), &m);
        let nova = NovaPasta {
            volume_id: raiz.volume_id,
            pai,
            path_raw: caminho,
            name_raw: nome,
            dev: entrada.dev,
            inode: entrada.inode,
            attrs: &entrada.attrs,
            mtime_ns: entrada.mtime_ns,
            run_id: self.run_id,
        };
        if pai.is_none() {
            return Ok(reconciliacao::registrar_raiz(tx, &nova)?);
        }
        let (id, efeito) = reconciliacao::gravar_pasta(tx, &nova)?;
        if efeito == Efeito::Novo {
            self.novas_pastas.push(Rastro {
                cand: Cand {
                    id,
                    volume_id: raiz.volume_id,
                    dev: entrada.dev as i64,
                    inode: entrada.inode as i64,
                    tamanho: 0,
                    mtime_ns: entrada.mtime_ns.unwrap_or(i64::MIN),
                    nome: nome.to_vec(),
                },
                caminho: caminho.to_vec(),
                observado_em: self.t,
            });
        }
        Ok(id)
    }

    fn mime_id(&mut self, tx: &Connection, e: &Entrada) -> Result<Option<i64>, ErroScan> {
        let Some(td) = &e.tipo else { return Ok(None) };
        let chave = (td.extensao.clone(), td.mime.clone(), td.fonte.como_str());
        if let Some(id) = self.tipos.get(&chave) {
            return Ok(Some(*id));
        }
        let id = inventario::obter_tipo_de_arquivo(
            tx,
            td.extensao.as_deref(),
            td.mime.as_deref(),
            td.descricao.as_deref(),
            td.categoria.as_deref(),
            td.fonte.como_str(),
            td.confianca,
        )?;
        self.tipos.insert(chave, id);
        Ok(Some(id))
    }

    fn processar_dir(&mut self, tx: &Connection, dir: &[u8], obs: i64) -> Result<(), ErroScan> {
        let Some(ri) = self.raiz_de(dir) else {
            self.res.ignorados += 1;
            return Ok(());
        };
        if !self.raiz_ok(ri) {
            return Ok(());
        }
        let raiz = self.raizes[ri].clone();
        match visita::lstat(dir) {
            Ok(m) if m.is_dir() => {
                if m.dev() != raiz.dev {
                    // Outro filesystem montado sob a raiz: fronteira, não se cruza.
                    self.res.ignorados += 1;
                    return Ok(());
                }
            }
            Ok(_) => {
                // Virou arquivo: o pai decide (a linha da pasta some pela listagem dele).
                self.agendar_pai(dir, &raiz, obs);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.agendar_pai(dir, &raiz, obs);
                return Ok(());
            }
            Err(e) => {
                self.problema(dir, format!("lstat: {e}"));
                return Ok(());
            }
        }
        let lista = match visita::ler_diretorio(dir, self.classificador, self.exclusoes) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.agendar_pai(dir, &raiz, obs);
                return Ok(());
            }
            Err(e) => {
                self.problema(dir, format!("não foi possível listar: {e}"));
                return Ok(());
            }
        };
        self.res.diretorios += 1;
        for p in &lista.problemas {
            self.problema(&p.caminho, p.motivo.clone());
        }
        let pasta_id = self.garantir_pasta(tx, ri, dir)?;

        struct LinhaArq {
            id: i64,
            ativo: bool,
            dev: Option<i64>,
            inode: Option<i64>,
            tamanho: i64,
            mtime: Option<i64>,
            nome: Vec<u8>,
        }
        struct LinhaPasta {
            id: i64,
            ativo: bool,
            dev: Option<i64>,
            inode: Option<i64>,
            mtime: Option<i64>,
            nome: Vec<u8>,
        }
        let arquivos_cat: HashMap<Vec<u8>, LinhaArq> = {
            let mut st = tx.prepare_cached(
                "SELECT id, path_raw, state, native_dev, native_file_id, logical_bytes, mtime_ns,
                        name_raw FROM files WHERE folder_id = ?1",
            )?;
            st.query_map([pasta_id], |l| {
                Ok((
                    l.get::<_, Vec<u8>>(1)?,
                    LinhaArq {
                        id: l.get(0)?,
                        ativo: l.get::<_, String>(2)? == "ativo",
                        dev: l.get(3)?,
                        inode: l.get(4)?,
                        tamanho: l.get(5)?,
                        mtime: l.get(6)?,
                        nome: l.get(7)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?
        };
        let pastas_cat: HashMap<Vec<u8>, LinhaPasta> = {
            let mut st = tx.prepare_cached(
                "SELECT id, path_raw, state, native_dev, native_file_id, mtime_ns, name_raw
                 FROM folders WHERE parent_folder_id = ?1",
            )?;
            st.query_map([pasta_id], |l| {
                Ok((
                    l.get::<_, Vec<u8>>(1)?,
                    LinhaPasta {
                        id: l.get(0)?,
                        ativo: l.get::<_, String>(2)? == "ativo",
                        dev: l.get(3)?,
                        inode: l.get(4)?,
                        mtime: l.get(5)?,
                        nome: l.get(6)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?
        };

        let mut no_disco: HashSet<&[u8]> = HashSet::new();
        let mut pastas_no_disco: HashSet<&[u8]> = HashSet::new();
        let agora_ns = agora_ns();
        let mut instavel = false;
        for e in &lista.entradas {
            match e.natureza {
                Natureza::Pasta => {
                    pastas_no_disco.insert(&e.caminho);
                    if e.dev != raiz.dev {
                        continue;
                    }
                    let igual = pastas_cat.get(&e.caminho).is_some_and(|p| {
                        p.ativo && p.dev == Some(e.dev as i64) && p.inode == Some(e.inode as i64)
                    });
                    if igual {
                        continue;
                    }
                    let (id, efeito) = reconciliacao::gravar_pasta(
                        tx,
                        &NovaPasta {
                            volume_id: raiz.volume_id,
                            pai: Some(pasta_id),
                            path_raw: &e.caminho,
                            name_raw: e.nome(),
                            dev: e.dev,
                            inode: e.inode,
                            attrs: &e.attrs,
                            mtime_ns: e.mtime_ns,
                            run_id: self.run_id,
                        },
                    )?;
                    // Pasta nova, trocada ou de volta: relê o conteúdo.
                    self.agendar(e.caminho.clone(), obs);
                    if efeito == Efeito::Novo {
                        self.novas_pastas.push(Rastro {
                            cand: Cand {
                                id,
                                volume_id: raiz.volume_id,
                                dev: e.dev as i64,
                                inode: e.inode as i64,
                                tamanho: 0,
                                mtime_ns: e.mtime_ns.unwrap_or(i64::MIN),
                                nome: e.nome().to_vec(),
                            },
                            caminho: e.caminho.clone(),
                            observado_em: obs,
                        });
                    }
                }
                Natureza::Outra(tipo) => {
                    no_disco.insert(&e.caminho);
                    let mime_id = if tipo == TipoEntrada::Arquivo {
                        self.mime_id(tx, e)?
                    } else {
                        None
                    };
                    let novo = NovoArquivo {
                        volume_id: raiz.volume_id,
                        pasta_id,
                        path_raw: &e.caminho,
                        name_raw: e.nome(),
                        tipo,
                        dev: e.dev,
                        inode: e.inode,
                        nlink: e.nlink,
                        bytes_logicos: e.tamanho,
                        bytes_alocados: e.alocado,
                        mtime_ns: e.mtime_ns,
                        ctime_ns: e.ctime_ns,
                        birthtime_ns: e.birth_ns,
                        attrs: &e.attrs,
                        alvo_symlink: e.alvo.as_deref(),
                        mime_id,
                        run_id: self.run_id,
                    };
                    let efeito = reconciliacao::gravar_arquivo(tx, &novo)?;
                    if efeito == Efeito::Inalterado {
                        continue;
                    }
                    if e.mtime_ns
                        .is_some_and(|m| agora_ns.saturating_sub(m) < self.estabilidade_ns)
                    {
                        instavel = true;
                    }
                    match efeito {
                        Efeito::Alterado => {
                            self.res.alterados += 1;
                            let id = arquivos_cat.get(&e.caminho).map(|a| a.id);
                            self.evento(
                                tx,
                                TipoEvento::Alterado,
                                raiz.volume_id,
                                &e.caminho,
                                id,
                                obs,
                                None,
                            )?;
                        }
                        Efeito::Novo => {
                            let id: i64 = tx.query_row(
                                "SELECT id FROM files WHERE volume_id = ?1 AND path_raw = ?2",
                                params![raiz.volume_id, e.caminho],
                                |l| l.get(0),
                            )?;
                            self.novos_arq.push(Rastro {
                                cand: Cand {
                                    id,
                                    volume_id: raiz.volume_id,
                                    dev: e.dev as i64,
                                    inode: e.inode as i64,
                                    tamanho: e.tamanho as i64,
                                    mtime_ns: e.mtime_ns.unwrap_or(i64::MIN),
                                    nome: e.nome().to_vec(),
                                },
                                caminho: e.caminho.clone(),
                                observado_em: obs,
                            });
                        }
                        Efeito::Inalterado => {}
                    }
                }
            }
        }
        if instavel {
            self.res.instaveis.push(dir.to_vec());
        }
        // Sumiços: só com listagem completa, e nunca para caminhos incertos.
        if lista.completa {
            let incertos: HashSet<&[u8]> = lista.incertos.iter().map(Vec::as_slice).collect();
            for (caminho, a) in &arquivos_cat {
                if !a.ativo
                    || no_disco.contains(caminho.as_slice())
                    || incertos.contains(caminho.as_slice())
                {
                    continue;
                }
                let cand = match (a.dev, a.inode, a.mtime) {
                    (Some(dev), Some(inode), Some(mtime)) => Cand {
                        id: a.id,
                        volume_id: raiz.volume_id,
                        dev,
                        inode,
                        tamanho: a.tamanho,
                        mtime_ns: mtime,
                        nome: a.nome.clone(),
                    },
                    // Sem evidência de identidade: nunca casa com nada (inode impossível).
                    _ => Cand {
                        id: a.id,
                        volume_id: raiz.volume_id,
                        dev: -1,
                        inode: -1,
                        tamanho: -1,
                        mtime_ns: i64::MIN,
                        nome: a.nome.clone(),
                    },
                };
                self.vel_arq.push(Rastro {
                    cand,
                    caminho: caminho.clone(),
                    observado_em: obs,
                });
            }
            for (caminho, p) in &pastas_cat {
                if !p.ativo
                    || pastas_no_disco.contains(caminho.as_slice())
                    || incertos.contains(caminho.as_slice())
                {
                    continue;
                }
                let cand = match (p.dev, p.inode) {
                    (Some(dev), Some(inode)) => Cand {
                        id: p.id,
                        volume_id: raiz.volume_id,
                        dev,
                        inode,
                        tamanho: 0,
                        mtime_ns: p.mtime.unwrap_or(i64::MIN),
                        nome: p.nome.clone(),
                    },
                    _ => Cand {
                        id: p.id,
                        volume_id: raiz.volume_id,
                        dev: -1,
                        inode: -1,
                        tamanho: 0,
                        mtime_ns: i64::MIN,
                        nome: p.nome.clone(),
                    },
                };
                self.vel_pastas.push(Rastro {
                    cand,
                    caminho: caminho.clone(),
                    observado_em: obs,
                });
            }
        }
        Ok(())
    }

    fn agendar_pai(&mut self, dir: &[u8], raiz: &RaizMonitorada, obs: i64) {
        if dir != raiz.caminho.as_slice()
            && let Some(p) = pai_de(dir)
            && reconciliacao::dentro_de(p, &raiz.caminho)
        {
            self.agendar(p.to_vec(), obs);
        }
    }

    /// Renomeações de arquivos, remoções e eventos de criação, no fim do lote.
    fn finalizar(&mut self, tx: &Connection) -> Result<(), ErroScan> {
        // Pastas sumidas e ainda sem par: seus arquivos podem ter sido movidos para outro lugar
        // antes da pasta ser apagada; entram como candidatos a renomeação antes da remoção.
        let vel_pastas = std::mem::take(&mut self.vel_pastas);
        for p in &vel_pastas {
            let Some(ri) = self.raiz_de(&p.caminho) else {
                continue;
            };
            if !self.raiz_ok(ri) {
                continue;
            }
            let vol = self.raizes[ri].volume_id;
            let (lo, hi) = {
                let mut base = p.caminho.clone();
                if base.last() == Some(&b'/') {
                    base.pop();
                }
                let mut lo = base.clone();
                lo.push(b'/');
                let mut hi = base;
                hi.push(b'0');
                (lo, hi)
            };
            let mut st = tx.prepare_cached(
                "SELECT id, native_dev, native_file_id, logical_bytes, mtime_ns, name_raw, path_raw
                 FROM files WHERE volume_id = ?1 AND state = 'ativo' AND path_raw >= ?2
                   AND path_raw < ?3 AND native_dev IS NOT NULL AND native_file_id IS NOT NULL
                   AND mtime_ns IS NOT NULL LIMIT ?4",
            )?;
            let linhas =
                st.query_map(params![vol, lo, hi, MAX_CANDIDATOS_SUBARVORE as i64], |l| {
                    Ok(Rastro {
                        cand: Cand {
                            id: l.get(0)?,
                            volume_id: vol,
                            dev: l.get(1)?,
                            inode: l.get(2)?,
                            tamanho: l.get(3)?,
                            mtime_ns: l.get(4)?,
                            nome: l.get(5)?,
                        },
                        caminho: l.get(6)?,
                        observado_em: p.observado_em,
                    })
                })?;
            for r in linhas {
                self.vel_arq.push(r?);
            }
        }
        let vel: Vec<Cand> = self.vel_arq.iter().map(|r| r.cand.clone()).collect();
        let novos: Vec<Cand> = self.novos_arq.iter().map(|r| r.cand.clone()).collect();
        let pares = emparelhar(&vel, &novos);
        let mut renomeados_velhos = HashSet::new();
        let mut renomeados_novos = HashSet::new();
        for (velho, novo) in &pares {
            reconciliacao::aplicar_renomeacao_arquivo(
                tx,
                *velho,
                *novo,
                self.run_id,
                self.fonte,
                self.t,
                true,
            )?;
            self.res.renomeados += 1;
            renomeados_velhos.insert(*velho);
            renomeados_novos.insert(*novo);
        }
        // Remoções de arquivos (listagens completas de raízes disponíveis).
        let vel_arq = std::mem::take(&mut self.vel_arq);
        for r in vel_arq {
            if renomeados_velhos.contains(&r.cand.id) {
                continue;
            }
            let Some(ri) = self.raiz_de(&r.caminho) else {
                continue;
            };
            if !self.raiz_ok(ri) {
                continue;
            }
            if reconciliacao::marcar_ausente(tx, Tabela::Arquivos, r.cand.id)?.is_some() {
                self.res.removidos += 1;
                self.evento(
                    tx,
                    TipoEvento::Removido,
                    r.cand.volume_id,
                    &r.caminho,
                    Some(r.cand.id),
                    r.observado_em,
                    None,
                )?;
            }
        }
        // Pastas removidas (subárvore inteira).
        for p in vel_pastas {
            let Some(ri) = self.raiz_de(&p.caminho) else {
                continue;
            };
            if !self.raiz_ok(ri) {
                continue;
            }
            let n = reconciliacao::marcar_subarvore_ausente(tx, p.cand.volume_id, &p.caminho)?;
            if n > 0 {
                self.res.removidos += 1;
                self.evento(
                    tx,
                    TipoEvento::Removido,
                    p.cand.volume_id,
                    &p.caminho,
                    None,
                    p.observado_em,
                    Some("pasta"),
                )?;
            }
        }
        // Criações que sobraram.
        let novos_arq = std::mem::take(&mut self.novos_arq);
        for r in novos_arq {
            if renomeados_novos.contains(&r.cand.id) {
                continue;
            }
            self.res.criados += 1;
            self.evento(
                tx,
                TipoEvento::Criado,
                r.cand.volume_id,
                &r.caminho,
                Some(r.cand.id),
                r.observado_em,
                None,
            )?;
        }
        Ok(())
    }
}
