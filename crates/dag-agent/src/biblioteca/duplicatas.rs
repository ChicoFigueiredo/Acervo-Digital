//! Duplicatas exatas: arquivos do Catálogo com o mesmo Hash Completo e tamanho de um Formato
//! homologado. Só índices (`hashes(size_bytes, hash)`), nunca varredura; nenhum OCR; nenhum
//! arquivo é tocado. O descarte continua sendo o fluxo da Fase A com dupla confirmação.

use std::collections::BTreeMap;

use dag_biblio::TipoDuplicidade;
use dag_biblio::descarte::{
    CopiaCandidata, CopiaPreservada, DecisaoDescarte, Iniciativa, PapelCopia, PedidoDescarte,
    VerificacaoCobertura, avaliar,
};
use dag_core::{
    Autor, CopiaDuplicata, DadosBiblioteca, DecisaoDescarteInfo, FormatoHomologadoRef,
    GrupoDuplicata, LivroRef, PapelCopiaInfo, Resposta,
};
use dag_store::biblioteca as store;
use rusqlite::Connection;

use super::{cursor_i64, dados, hex, homologada_ativa, limite_pagina};
use crate::agente::Agente;
use crate::erro::ErroAgente;

pub(super) fn livro_ref(l: &store::LivroHomologado) -> LivroRef {
    LivroRef {
        id: l.id,
        calibre_id: l.calibre_id,
        titulo: l.titulo.clone(),
        autores: l.autores.clone(),
        isbn13: l.isbn13.clone(),
        editora: l.editora.clone(),
        idioma: l.idioma.clone(),
        estante: l.estante.clone(),
        tags: l.tags.clone(),
        caminho: l.caminho.clone(),
    }
}

pub(super) fn formato_ref(f: &store::FormatoHomologado) -> FormatoHomologadoRef {
    FormatoHomologadoRef {
        formato_id: f.id,
        formato: f.formato.clone(),
        caminho: f.caminho.clone(),
        tamanho: f.tamanho,
        hash: f.hash.map(|h| hex(&h)).unwrap_or_default(),
        file_id: f.file_id,
    }
}

pub(super) fn iniciativa_do(autor: Autor) -> Iniciativa {
    // Descarte nunca parte de IA, MCP ou regra automática: o MCP vê a decisão como bloqueada.
    if autor == Autor::Mcp {
        Iniciativa::Automatica
    } else {
        Iniciativa::Humana
    }
}

fn papel_da_copia(
    caminho: &str,
    bibliotecas: &[String],
    pastas_a_classificar: &[String],
) -> (PapelCopia, PapelCopiaInfo) {
    if bibliotecas.iter().any(|b| caminho.starts_with(b.as_str())) {
        return (
            PapelCopia::EspelhoDeBiblioteca,
            PapelCopiaInfo::EspelhoDeBiblioteca,
        );
    }
    let baixo = caminho.to_lowercase();
    if pastas_a_classificar
        .iter()
        .any(|p| baixo.contains(&p.to_lowercase()))
    {
        return (
            PapelCopia::DownloadOuAClassificar,
            PapelCopiaInfo::DownloadOuAClassificar,
        );
    }
    (PapelCopia::Outro, PapelCopiaInfo::Outro)
}

/// Avalia (sem executar) o descarte de uma cópia idêntica a um Formato homologado.
pub(super) fn avaliar_copia(
    conn: &Connection,
    info: &store::InfoArquivo,
    autor: Autor,
    bibliotecas: &[String],
    pastas: &[String],
    verificacao_cobertura: VerificacaoCobertura,
) -> Result<(CopiaDuplicata, bool), ErroAgente> {
    let grupo = store::copias_do_grupo(conn, info.file_id)?;
    let hash_txt = info.hash.map(|h| hex(&h));
    let copias: Vec<CopiaPreservada> = grupo
        .iter()
        .filter(|c| c.file_id != info.file_id)
        .map(|c| CopiaPreservada {
            arquivo_id: c.file_id,
            volume_id: c.volume_id,
            dispositivo_id: c.dispositivo_id.unwrap_or(0),
            hash: hash_txt.clone(),
            tamanho: info.tamanho,
            hash_conferido: c.hash_conferido,
            volume_conectado: c.conectado,
            marcacao_backup: c.marcacao_backup,
            na_lixeira: c.na_lixeira,
        })
        .collect();
    let cobertura: Vec<i64> = grupo
        .iter()
        .filter(|c| c.coberta)
        .map(|c| c.file_id)
        .collect();
    let (papel, papel_info) = papel_da_copia(&info.caminho, bibliotecas, pastas);
    let pedido = PedidoDescarte {
        candidata: CopiaCandidata {
            papel,
            arquivo_id: info.file_id,
            volume_id: info.volume_id,
            dispositivo_id: info.dispositivo_id.unwrap_or(0),
            hash: hash_txt.clone(),
            tamanho: info.tamanho,
        },
        classificacao: TipoDuplicidade::Exata,
        copias,
        iniciativa: iniciativa_do(autor),
        cobertura_do_formato_homologado: cobertura,
        verificacao_cobertura,
    };
    let decisao = match avaliar(&pedido) {
        DecisaoDescarte::Sugerir(r) => DecisaoDescarteInfo {
            sugerir: true,
            copia_preservada: Some(r.copia_preservada),
            razoes: Vec::new(),
            passos_obrigatorios: r.passos_obrigatorios,
        },
        DecisaoDescarte::Bloqueado { razoes } => DecisaoDescarteInfo {
            sugerir: false,
            copia_preservada: None,
            razoes: razoes
                .iter()
                .map(dag_biblio::descarte::RazaoBloqueio::texto)
                .collect(),
            passos_obrigatorios: Vec::new(),
        },
    };
    let ext = info
        .nome
        .rsplit('.')
        .next()
        .map(str::to_uppercase)
        .unwrap_or_default();
    let coberta = grupo.iter().any(|c| c.file_id == info.file_id && c.coberta);
    Ok((
        CopiaDuplicata {
            file_id: info.file_id,
            caminho: info.caminho.clone(),
            volume: info.rotulo_volume.clone().unwrap_or_default(),
            volume_id: info.volume_id,
            dispositivo_id: info.dispositivo_id,
            tamanho: info.tamanho,
            formato: ext,
            hash: hash_txt.unwrap_or_default(),
            papel: papel_info,
            coberta,
            decisao,
        },
        coberta,
    ))
}

pub(super) fn caminhos_das_bibliotecas(conn: &Connection) -> Vec<String> {
    store::listar_bibliotecas(conn, None)
        .map(|v| {
            v.into_iter()
                .map(|b| {
                    let mut c = b.caminho;
                    if !c.ends_with('/') {
                        c.push('/');
                    }
                    c
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Agente {
    /// Uma página (por fatia de Formatos homologados) de duplicatas exatas. Registra os
    /// candidatos e emite alertas idempotentes; não toca em arquivo algum.
    pub(super) fn duplicatas(
        &mut self,
        raiz: Option<&str>,
        limite: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Resposta, ErroAgente> {
        let cfg = self.config_biblioteca();
        let agora = self.agora_biblio();
        let autor = self.autor;
        let conn = self.conexao();
        let Some(bib) = homologada_ativa(conn)? else {
            return Err(ErroAgente::biblio(
                "sem_homologada",
                "registre e sincronize a Biblioteca Homologada primeiro",
            ));
        };
        let fatia = limite_pagina(limite);
        let apos = cursor_i64(cursor).unwrap_or(0);
        let ids: Vec<i64> = {
            let mut st = conn.prepare(
                "SELECT id FROM formatos_homologados WHERE biblioteca_id = ?1 AND hash IS NOT NULL
                 AND id > ?2 ORDER BY id LIMIT ?3",
            )?;
            st.query_map(rusqlite::params![bib.id, apos, fatia as i64], |l| l.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let proximo = if ids.len() == fatia {
            ids.last().copied()
        } else {
            None
        };
        let achados = store::arquivos_iguais_aos_homologados(conn, bib.id, apos, fatia)?;
        let bibliotecas = caminhos_das_bibliotecas(conn);
        let verificacao = super::cobertura::situacao_da_cobertura(conn, &cfg, bib.id, agora);
        let prefixo = raiz.map(str::to_owned);
        let mut grupos: BTreeMap<i64, GrupoDuplicata> = BTreeMap::new();
        let mut emitidos = 0_u64;
        for a in achados {
            if ids.last().is_some_and(|u| a.formato_id > *u) {
                continue;
            }
            if a.tamanho < cfg.tamanho_minimo_duplicata {
                continue;
            }
            let Some(info) = store::info_arquivo(conn, a.file_id)? else {
                continue;
            };
            if !info.ativo || info.na_lixeira || info.hash.is_none() {
                continue;
            }
            if let Some(p) = &prefixo
                && !info.caminho.starts_with(p.as_str())
            {
                continue;
            }
            if store::e_formato_homologado(conn, info.file_id)? {
                continue;
            }
            let (copia, _) = avaliar_copia(
                conn,
                &info,
                autor,
                &bibliotecas,
                &cfg.pastas_a_classificar,
                verificacao.clone(),
            )?;
            store::registrar_candidato(
                conn,
                &store::NovoCandidato {
                    file_id: info.file_id,
                    alvo: store::AlvoDuplicidade::FormatoHomologado(a.formato_id),
                    tipo: TipoDuplicidade::Exata,
                    evidencias_json: serde_json::json!({
                        "hash": copia.hash, "tamanho": info.tamanho,
                    })
                    .to_string(),
                    risco: store::RiscoDuplicidade::Baixo,
                    bytes: info.tamanho,
                },
                agora,
            )?;
            let (_, criado) = store::emitir_alerta(
                conn,
                &store::NovoAlerta {
                    chave: Some(format!("dup:{}:{}", info.file_id, a.formato_id)),
                    tipo: store::TipoAlerta::DuplicataExata,
                    file_id: Some(info.file_id),
                    dados_json: serde_json::json!({
                        "formato_id": a.formato_id, "livro_id": a.livro_id,
                        "caminho": info.caminho, "bytes": info.tamanho,
                    })
                    .to_string(),
                },
                agora,
            )?;
            emitidos += u64::from(criado);
            let g = match grupos.entry(a.formato_id) {
                std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::btree_map::Entry::Vacant(v) => {
                    let Some(livro) = store::obter_livro(conn, a.livro_id)? else {
                        continue;
                    };
                    let fs = store::formatos_do_livro(conn, a.livro_id)?;
                    v.insert(GrupoDuplicata {
                        livro: livro_ref(&livro),
                        formatos_homologados: fs.iter().map(formato_ref).collect(),
                        copias: Vec::new(),
                        bytes_totais: 0,
                        bytes_sugeridos: 0,
                    })
                }
            };
            g.bytes_totais += copia.tamanho;
            if copia.decisao.sugerir {
                g.bytes_sugeridos += copia.tamanho;
            }
            g.copias.push(copia);
        }
        let grupos: Vec<GrupoDuplicata> = grupos.into_values().collect();
        let (t, s) = grupos.iter().fold((0, 0), |(t, s), g| {
            (t + g.bytes_totais, s + g.bytes_sugeridos)
        });
        if emitidos > 0 {
            self.mensagem(format!("alerta: {emitidos} duplicata(s) exata(s) nova(s)"));
        }
        Ok(dados(DadosBiblioteca::Duplicatas {
            grupos,
            bytes_totais_pagina: t,
            bytes_sugeridos_pagina: s,
            alertas_emitidos: emitidos,
            proximo_cursor: proximo.map(|i| i.to_string()),
        }))
    }
}
