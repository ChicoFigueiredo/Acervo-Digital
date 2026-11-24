//! Lixeira em dois níveis, lado agente (ADR 0005): mover para a `DAG-Lixeira` do Volume,
//! restaurar e listar. O agente é o escritor único (ADR 0004): GUI, CLI e MCP pedem; só ele
//! executa o `rename`, e só pelo módulo [`fs`].
//!
//! Ordem de segurança de cada pedido: resolve os alvos no Catálogo → recusa o que não tem
//! Marcação para Deleção, o que é Única Cópia sem confirmação e as pastas com arquivo sem
//! marcação → para cada arquivo, confere o disco contra o Catálogo e só então faz o `rename`
//! → registra no Catálogo. Qualquer recusa deixa o arquivo exatamente onde estava.

pub mod fs;

use std::collections::{BTreeSet, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;

use dag_core::{
    AcaoLixeira, Alvo, ArquivoId, ItemMovido, ItemRecusado, Resposta, ResultadoLixeira, VolumeId,
};
use dag_store::duplicados::{self, Escopo, outras_copias};
use dag_store::lixeira::{
    self, Candidato, NovoLugar, SituacaoMarca, destino_previsto, registrar_movido,
    registrar_restaurado, resolver_alvo,
};

use crate::agente::{Agente, agora};
use crate::erro::ErroAgente;

pub use fs::{Dispositivos, DispositivosReais, ErroMover};

/// Quantos arquivos aparecem por extenso nas listas de motivos e na simulação.
const AMOSTRA: usize = 20;
const MAX_ITENS_NA_RESPOSTA: usize = 1000;

fn texto(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn amostra(v: &[&Candidato]) -> Vec<String> {
    v.iter().take(AMOSTRA).map(|c| c.caminho()).collect()
}

fn recusado(
    alvo: &str,
    codigo: &str,
    motivo: impl Into<String>,
    arquivos: Vec<String>,
) -> ItemRecusado {
    ItemRecusado {
        alvo: alvo.to_string(),
        codigo: codigo.to_string(),
        motivo: motivo.into(),
        arquivos,
    }
}

fn dispositivos_reais() -> Arc<dyn Dispositivos> {
    Arc::new(DispositivosReais)
}

impl Agente {
    /// Troca a conferência de `st_dev` (testes simulam outro Volume).
    #[must_use]
    pub fn com_dispositivos(mut self, d: Arc<dyn Dispositivos>) -> Self {
        self.dispositivos = d;
        self
    }

    pub(crate) fn dispositivos_padrao() -> Arc<dyn Dispositivos> {
        dispositivos_reais()
    }

    /// `MoverParaLixeira`.
    pub(crate) fn mover_para_lixeira(
        &mut self,
        alvos: &[Alvo],
        confirmacao_unica_copia: bool,
        marcar_tudo: bool,
        simular: bool,
    ) -> Result<Resposta, ErroAgente> {
        if alvos.is_empty() {
            return Err(ErroAgente::Invalido("informe ao menos um alvo".into()));
        }
        let mut res = ResultadoLixeira {
            acao: AcaoLixeira::Mover,
            simulado: simular,
            movidos: Vec::new(),
            recusados: Vec::new(),
            total_arquivos: 0,
            total_bytes: 0,
            unicas_copias: 0,
            marcados_agora: 0,
            destinos: Vec::new(),
            volumes: Vec::new(),
        };
        // 1. Resolve e confere as Marcações, alvo a alvo.
        let mut aceitos: Vec<(String, Vec<Candidato>)> = Vec::new();
        let mut a_marcar: Vec<(String, Vec<Candidato>)> = Vec::new();
        {
            let conn = self.catalogo.armazem().conexao();
            for alvo in alvos {
                let r = match resolver_alvo(conn, alvo, false) {
                    Ok(r) => r,
                    Err(e) => {
                        res.recusados.push(recusado(
                            &descricao_alvo(alvo),
                            e.codigo(),
                            e.to_string(),
                            vec![],
                        ));
                        continue;
                    }
                };
                if r.arquivos.is_empty() {
                    res.recusados.push(recusado(
                        &r.descricao,
                        "sem_arquivos",
                        if r.eh_pasta {
                            "nenhum arquivo catalogado ainda no lugar dentro desta pasta"
                        } else {
                            "nenhuma cópia com Marcação para Deleção neste alvo"
                        },
                        vec![],
                    ));
                    continue;
                }
                let (bons, sem): (Vec<&Candidato>, Vec<&Candidato>) =
                    r.arquivos.iter().partition(|c| c.marca.pode_mover());
                let _ = bons;
                if !sem.is_empty() {
                    if r.eh_pasta && marcar_tudo {
                        let impossiveis: Vec<&Candidato> = sem
                            .iter()
                            .copied()
                            .filter(|c| {
                                matches!(c.marca, SituacaoMarca::Backup | SituacaoMarca::Pendente)
                            })
                            .collect();
                        if impossiveis.is_empty() {
                            a_marcar.push((
                                r.descricao.clone(),
                                sem.iter().map(|c| (*c).clone()).collect(),
                            ));
                        } else {
                            res.recusados.push(recusado(
                                &r.descricao,
                                "sem_marcacao",
                                format!(
                                    "{} arquivo(s) têm Marcação de Backup ou Marcação para Deleção pendente e não podem ser marcados automaticamente; resolva-os antes",
                                    impossiveis.len()
                                ),
                                amostra(&impossiveis),
                            ));
                            continue;
                        }
                    } else {
                        let motivo = if r.eh_pasta {
                            format!(
                                "a pasta só vai para a Lixeira se todos os arquivos catalogados dentro dela têm Marcação para Deleção; {} não têm (use `marcar tudo e mover` para marcá-los num passo explícito)",
                                sem.len()
                            )
                        } else {
                            format!(
                                "{} não tem Marcação para Deleção ativa (marque antes de mover)",
                                sem[0].caminho()
                            )
                        };
                        res.recusados.push(recusado(
                            &r.descricao,
                            "sem_marcacao",
                            motivo,
                            amostra(&sem),
                        ));
                        continue;
                    }
                }
                aceitos.push((r.descricao, r.arquivos));
            }
        }
        // 2. "Marcar tudo e mover": passo explícito; em simulação só é contado.
        for (desc, arquivos) in a_marcar {
            if !simular {
                let mut falhou: Vec<String> = Vec::new();
                for c in arquivos.iter().filter(|c| !c.marca.pode_mover()) {
                    match duplicados::marcar(
                        &mut self.catalogo,
                        dag_core::TipoMarcacao::Delecao,
                        &Alvo::Arquivo(ArquivoId(c.id)),
                        self.autor,
                        agora(),
                    ) {
                        Ok(_) => res.marcados_agora += 1,
                        Err(e) => falhou.push(format!("{}: {e}", c.caminho())),
                    }
                }
                if !falhou.is_empty() {
                    res.recusados.push(recusado(
                        &desc,
                        "sem_marcacao",
                        format!(
                            "não consegui marcar {} arquivo(s); a pasta não foi movida (as marcações já feitas continuam)",
                            falhou.len()
                        ),
                        falhou.into_iter().take(AMOSTRA).collect(),
                    ));
                    continue;
                }
            } else {
                res.marcados_agora +=
                    arquivos.iter().filter(|c| !c.marca.pode_mover()).count() as u64;
            }
            // Recarrega com as marcações novas.
            let recarregado = if simular {
                arquivos
            } else {
                match resolver_alvo(
                    self.catalogo.armazem().conexao(),
                    &Alvo::Pasta(desc.clone()),
                    false,
                ) {
                    Ok(r) => r.arquivos,
                    Err(_) => arquivos,
                }
            };
            aceitos.push((desc, recarregado));
        }
        // 3. Hard links ficam de fora (mover só um nome não remove nada); duplicatas de alvo.
        let mut vistos: HashSet<i64> = HashSet::new();
        let mut plano: Vec<(String, Candidato)> = Vec::new();
        for (desc, arquivos) in aceitos {
            let mut hl = Vec::new();
            for c in arquivos {
                if c.hard_link {
                    hl.push(c.caminho());
                } else if vistos.insert(c.id) {
                    plano.push((desc.clone(), c));
                }
            }
            if !hl.is_empty() {
                res.recusados.push(recusado(
                    &desc,
                    "hard_link",
                    format!(
                        "{} arquivo(s) têm outros nomes (hard links) para o mesmo conteúdo: mover um só não remove nada, então não vão",
                        hl.len()
                    ),
                    hl.into_iter().take(AMOSTRA).collect(),
                ));
            }
        }
        // 4. Única Cópia: o que sobra fora do lote e fora da Lixeira decide, até estabilizar
        // (recusar um alvo mantém as cópias dele fora da Lixeira).
        let mut unicas: BTreeSet<i64>;
        loop {
            let ids: HashSet<i64> = plano.iter().map(|(_, c)| c.id).collect();
            let conn = self.catalogo.armazem().conexao();
            unicas = BTreeSet::new();
            for (_, c) in &plano {
                if c.hash.is_none() {
                    continue;
                }
                let outras = outras_copias(conn, c.id)?;
                if outras.iter().all(|o| ids.contains(o)) {
                    unicas.insert(c.id);
                }
            }
            if confirmacao_unica_copia || unicas.is_empty() {
                break;
            }
            // Recusa os alvos que têm Única Cópia.
            let ruins: BTreeSet<String> = plano
                .iter()
                .filter(|(_, c)| unicas.contains(&c.id))
                .map(|(d, _)| d.clone())
                .collect();
            for d in &ruins {
                let lista: Vec<&Candidato> = plano
                    .iter()
                    .filter(|(dd, c)| dd == d && unicas.contains(&c.id))
                    .map(|(_, c)| c)
                    .collect();
                res.recusados.push(recusado(
                    d,
                    "unica_copia",
                    format!(
                        "{} arquivo(s) são Única Cópia: não existe outra cópia fora da Lixeira no Catálogo. Mover exige confirmar que esta é a última cópia (--ultima-copia)",
                        lista.len()
                    ),
                    amostra(&lista),
                ));
            }
            plano.retain(|(d, _)| !ruins.contains(d));
        }
        res.unicas_copias = unicas.len() as u64;
        res.total_arquivos = plano.len() as u64;
        res.total_bytes = plano.iter().map(|(_, c)| c.bytes).sum();
        let mut destinos = BTreeSet::new();
        let mut volumes = BTreeSet::new();
        for (_, c) in &plano {
            destinos.insert(texto(&c.raiz_da_lixeira_absoluta()));
            volumes.insert(c.volume_nome.clone());
        }
        res.destinos = destinos.into_iter().collect();
        res.volumes = volumes.into_iter().collect();
        if simular {
            for (_, c) in plano.iter().take(MAX_ITENS_NA_RESPOSTA) {
                res.movidos.push(ItemMovido {
                    arquivo: ArquivoId(c.id),
                    volume: VolumeId(c.volume_id),
                    de: c.caminho(),
                    para: destino_previsto(c).map(|d| texto(&d)).unwrap_or_default(),
                    bytes: c.bytes,
                    unica_copia: unicas.contains(&c.id),
                });
            }
            return Ok(Resposta::Lixeira { resultado: res });
        }
        // 5. Executa: confere o disco, `rename`, registra.
        let disp = Arc::clone(&self.dispositivos);
        let mut chaves_ids = Vec::new();
        let mut movidos_ok = 0u64;
        let mut bytes_ok = 0u64;
        for (_, c) in &plano {
            if c.marca == SituacaoMarca::DelecaoPorRegra
                && let Err(e) = duplicados::fixar_delecao_individual(
                    &mut self.catalogo,
                    c.id,
                    self.autor,
                    agora(),
                )
            {
                res.recusados.push(recusado(
                    &c.caminho(),
                    "sem_marcacao",
                    format!("não consegui fixar a Marcação na cópia antes de movê-la: {e}"),
                    vec![],
                ));
                continue;
            }
            match fs::mover_para_lixeira(c, &*disp) {
                Ok(m) => {
                    let lugar = NovoLugar {
                        path_raw: m.destino.clone(),
                        pastas: m.pastas,
                        ctime_ns: m.ctime_ns,
                    };
                    if let Err(e) = registrar_movido(&mut self.catalogo, c, &lugar, agora()) {
                        // O arquivo já está na Lixeira; a próxima varredura reconcilia.
                        res.recusados.push(recusado(
                            &c.caminho(),
                            "catalogo",
                            format!(
                                "movido para {}, mas o Catálogo não registrou ({e}); um Fast Scan acerta",
                                texto(&m.destino)
                            ),
                            vec![],
                        ));
                    }
                    chaves_ids.push(c.id);
                    movidos_ok += 1;
                    bytes_ok += c.bytes;
                    if res.movidos.len() < MAX_ITENS_NA_RESPOSTA {
                        res.movidos.push(ItemMovido {
                            arquivo: ArquivoId(c.id),
                            volume: VolumeId(c.volume_id),
                            de: c.caminho(),
                            para: texto(&m.destino),
                            bytes: c.bytes,
                            unica_copia: unicas.contains(&c.id),
                        });
                    }
                }
                Err(e) => {
                    res.recusados
                        .push(recusado(&c.caminho(), e.codigo(), e.mensagem(), vec![]))
                }
            }
        }
        res.total_arquivos = movidos_ok;
        res.total_bytes = bytes_ok;
        res.unicas_copias = chaves_ids.iter().filter(|i| unicas.contains(i)).count() as u64;
        self.refazer_grupos_de(&chaves_ids)?;
        Ok(Resposta::Lixeira { resultado: res })
    }

    /// `RestaurarDaLixeira`.
    pub(crate) fn restaurar_da_lixeira(&mut self, alvos: &[Alvo]) -> Result<Resposta, ErroAgente> {
        if alvos.is_empty() {
            return Err(ErroAgente::Invalido("informe ao menos um alvo".into()));
        }
        let mut res = ResultadoLixeira {
            acao: AcaoLixeira::Restaurar,
            simulado: false,
            movidos: Vec::new(),
            recusados: Vec::new(),
            total_arquivos: 0,
            total_bytes: 0,
            unicas_copias: 0,
            marcados_agora: 0,
            destinos: Vec::new(),
            volumes: Vec::new(),
        };
        let mut plano: Vec<Candidato> = Vec::new();
        let mut vistos = HashSet::new();
        {
            let conn = self.catalogo.armazem().conexao();
            for alvo in alvos {
                match resolver_alvo(conn, alvo, true) {
                    Ok(r) if r.arquivos.is_empty() => res.recusados.push(recusado(
                        &r.descricao,
                        "nada_na_lixeira",
                        "nada Na Lixeira neste alvo",
                        vec![],
                    )),
                    Ok(r) => {
                        for c in r.arquivos {
                            if vistos.insert(c.id) {
                                plano.push(c);
                            }
                        }
                    }
                    Err(e) => res.recusados.push(recusado(
                        &descricao_alvo(alvo),
                        e.codigo(),
                        e.to_string(),
                        vec![],
                    )),
                }
            }
        }
        let disp = Arc::clone(&self.dispositivos);
        let mut ids = Vec::new();
        for c in &plano {
            match fs::restaurar(c, &*disp) {
                Ok(m) => {
                    let lugar = NovoLugar {
                        path_raw: m.destino.clone(),
                        pastas: m.pastas,
                        ctime_ns: m.ctime_ns,
                    };
                    if let Err(e) =
                        registrar_restaurado(&mut self.catalogo, c, &lugar, self.autor, agora())
                    {
                        res.recusados.push(recusado(
                            &c.caminho(),
                            "catalogo",
                            format!(
                                "restaurado em {}, mas o Catálogo não registrou ({e}); um Fast Scan acerta",
                                texto(&m.destino)
                            ),
                            vec![],
                        ));
                    }
                    ids.push(c.id);
                    res.total_arquivos += 1;
                    res.total_bytes += c.bytes;
                    if res.movidos.len() < MAX_ITENS_NA_RESPOSTA {
                        res.movidos.push(ItemMovido {
                            arquivo: ArquivoId(c.id),
                            volume: VolumeId(c.volume_id),
                            de: c.caminho(),
                            para: texto(&m.destino),
                            bytes: c.bytes,
                            unica_copia: false,
                        });
                    }
                }
                Err(e) => {
                    res.recusados
                        .push(recusado(&c.caminho(), e.codigo(), e.mensagem(), vec![]))
                }
            }
        }
        let mut destinos = BTreeSet::new();
        let mut volumes = BTreeSet::new();
        for c in &plano {
            destinos.insert(texto(&c.raiz_da_lixeira_absoluta()));
            volumes.insert(c.volume_nome.clone());
        }
        res.destinos = destinos.into_iter().collect();
        res.volumes = volumes.into_iter().collect();
        self.refazer_grupos_de(&ids)?;
        Ok(Resposta::Lixeira { resultado: res })
    }

    /// `ListarLixeira`.
    pub(crate) fn listar_lixeira(
        &self,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let depois = cursor
            .map(|c| {
                c.parse::<i64>()
                    .map_err(|_| ErroAgente::Invalido(format!("cursor inválido: `{c}`")))
            })
            .transpose()?;
        let conn = self.catalogo.armazem().conexao();
        let (itens, prox, t) = lixeira::listar(conn, depois, limite.unwrap_or(50).clamp(1, 1000))?;
        Ok(Resposta::ListaLixeira {
            itens,
            total: t.itens,
            bytes: t.bytes,
            descartados: t.descartados,
            ultimos_descartados: lixeira::ultimos_descartados(conn, 10)?,
            proximo_cursor: prox.map(|p| p.to_string()),
        })
    }

    /// Refaz só os Grupos de Duplicados dos hashes afetados (a cópia Na Lixeira deixa de contar).
    fn refazer_grupos_de(&mut self, ids: &[i64]) -> Result<(), ErroAgente> {
        if ids.is_empty() {
            return Ok(());
        }
        let chaves = lixeira::chaves_de_hash(self.catalogo.armazem().conexao(), ids)?;
        if !chaves.is_empty() {
            duplicados::reconstruir_grupos(
                &mut self.catalogo,
                Escopo::Hashes(&chaves),
                self.autor,
                agora(),
            )?;
        }
        Ok(())
    }

    /// Reconcilia o Catálogo com o que as varreduras viram em `DAG-Lixeira`.
    pub(crate) fn reconciliar_lixeira(&mut self) -> Result<lixeira::ResumoLixeira, ErroAgente> {
        let r = lixeira::reconciliar(&mut self.catalogo, self.autor, agora())?;
        if r.postos_na_lixeira + r.retirados + r.descartados + r.marcados > 0 {
            self.mensagem(format!(
                "[lixeira] {} posto(s) à mão, {} retirado(s), {} descartado(s), {} marcação(ões) criada(s)",
                r.postos_na_lixeira, r.retirados, r.descartados, r.marcados
            ));
        }
        Ok(r)
    }

    /// Acrescenta às raízes a `DAG-Lixeira` dos Volumes envolvidos (se existir e não estiver
    /// dentro de uma raiz): as varreduras sempre visitam a Lixeira.
    pub(crate) fn raizes_com_lixeira(
        &self,
        raizes: Vec<std::path::PathBuf>,
    ) -> Result<Vec<std::path::PathBuf>, ErroAgente> {
        let volumes = self.volumes_do_sistema()?;
        let mut out = raizes.clone();
        for v in &volumes {
            for mp in &v.pontos_de_montagem {
                let mpp = std::path::Path::new(std::ffi::OsStr::from_bytes(mp));
                if !raizes.iter().any(|r| r.starts_with(mpp)) {
                    continue;
                }
                let lixo = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
                    &dag_core::lixeira::raiz_da_lixeira(mp),
                ));
                let eh_pasta = std::fs::symlink_metadata(&lixo).is_ok_and(|m| m.is_dir());
                let coberta = out.iter().any(|r| lixo.starts_with(r));
                if eh_pasta && !coberta {
                    out.push(lixo);
                }
            }
        }
        Ok(out)
    }
}

fn descricao_alvo(a: &Alvo) -> String {
    match a {
        Alvo::Caminho(c) | Alvo::Pasta(c) | Alvo::Volume(c) => c.clone(),
        Alvo::Arquivo(i) => format!("arquivo {i}"),
        Alvo::Grupo(g) => format!("grupo {g}"),
    }
}
