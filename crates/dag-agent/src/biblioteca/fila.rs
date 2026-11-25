//! Enfileiramento barato (só índices) e listagem da fila persistente.

use dag_core::{
    ContagemFila, DadosBiblioteca, EstadoFilaInfo, ItemFilaInfo, MotivoFila, Resposta,
    ResumoEnfileiramento,
};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::{cursor_par, dados, formato_do_nome, limite_pagina};
use crate::agente::Agente;
use crate::erro::ErroAgente;

pub(super) fn motivo_store(m: MotivoFila) -> store::MotivoJob {
    match m {
        MotivoFila::Novo => store::MotivoJob::Novo,
        MotivoFila::Alterado => store::MotivoJob::Alterado,
        MotivoFila::Reprocessar => store::MotivoJob::Reprocessar,
    }
}

fn motivo_info(m: store::MotivoJob) -> MotivoFila {
    match m {
        store::MotivoJob::Novo => MotivoFila::Novo,
        store::MotivoJob::Alterado => MotivoFila::Alterado,
        store::MotivoJob::Reprocessar => MotivoFila::Reprocessar,
    }
}

fn estado_info(e: store::EstadoJob) -> EstadoFilaInfo {
    match e {
        store::EstadoJob::Pendente => EstadoFilaInfo::Pendente,
        store::EstadoJob::EmAndamento => EstadoFilaInfo::EmAndamento,
        store::EstadoJob::Concluido => EstadoFilaInfo::Concluido,
        store::EstadoJob::Falhou => EstadoFilaInfo::Falhou,
        store::EstadoJob::Excecao => EstadoFilaInfo::Excecao,
    }
}

fn estado_store(e: EstadoFilaInfo) -> store::EstadoJob {
    match e {
        EstadoFilaInfo::Pendente => store::EstadoJob::Pendente,
        EstadoFilaInfo::EmAndamento => store::EstadoJob::EmAndamento,
        EstadoFilaInfo::Concluido => store::EstadoJob::Concluido,
        EstadoFilaInfo::Falhou => store::EstadoJob::Falhou,
        EstadoFilaInfo::Excecao => store::EstadoJob::Excecao,
    }
}

/// Prioridade barata: arquivos maiores economizam mais; novos antes dos reprocessados.
pub(super) fn prioridade(tamanho: u64, motivo: MotivoFila) -> i64 {
    let mib = (tamanho >> 20).max(1);
    let bits = i64::from(64 - mib.leading_zeros());
    bits + match motivo {
        MotivoFila::Novo => 2,
        MotivoFila::Alterado => 3,
        MotivoFila::Reprocessar => 0,
    }
}

/// Nome elegível: formato bibliográfico e não é arquivo temporário de download.
pub(super) fn nome_elegivel(nome: &str, ignoradas: &[String]) -> bool {
    let baixo = nome.to_lowercase();
    if ignoradas.iter().any(|e| baixo.ends_with(&e.to_lowercase())) {
        return false;
    }
    formato_do_nome(nome).is_some()
}

/// Faixa `[lo, hi)` de `path_raw` que cobre tudo sob o diretório.
pub(super) fn faixa(raiz: &str) -> (Vec<u8>, Vec<u8>) {
    let mut lo = raiz.as_bytes().to_vec();
    if lo.last() != Some(&b'/') {
        lo.push(b'/');
    }
    let mut hi = lo.clone();
    if let Some(u) = hi.last_mut() {
        *u += 1; // '/' -> '0'
    }
    (lo, hi)
}

/// Enfileira um arquivo se ainda precisa (CA-B11: o já processado com esta versão de regras e
/// sem mudança não volta).
pub(super) fn enfileirar_arquivo(
    conn: &Connection,
    file_id: i64,
    tamanho: u64,
    motivo: MotivoFila,
    versao: &str,
    agora: i64,
) -> Result<Option<bool>, ErroAgente> {
    if motivo == MotivoFila::Novo && store::processado_com_versao(conn, file_id, versao)? {
        return Ok(None);
    }
    let r = store::enfileirar_job(
        conn,
        file_id,
        motivo_store(motivo),
        prioridade(tamanho, motivo),
        Some(versao),
        agora,
    )?;
    Ok(Some(matches!(r, store::Enfileirado::Criado(_))))
}

impl Agente {
    pub(super) fn enfileirar_biblioteca(
        &mut self,
        raizes: &[String],
        motivo: Option<MotivoFila>,
    ) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let motivo = motivo.unwrap_or(MotivoFila::Novo);
        let mut r = ResumoEnfileiramento::default();
        'raizes: for raiz in raizes {
            let (lo, hi) = faixa(raiz);
            let volumes = store::volumes_sob_prefixo(self.conexao(), raiz.as_bytes())?;
            for v in volumes {
                let mut apos: Vec<u8> = Vec::new();
                loop {
                    if !self.seguir_biblioteca() {
                        r.interrompido = true;
                        break 'raizes;
                    }
                    let conn = self.conexao();
                    let lote = store::arquivos_na_faixa(conn, v, &lo, &hi, &apos, 1000)?;
                    if lote.is_empty() {
                        break;
                    }
                    let tx = conn.unchecked_transaction()?;
                    for a in &lote {
                        r.examinados += 1;
                        if !nome_elegivel(&a.nome, &cfg.extensoes_ignoradas)
                            || store::e_formato_homologado(&tx, a.file_id)?
                        {
                            r.ignorados += 1;
                            continue;
                        }
                        match enfileirar_arquivo(
                            &tx,
                            a.file_id,
                            a.tamanho,
                            motivo,
                            &cfg.versao_regras,
                            agora,
                        )? {
                            None => r.ignorados += 1,
                            Some(true) => r.enfileirados += 1,
                            Some(false) => r.ja_na_fila += 1,
                        }
                    }
                    tx.commit()?;
                    if let Some(u) = lote.last() {
                        apos.clone_from(&u.path_raw);
                    }
                    self.mensagem(format!(
                        "biblioteca: {} examinados, {} enfileirados",
                        r.examinados, r.enfileirados
                    ));
                }
            }
        }
        Ok(dados(DadosBiblioteca::Enfileirada { resumo: r }))
    }

    pub(super) fn fila_biblioteca(
        &mut self,
        estado: Option<EstadoFilaInfo>,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let c = store::contagem_jobs(conn)?;
        let apos = cursor_par(cursor).map(|(ordem, id)| store::CursorJob { ordem, id });
        let pag = store::listar_jobs(
            conn,
            estado_store(estado.unwrap_or(EstadoFilaInfo::Pendente)),
            apos,
            limite_pagina(limite),
        )?;
        let itens = pag
            .itens
            .iter()
            .map(|j| ItemFilaInfo {
                id: j.id,
                file_id: j.file_id,
                caminho: store::info_arquivo(conn, j.file_id)
                    .ok()
                    .flatten()
                    .map(|i| i.caminho),
                motivo: motivo_info(j.motivo),
                prioridade: j.prioridade,
                estado: estado_info(j.estado),
                etapa: j.etapa.clone(),
                tentativas: j.tentativas,
                erro: j.erro.clone(),
                enfileirado_em: j.enfileirado_em,
            })
            .collect();
        Ok(dados(DadosBiblioteca::Fila {
            contagem: ContagemFila {
                pendentes: c.pendentes,
                em_andamento: c.em_andamento,
                concluidos: c.concluidos,
                falhos: c.falhos,
                excecoes: c.excecoes,
            },
            itens,
            proximo_cursor: pag.proximo.map(|c| format!("{}:{}", c.ordem, c.id)),
        }))
    }
}
