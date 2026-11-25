//! Vigia de novidades (B-25, B-05, CA-B12): lê os eventos `criado`/`alterado`/`renomeado` que a
//! varredura e o monitor já gravam (nenhum observador novo, nenhum custo extra de E/S),
//! espera o arquivo assentar (debounce), ignora temporários de download, desassocia sugestões
//! quando o conteúdo mudou, avisa na hora se o arquivo já é um Formato homologado (WOW-01) e
//! enfileira só com índices. Fica dormente até a primeira sincronização da Homologada.

use dag_core::{ConfigBiblioteca, MotivoFila};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::fila::{arquivo_elegivel, enfileirar_arquivo};
use crate::agente::Agente;
use crate::erro::ErroAgente;

/// Marca-d'água: id do último evento já tratado.
pub(crate) const CHAVE_MARCA: &str = "biblioteca.watcher.ultimo_evento";

const LOTE: usize = 500;

/// O que uma passada do vigia fez.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumoVigia {
    pub examinados: u64,
    pub enfileirados: u64,
    pub ignorados: u64,
    /// Eventos recentes demais: esperam o próximo ciclo (debounce).
    pub adiados: u64,
    pub alertas: u64,
    pub desassociados: u64,
}

pub(crate) fn vigiar(
    conn: &Connection,
    cfg: &ConfigBiblioteca,
    agora: i64,
) -> Result<ResumoVigia, ErroAgente> {
    let mut r = ResumoVigia::default();
    let Some(marca) =
        dag_store::configuracao::ler(conn, CHAVE_MARCA)?.and_then(|t| t.parse::<i64>().ok())
    else {
        return Ok(r); // dormente até a primeira sincronização
    };
    let debounce = i64::try_from(cfg.debounce_s).unwrap_or(i64::MAX);
    let mut ultimo = marca;
    // Topo dos eventos no começo da passada: se nada que interessa sobrou até ele, a marca-d'água
    // anda até lá. Sem isso, uma enxurrada de eventos `removido` depois do último `criado` seria
    // relida inteira a cada passada do monitor.
    let topo = store::ultimo_evento_id(conn)?;
    let mut completo = true;
    'fora: loop {
        let eventos = store::eventos_de_novidade(conn, ultimo, LOTE)?;
        if eventos.is_empty() {
            break;
        }
        for ev in &eventos {
            if agora - ev.observado_em < debounce {
                // Não avança além do primeiro evento instável: mantém a ordem.
                r.adiados += 1;
                completo = false;
                break 'fora;
            }
            r.examinados += 1;
            ultimo = ev.id;
            let info = match (ev.file_id, ev.volume_id, &ev.path_raw) {
                (Some(f), _, _) => store::info_arquivo(conn, f)?,
                (None, Some(v), Some(p)) => store::info_arquivo_por_caminho(conn, v, p)?,
                _ => None,
            };
            let Some(info) = info else {
                r.ignorados += 1;
                continue;
            };
            if !info.ativo
                || info.na_lixeira
                || !arquivo_elegivel(conn, info.file_id, &info.nome, &cfg.extensoes_ignoradas)?
            {
                r.ignorados += 1;
                continue;
            }
            if store::e_formato_homologado(conn, info.file_id)? {
                r.ignorados += 1;
                continue;
            }
            let mut motivo = if ev.tipo == "alterado" {
                MotivoFila::Alterado
            } else {
                MotivoFila::Novo
            };
            if let Some(h) = info.hash {
                if store::hash_da_extracao(conn, info.file_id)?.is_some_and(|a| a != h) {
                    store::desassociar_conteudo(conn, info.file_id)?;
                    r.desassociados += 1;
                    motivo = MotivoFila::Alterado;
                }
                if info.tamanho >= cfg.tamanho_minimo_duplicata
                    && let Some(a) = store::buscar_por_hash(conn, &h, Some(info.tamanho))?.first()
                {
                    let (_, criado) = store::emitir_alerta(
                        conn,
                        &store::NovoAlerta {
                            chave: Some(format!("ja_tem:{}:{}", info.file_id, a.formato.id)),
                            tipo: store::TipoAlerta::JaTemEsteLivro,
                            file_id: Some(info.file_id),
                            dados_json: serde_json::json!({
                                "livro": a.livro.titulo, "livro_id": a.livro.id,
                                "formato_id": a.formato.id, "caminho": info.caminho,
                            })
                            .to_string(),
                        },
                        agora,
                    )?;
                    r.alertas += u64::from(criado);
                }
            }
            match enfileirar_arquivo(conn, info.file_id, info.tamanho, motivo, cfg, agora)? {
                Some(true) => r.enfileirados += 1,
                _ => r.ignorados += 1,
            }
        }
        if eventos.len() < LOTE {
            break;
        }
    }
    if completo {
        ultimo = ultimo.max(topo);
    }
    if ultimo != marca {
        dag_store::configuracao::gravar(conn, CHAVE_MARCA, &ultimo.to_string())?;
    }
    Ok(r)
}

impl Agente {
    /// Uma passada do vigia (a varredura e o monitor a chamam sozinhos; é pública para testes
    /// e para quem quiser forçar).
    ///
    /// # Errors
    /// Falha do Catálogo.
    pub fn vigiar_biblioteca(&mut self) -> Result<ResumoVigia, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let r = vigiar(self.conexao(), &cfg, agora)?;
        if r.alertas > 0 {
            self.mensagem(format!("alerta: {} livro(s) que você já tem", r.alertas));
        }
        Ok(r)
    }
}

/// Chamada pelo monitor (sem `Agente`): erros não derrubam a reconciliação.
pub(crate) fn vigiar_silencioso(conn: &Connection, agora: i64) {
    let cfg = super::ler_config(conn);
    if let Err(e) = vigiar(conn, &cfg, agora) {
        eprintln!("agente: vigia da biblioteca falhou: {e}");
    }
}
