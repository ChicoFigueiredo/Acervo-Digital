//! Alertas, resumo e relatório de espaço (só leitura sobre o Catálogo, exceto marcar visto).

use dag_biblio::espaco::{ArquivoEspaco, relatorio};
use dag_core::{
    AlertaInfo, ContagemFila, DadosBiblioteca, EspacoVolumeInfo, GrupoEspacoInfo,
    RelatorioEspacoInfo, Resposta, ResumoBiblioteca,
};
use dag_store::biblioteca as store;

use super::{cursor_i64, dados, hex, limite_pagina};
use crate::agente::Agente;
use crate::erro::ErroAgente;

pub(super) fn alerta_info(a: &store::Alerta) -> AlertaInfo {
    AlertaInfo {
        id: a.id,
        chave: a.chave.clone(),
        tipo: serde_json::to_value(a.tipo)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        file_id: a.file_id,
        dados: serde_json::from_str(&a.dados_json).unwrap_or(serde_json::Value::Null),
        criado_em: a.criado_em,
        visto_em: a.visto_em,
        silenciado: a.silenciado,
    }
}

impl Agente {
    pub(super) fn alertas_biblioteca(
        &mut self,
        nao_vistos: bool,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let conn = self.conexao();
        let antes = cursor_i64(cursor);
        let lim = limite_pagina(limite);
        let pag = if nao_vistos {
            store::listar_nao_vistos(conn, antes, lim)?
        } else {
            store::listar_alertas(conn, antes, lim)?
        };
        Ok(dados(DadosBiblioteca::Alertas {
            itens: pag.itens.iter().map(alerta_info).collect(),
            nao_vistos_total: store::contar_nao_vistos(conn)?,
            proximo_cursor: pag.proximo.map(|i| i.to_string()),
        }))
    }

    pub(super) fn marcar_vistos_biblioteca(&mut self, ids: &[i64]) -> Result<Resposta, ErroAgente> {
        let agora = self.agora_biblio();
        let conn = self.conexao();
        let mut n = 0_u64;
        for id in ids {
            n += u64::from(store::marcar_visto(conn, *id, agora)?);
        }
        Ok(dados(DadosBiblioteca::Vistos { marcados: n }))
    }

    pub(super) fn resumo_biblioteca(&mut self, desde: Option<i64>) -> Result<Resposta, ErroAgente> {
        let ate = self.agora_biblio();
        let de = desde.unwrap_or(ate - 86_400);
        let conn = self.conexao();
        let c = store::contagem_jobs(conn)?;
        let resumo = ResumoBiblioteca {
            de,
            ate,
            alertas_por_tipo: store::resumo_periodo(conn, de, ate)?
                .into_iter()
                .map(|r| {
                    (
                        serde_json::to_value(r.tipo)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        r.total,
                        r.nao_vistos,
                    )
                })
                .collect(),
            livros_novos_na_fila: store::jobs_enfileirados_desde(conn, de)?,
            fila: ContagemFila {
                pendentes: c.pendentes,
                em_andamento: c.em_andamento,
                concluidos: c.concluidos,
                falhos: c.falhos,
                excecoes: c.excecoes,
            },
            revisoes_pendentes: store::contar_sugestoes_pendentes(conn)?,
            candidatos_duplicidade_abertos: store::contar_candidatos_abertos(conn)?,
            bytes_recuperaveis_exatos: store::bytes_recuperaveis_exatos(conn)?,
            alertas_nao_vistos: store::contar_nao_vistos(conn)?,
            jobs_concluidos_periodo: store::contar_jobs_concluidos_periodo(conn, de, ate)?,
            classificados_periodo: store::contar_classificados_periodo(conn, de, ate)?,
            decisoes_periodo: store::contar_decisoes_periodo(conn, de, ate)?,
            duplicatas_periodo: store::contar_candidatos_periodo(conn, de, ate, false)?,
            duplicatas_exatas_periodo: store::contar_candidatos_periodo(conn, de, ate, true)?,
        };
        Ok(dados(DadosBiblioteca::Resumo { resumo }))
    }

    /// Espaço recuperável das cópias idênticas a Formatos homologados, sem dupla contagem
    /// (hard links, reflinks e Membros de Contêiner não somam).
    pub(super) fn relatorio_espaco_biblioteca(
        &mut self,
        volume: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        // Abaixo do tamanho mínimo (1 KiB) não há duplicata: a Homologada real tem formatos de 0
        // byte, que casariam com as ~139 mil entradas vazias do Catálogo.
        let minimo =
            i64::try_from(self.config_biblioteca().tamanho_minimo_duplicata).unwrap_or(i64::MAX);
        let cancelado = || self.controle.estado() == dag_scanner::EstadoControle::Cancelado;
        let guarda = self.guarda_interrupcao();
        let conn = self.conexao();
        let vol: Option<i64> = volume.and_then(|v| {
            v.parse().ok().or_else(|| {
                conn.query_row("SELECT id FROM volumes WHERE rotulo = ?1", [v], |l| {
                    l.get(0)
                })
                .ok()
            })
        });
        // CROSS JOIN fixa a ordem: parte dos Formatos homologados (milhares) e só então busca
        // os hashes por (tamanho, hash). Sem isso o SQLite propaga `tamanho >= ?2` para
        // `hashes.size_bytes` e varre ~20 milhões de hashes com leitura aleatória em `files`
        // (horas no Catálogo real, medido no ensaio do M1).
        let mut st = conn.prepare(
            "SELECT DISTINCT f.id, f.volume_id, f.native_dev, f.native_file_id, f.nlink,
                    f.logical_bytes, f.allocated_bytes, h.hash,
                    EXISTS (SELECT 1 FROM formatos_homologados x WHERE x.file_id = f.id)
             FROM formatos_homologados fh
             CROSS JOIN hashes h INDEXED BY idx_hashes_dup
                   ON h.size_bytes = fh.tamanho AND h.hash = fh.hash
             CROSS JOIN files f ON f.id = h.file_id
             WHERE fh.hash IS NOT NULL AND f.state = 'ativo' AND f.na_lixeira = 0
               AND fh.tamanho >= ?2
               AND (?1 IS NULL OR f.volume_id = ?1)",
        )?;
        let arquivos: Vec<ArquivoEspaco> = st
            .query_map(rusqlite::params![vol, minimo], |l| {
                let dev: Option<i64> = l.get(2)?;
                let ino: Option<i64> = l.get(3)?;
                let hash: Option<Vec<u8>> = l.get(7)?;
                Ok(ArquivoEspaco {
                    volume_id: l.get(1)?,
                    arquivo_id: l.get(0)?,
                    id_fisico: dev.zip(ino).map(|(d, i)| (d as u64, i as u64)),
                    links: u32::try_from(l.get::<_, Option<i64>>(4)?.unwrap_or(1)).unwrap_or(1),
                    tamanho_logico: l.get::<_, i64>(5)?.max(0) as u64,
                    tamanho_alocado: l.get::<_, Option<i64>>(6)?.map(|a| a.max(0) as u64),
                    hash: hash.map(|h| hex(&h)),
                    membro_de_conteiner: false,
                    reflink: false,
                    descartado: false,
                    homologado: l.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| {
                if cancelado() {
                    ErroAgente::biblio("cancelado", "relatório de espaço interrompido; nada mudou")
                } else {
                    e.into()
                }
            })?;
        drop(guarda);
        // Espelhos e cópias antigas da biblioteca Calibre são candidatos a Cobertura, não a
        // descarte: ficam fora da estimativa e só aparecem somados na observação.
        let bibliotecas = super::duplicatas::caminhos_das_bibliotecas(conn);
        let (mut espelhos, mut bytes_espelhos) = (0_u64, 0_u64);
        let mut arquivos = arquivos;
        let mut fora = std::collections::HashSet::new();
        for a in arquivos.iter().filter(|a| !a.homologado) {
            if let Some(info) = store::info_arquivo(conn, a.arquivo_id)?
                && super::duplicatas::e_espelho_de_biblioteca(&info.caminho, &bibliotecas)
            {
                fora.insert(a.arquivo_id);
                espelhos += 1;
                bytes_espelhos += a.tamanho_logico;
            }
        }
        arquivos.retain(|a| !fora.contains(&a.arquivo_id));
        let r = relatorio(&arquivos, 4096);
        let volumes = r
            .volumes
            .iter()
            .map(|v| EspacoVolumeInfo {
                volume_id: v.volume_id,
                caminhos: v.caminhos,
                arquivos_fisicos: v.arquivos_fisicos,
                bytes_logicos: v.bytes_logicos,
                bytes_alocados: v.bytes_alocados,
                estimativa_recuperavel: v.estimativa_recuperavel,
                economia_incerta: v.economia_incerta,
                efetivamente_recuperado: v.efetivamente_recuperado,
                caminhos_hardlink_ignorados: v.caminhos_hardlink_ignorados,
                membros_ignorados: v.membros_ignorados,
            })
            .collect();
        let grupos = r
            .grupos
            .iter()
            .map(|g| GrupoEspacoInfo {
                hash: g.hash.clone(),
                tamanho_logico: g.tamanho_logico,
                mantida: g.mantida,
                redundantes: g
                    .redundantes
                    .iter()
                    .map(|c| {
                        (
                            c.volume_id,
                            c.arquivo_id,
                            c.bytes_alocados,
                            c.incerta,
                            c.motivo_incerteza.clone(),
                        )
                    })
                    .collect(),
                bytes_estimados: g.bytes_estimados,
                bytes_incertos: g.bytes_incertos,
            })
            .collect();
        Ok(dados(DadosBiblioteca::Espaco {
            relatorio: RelatorioEspacoInfo {
                volumes,
                grupos,
                observacao: if espelhos == 0 {
                    "estimativa: nada é apagado; o descarte exige Cobertura e dupla confirmação"
                        .into()
                } else {
                    format!(
                        "estimativa: nada é apagado; o descarte exige Cobertura e dupla \
                         confirmação. Fora da conta: {espelhos} cópia(s) em espelhos ou cópias \
                         antigas de biblioteca Calibre ({} GiB), que são candidatas a \
                         Cobertura, não a descarte.",
                        format!("{:.1}", bytes_espelhos as f64 / f64::from(1u32 << 30))
                            .replace('.', ",")
                    )
                },
            },
        }))
    }
}
