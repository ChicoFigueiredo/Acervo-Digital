//! Enfileiramento barato (só índices) e listagem da fila persistente.

use dag_biblio::FormatoLivro;
use dag_biblio::prioridade::{ItemFila, PoliticaPrioridade, pontuacao};
use dag_core::{
    ConfigBiblioteca, ContagemFila, DadosBiblioteca, EstadoFilaInfo, ItemFilaInfo, MotivoFila,
    Resposta, ResumoEnfileiramento,
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

/// Aging mínimo: nunca zero, para a fila não deixar ninguém morrer de fome.
const AGING_MINIMO: f64 = 0.01;

/// Prioridade gravada do job (maior sai primeiro): `dag_biblio::prioridade::pontuacao` (economia
/// potencial pelo tamanho e pela suspeita de duplicidade, e novidade) dividida por
/// `aging_por_hora`.
///
/// Por que assim: a migração 0011 fixa a `ordem` como `prioridade * 3600 - enfileirado_em`, uma
/// coluna gerada e indexada em que cada hora de espera vale 1 ponto de `prioridade`. Dividir a
/// pontuação por `aging_por_hora` converte a escala: `aging_por_hora` pontos de pontuação por hora
/// de espera passam a valer exatamente 1 ponto gravado por hora. A ordem resultante é a da
/// pontuação com aging configurável, e o índice `(estado, ordem, id)` continua entregando o
/// próximo job em O(log n). Trocar a coluna (migração) obrigaria a reescrever a tabela e o índice
/// da fila viva; aplicar o fator ao gravar não toca o esquema. Efeito colateral: mudar
/// `aging_por_hora` só vale para o que entrar na fila depois (ou for reenfileirado com prioridade
/// maior).
pub(super) fn prioridade(
    cfg: &ConfigBiblioteca,
    tamanho: u64,
    suspeita_duplicidade: f64,
    motivo: MotivoFila,
) -> i64 {
    let aging = if cfg.aging_por_hora.is_finite() && cfg.aging_por_hora > 0.0 {
        cfg.aging_por_hora.max(AGING_MINIMO)
    } else {
        AGING_MINIMO
    };
    // Sem aging aqui: ele é aplicado pela coluna `ordem` (1 ponto gravado por hora).
    let pol = PoliticaPrioridade {
        aging_por_hora: 0.0,
        ..PoliticaPrioridade::default()
    };
    let item = ItemFila {
        id: 0,
        tamanho,
        suspeita_duplicidade,
        entrou_em: 0,
    };
    // Reprocessar não é novidade: a novidade já decaiu.
    let espera_virtual = match motivo {
        MotivoFila::Reprocessar => pol.meia_vida_novidade_s.saturating_mul(10),
        _ => 0,
    };
    let mut base = pontuacao(&item, espera_virtual, &pol);
    if motivo == MotivoFila::Alterado {
        base += 0.2;
    }
    (base / aging).round() as i64
}

/// Formato bibliográfico do nome, ou `Outro` para compactado (que pode ter livros dentro).
pub(super) fn formato_para_fila(nome: &str) -> Option<FormatoLivro> {
    formato_do_nome(nome).or_else(|| nome_de_compactado(nome).then_some(FormatoLivro::Outro))
}

/// Nome de compactado genérico (zip, rar, 7z, tar...). Quadrinhos (CBZ...) já são formato.
pub(super) fn nome_de_compactado(nome: &str) -> bool {
    let b = nome.to_lowercase();
    [
        ".zip", ".rar", ".7z", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".tar.zst",
    ]
    .iter()
    .any(|e| b.ends_with(e))
}

/// O arquivo entra na fila? Formato bibliográfico, fora os downloads incompletos; compactado só
/// se a etapa de Contêineres (Fase A) já o leu e há livro entre os Membros.
pub(super) fn arquivo_elegivel(
    conn: &Connection,
    file_id: i64,
    nome: &str,
    ignoradas: &[String],
) -> Result<bool, ErroAgente> {
    let baixo = nome.to_lowercase();
    if ignoradas.iter().any(|e| baixo.ends_with(&e.to_lowercase())) {
        return Ok(false);
    }
    if formato_do_nome(nome).is_some() {
        return Ok(true);
    }
    if nome_de_compactado(nome) {
        return Ok(store::conteiner_tem_livros(conn, file_id)?);
    }
    Ok(false)
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
    cfg: &ConfigBiblioteca,
    agora: i64,
) -> Result<Option<bool>, ErroAgente> {
    let versao = cfg.versao_regras.as_str();
    if motivo == MotivoFila::Novo && store::processado_com_versao(conn, file_id, versao)? {
        return Ok(None);
    }
    let suspeita = store::suspeita_de_duplicidade(conn, file_id)?;
    let r = store::enfileirar_job(
        conn,
        file_id,
        motivo_store(motivo),
        prioridade(cfg, tamanho, suspeita, motivo),
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
                        if !arquivo_elegivel(&tx, a.file_id, &a.nome, &cfg.extensoes_ignoradas)?
                            || store::e_formato_homologado(&tx, a.file_id)?
                        {
                            r.ignorados += 1;
                            continue;
                        }
                        match enfileirar_arquivo(&tx, a.file_id, a.tamanho, motivo, &cfg, agora)? {
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

    pub(super) fn priorizar_biblioteca(
        &mut self,
        criterio: dag_core::CriterioPrioridade,
    ) -> Result<Resposta, ErroAgente> {
        let n = match criterio {
            dag_core::CriterioPrioridade::Tamanho => {
                store::priorizar_pendentes_por_tamanho(self.conexao())?
            }
        };
        Ok(Resposta::Ok {
            mensagem: Some(format!(
                "{n} job(s) pendente(s) reordenado(s) por tamanho, do maior para o menor"
            )),
        })
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
