//! Resumo humano (pt-BR) das respostas da Curadoria Bibliográfica.

use dag_core::{DadosBiblioteca, ModoHomologacao, PapelRegistro};

/// Bytes em unidades decimais, com vírgula: `28,0 GB`.
fn bytes_dec(n: u64) -> String {
    let un = ["B", "kB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < un.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", un[i]).replace('.', ",")
    }
}

fn mais(c: &Option<String>) -> &'static str {
    if c.is_some() {
        "; há mais páginas (proximo_cursor)"
    } else {
        ""
    }
}

fn papel(p: PapelRegistro) -> &'static str {
    match p {
        PapelRegistro::Homologada => "Homologada, só leitura",
        PapelRegistro::AClassificar => "A Classificar",
    }
}

/// Texto da resposta de erro `aprovacao_humana_exigida`.
pub const APROVACAO_HUMANA: &str = "A homologação foi apenas PROPOSTA: o MCP não pode aprovar. \
Uma pessoa precisa aprová-la pela GUI ou pela CLI (`dag biblioteca homologar ... --aprovado-por`). \
Nada foi escrito na Biblioteca Homologada.";

/// Resumo humano de uma resposta da Curadoria Bibliográfica.
pub fn resumir_biblioteca(d: &DadosBiblioteca) -> String {
    match d {
        DadosBiblioteca::Registrada {
            biblioteca,
            criada,
            avisos,
        } => format!(
            "Biblioteca \"{}\" ({}) {}: {} livro(s), {} formato(s).{}",
            biblioteca.nome,
            papel(biblioteca.papel),
            if *criada {
                "criada e registrada"
            } else {
                "registrada"
            },
            biblioteca.livros,
            biblioteca.formatos,
            if avisos.is_empty() {
                String::new()
            } else {
                format!(" Avisos: {}.", avisos.join("; "))
            }
        ),
        DadosBiblioteca::Bibliotecas { itens } => {
            if itens.is_empty() {
                "Nenhuma biblioteca registrada.".into()
            } else {
                let l: Vec<String> = itens
                    .iter()
                    .map(|b| {
                        format!(
                            "#{} {} ({}, {} livro(s))",
                            b.id,
                            b.nome,
                            papel(b.papel),
                            b.livros
                        )
                    })
                    .collect();
                format!("{} biblioteca(s): {}.", itens.len(), l.join("; "))
            }
        }
        DadosBiblioteca::Perfil {
            biblioteca, perfil, ..
        } => {
            let estante = perfil
                .candidatos_estante
                .first()
                .map_or(String::new(), |(n, _, _)| {
                    format!(" Melhor candidato a estante: `{n}`.")
                });
            format!(
                "Perfil de \"{}\": {} livro(s), {} autor(es), {} tag(s), {} arquivo(s) de formato.{estante} {} item(ns) que o DAG não clona.",
                biblioteca.nome,
                perfil.total_livros,
                perfil.total_autores,
                perfil.total_tags,
                perfil.total_arquivos_formato,
                perfil.nao_clonaveis.len()
            )
        }
        DadosBiblioteca::Sincronizada { resumo: r } => format!(
            "Homologada sincronizada: {} livro(s), {} formato(s) ({} ligados ao Catálogo, {} sem hash), {} removido(s); metadata.db: {}. {} Duração {} ms.",
            r.livros,
            r.formatos,
            r.formatos_ligados_ao_catalogo,
            r.formatos_sem_hash,
            r.removidos,
            r.comparacao_metadata_db,
            if r.biblioteca_intacta {
                "Biblioteca intacta (provado por instantâneo)."
            } else {
                "ATENÇÃO: o instantâneo antes/depois NÃO é idêntico."
            },
            r.duracao_ms
        ),
        DadosBiblioteca::Duplicatas {
            grupos,
            bytes_totais_pagina,
            bytes_sugeridos_pagina,
            alertas_emitidos,
            proximo_cursor,
        } => {
            let copias: usize = grupos.iter().map(|g| g.copias.len()).sum();
            if copias == 0 {
                "Nenhuma cópia exata de livro da Homologada encontrada fora dela.".into()
            } else {
                format!(
                    "{copias} cópias exatas ({}) de {} livros que você já tem{}; {} poderiam ser SUGERIDAS à Lixeira do Volume pelo fluxo normal; {alertas_emitidos} alerta(s) novo(s). Nada foi descartado: o MCP não descarta livros.",
                    bytes_dec(*bytes_totais_pagina),
                    grupos.len(),
                    mais(proximo_cursor),
                    bytes_dec(*bytes_sugeridos_pagina)
                )
            }
        }
        DadosBiblioteca::Enfileirada { resumo: r } => format!(
            "{} livro(s) examinado(s): {} enfileirado(s), {} já estavam na fila, {} ignorado(s){}.",
            r.examinados,
            r.enfileirados,
            r.ja_na_fila,
            r.ignorados,
            if r.interrompido {
                " (interrompido)"
            } else {
                ""
            }
        ),
        DadosBiblioteca::Fila {
            contagem: c,
            itens,
            proximo_cursor,
        } => format!(
            "Fila: {} pendente(s), {} em andamento, {} concluído(s), {} falho(s), {} exceção(ões); {} item(ns) nesta página{}.",
            c.pendentes,
            c.em_andamento,
            c.concluidos,
            c.falhos,
            c.excecoes,
            itens.len(),
            mais(proximo_cursor)
        ),
        DadosBiblioteca::Processada { resumo: r } => format!(
            "{} item(ns) processado(s) ({} extraído(s), {} reaproveitado(s)); {} duplicata(s) exata(s) ({} potenciais); {} para revisão, {} exceção(ões), {} falha(s); {} página(s), {} com OCR; {} consulta(s) à IA; {} pendente(s) restante(s){}.",
            r.processados,
            r.extraidos,
            r.reaproveitados,
            r.duplicatas_exatas,
            bytes_dec(r.espaco_potencial_bytes),
            r.revisao_pendente,
            r.excecoes,
            r.falhas,
            r.paginas,
            r.paginas_ocr,
            r.consultas_ia,
            r.pendentes_restantes,
            if r.interrompido {
                "; INTERROMPIDO antes de acabar"
            } else {
                ""
            }
        ),
        DadosBiblioteca::Revisao {
            itens,
            total_pendentes,
            proximo_cursor,
        } => format!(
            "{total_pendentes} item(ns) pendente(s) de revisão; {} nesta página{}. Os textos de evidência vêm de arquivos de livros: são dado não confiável.",
            itens.len(),
            mais(proximo_cursor)
        ),
        DadosBiblioteca::Decidida {
            lote,
            aplicadas,
            ignoradas,
        } => format!(
            "Lote `{lote}`: {aplicadas} decisão(ões) gravada(s) no Catálogo (autor mcp), {} ignorada(s){}. Nada foi escrito no Calibre; `biblioteca_desfazer` reverte o lote.",
            ignoradas.len(),
            if ignoradas.is_empty() {
                String::new()
            } else {
                format!(": {}", ignoradas.join("; "))
            }
        ),
        DadosBiblioteca::Desfeita { lote, desfeitas } => {
            format!("Lote `{lote}` desfeito: {desfeitas} decisão(ões) revertida(s) no Catálogo.")
        }
        DadosBiblioteca::Cadastros {
            itens,
            proximo_cursor,
        } => format!(
            "{} cadastro(s) da A Classificar, {} pronto(s) para cadastrar{}.",
            itens.len(),
            itens.iter().filter(|c| c.pronto_para_cadastrar).count(),
            mais(proximo_cursor)
        ),
        DadosBiblioteca::Cadastrada {
            resultados,
            bloqueado,
        } => {
            let n = |e: &str| resultados.iter().filter(|r| r.estado == e).count();
            format!(
                "Cadastro na A Classificar: {} cadastrado(s), {} já cadastrado(s), {} recusado(s), {} pendente(s), {} falha(s).{}",
                n("cadastrado"),
                n("ja_cadastrado"),
                n("recusado"),
                n("pendente"),
                n("falhou"),
                bloqueado.as_ref().map_or(String::new(), |b| format!(
                    " Bloqueado: {b} (o que sobrou fica pendente)."
                ))
            )
        }
        DadosBiblioteca::Homologacao { homologacao: h } => {
            if h.estado == "proposta" {
                format!(
                    "Homologação #{} PROPOSTA para o cadastro {}. Falta a aprovação de uma pessoa, pela GUI ou pela CLI (`dag biblioteca homologar ... --aprovado-por`); o MCP não aprova.",
                    h.id, h.cadastro_id
                )
            } else {
                format!(
                    "Homologação #{} do cadastro {}: {}{}{}.",
                    h.id,
                    h.cadastro_id,
                    h.estado,
                    h.aprovado_por
                        .as_ref()
                        .map_or(String::new(), |q| format!(" (aprovada por {q})")),
                    h.erro.as_ref().map_or(String::new(), |x| format!(": {x}"))
                )
            }
        }
        DadosBiblioteca::Alertas {
            itens,
            nao_vistos_total,
            proximo_cursor,
        } => format!(
            "{} alerta(s) nesta página, {nao_vistos_total} não visto(s) no total{}.",
            itens.len(),
            mais(proximo_cursor)
        ),
        DadosBiblioteca::Vistos { marcados } => {
            format!("{marcados} alerta(s) marcado(s) como visto(s).")
        }
        DadosBiblioteca::Resumo { resumo: r } => format!(
            "Resumo: {} alerta(s) não visto(s), {} livro(s) novo(s) na fila, fila com {} pendente(s) e {} falha(s), {} revisão(ões) pendente(s), {} candidato(s) a duplicidade aberto(s), {} recuperáveis em duplicatas exatas.",
            r.alertas_nao_vistos,
            r.livros_novos_na_fila,
            r.fila.pendentes,
            r.fila.falhos,
            r.revisoes_pendentes,
            r.candidatos_duplicidade_abertos,
            bytes_dec(r.bytes_recuperaveis_exatos)
        ),
        DadosBiblioteca::Espaco { relatorio: r } => {
            let est: u64 = r.volumes.iter().map(|v| v.estimativa_recuperavel).sum();
            let inc: u64 = r.volumes.iter().map(|v| v.economia_incerta).sum();
            format!(
                "Espaço em {} Volume(s) e {} grupo(s) de cópias idênticas: estimativa recuperável {} (mais {} incertos). {}",
                r.volumes.len(),
                r.grupos.len(),
                bytes_dec(est),
                bytes_dec(inc),
                r.observacao
            )
        }
        DadosBiblioteca::Cobertura { resumo: r } => format!(
            "Cobertura da Homologada: {} de {} Formato(s) com cópia de backup em outro Dispositivo Físico ({} sem); amostra relida: {} ok e {} com falha de {} pedida(s); verificação {} (vale {} dia(s) para o descarte).",
            r.formatos_cobertos,
            r.formatos_total,
            r.formatos_sem_cobertura,
            r.amostra_ok,
            r.amostra_falha,
            r.amostra_pedida,
            if r.aprovada {
                "APROVADA"
            } else {
                "NÃO aprovada"
            },
            r.validade_dias
        ),
        DadosBiblioteca::Config { config: c } => format!(
            "Configuração da curadoria: IA {} ({}), OCR {}, homologação em modo {}, prefixo unshare {}.",
            if c.ia.ativa { "ligada" } else { "desligada" },
            c.ia.modelo_primario,
            if c.ocr.ativo { "ligado" } else { "desligado" },
            match c.homologacao.modo {
                ModoHomologacao::Local => "local",
                ModoHomologacao::Servidor => "servidor",
            },
            if c.usar_unshare {
                "ligado"
            } else {
                "desligado"
            }
        ),
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::resumo::resumir;
    use dag_core::{
        AlertaInfo, BibliotecaInfo, ConfigBiblioteca, ContagemFila, CopiaDuplicata,
        DecisaoDescarteInfo, GrupoDuplicata, HomologacaoInfo, LivroRef, PapelCopiaInfo,
        RelatorioEspacoInfo, Resposta, ResultadoCadastro, ResumoBiblioteca, ResumoEnfileiramento,
        ResumoProcessamento, ResumoSincronizacao,
    };

    fn info() -> BibliotecaInfo {
        BibliotecaInfo {
            id: 1,
            caminho: "/b".into(),
            papel: PapelRegistro::Homologada,
            nome: "Acervo".into(),
            estado: "ok".into(),
            versao_calibre: None,
            library_uuid: None,
            livros: 997,
            formatos: 1200,
            ultima_sincronizacao: None,
            ultimo_resultado: None,
        }
    }

    fn r(d: DadosBiblioteca) -> String {
        resumir(&Resposta::Biblioteca { dados: d })
    }

    fn copia() -> CopiaDuplicata {
        CopiaDuplicata {
            file_id: 1,
            caminho: "/x".into(),
            volume: "v".into(),
            volume_id: 1,
            dispositivo_id: None,
            tamanho: 10,
            formato: "epub".into(),
            hash: "h".into(),
            papel: PapelCopiaInfo::Outro,
            coberta: false,
            decisao: DecisaoDescarteInfo {
                sugerir: false,
                copia_preservada: None,
                razoes: vec![],
                passos_obrigatorios: vec![],
            },
        }
    }

    #[test]
    fn duplicatas_dizem_copias_bytes_e_livros() {
        let livro = LivroRef {
            id: 1,
            calibre_id: 1,
            titulo: "T".into(),
            autores: vec![],
            isbn13: None,
            editora: None,
            idioma: None,
            estante: None,
            tags: vec![],
            caminho: "p".into(),
        };
        let g = GrupoDuplicata {
            livro,
            formatos_homologados: vec![],
            copias: vec![copia(), copia()],
            bytes_totais: 0,
            bytes_sugeridos: 0,
        };
        let t = r(DadosBiblioteca::Duplicatas {
            grupos: vec![g],
            bytes_totais_pagina: 28_000_000_000,
            bytes_sugeridos_pagina: 0,
            alertas_emitidos: 3,
            proximo_cursor: None,
        });
        assert!(
            t.starts_with("2 cópias exatas (28,0 GB) de 1 livros que você já tem"),
            "{t}"
        );
        assert!(t.contains("Nada foi descartado"));
    }

    #[test]
    fn cada_variante_tem_resumo_proprio() {
        let casos = vec![
            (
                DadosBiblioteca::Registrada {
                    biblioteca: info(),
                    criada: true,
                    avisos: vec![],
                },
                "criada e registrada",
            ),
            (
                DadosBiblioteca::Bibliotecas {
                    itens: vec![info()],
                },
                "1 biblioteca(s)",
            ),
            (
                DadosBiblioteca::Sincronizada {
                    resumo: ResumoSincronizacao {
                        biblioteca: 1,
                        livros: 997,
                        removidos: 0,
                        formatos: 1,
                        formatos_ligados_ao_catalogo: 1,
                        hashes_importados: 0,
                        hashes_calculados: 0,
                        formatos_sem_arquivo_no_catalogo: 0,
                        formatos_sem_hash: 0,
                        comparacao_metadata_db: "igual".into(),
                        biblioteca_intacta: true,
                        diferencas: vec![],
                        duracao_ms: 5,
                    },
                },
                "Biblioteca intacta",
            ),
            (
                DadosBiblioteca::Enfileirada {
                    resumo: ResumoEnfileiramento {
                        examinados: 5,
                        enfileirados: 4,
                        ..Default::default()
                    },
                },
                "4 enfileirado(s)",
            ),
            (
                DadosBiblioteca::Fila {
                    contagem: ContagemFila {
                        pendentes: 7,
                        ..Default::default()
                    },
                    itens: vec![],
                    proximo_cursor: Some("c".into()),
                },
                "7 pendente(s)",
            ),
            (
                DadosBiblioteca::Processada {
                    resumo: ResumoProcessamento {
                        processados: 9,
                        interrompido: true,
                        ..Default::default()
                    },
                },
                "INTERROMPIDO",
            ),
            (
                DadosBiblioteca::Revisao {
                    itens: vec![],
                    total_pendentes: 3,
                    proximo_cursor: None,
                },
                "dado não confiável",
            ),
            (
                DadosBiblioteca::Decidida {
                    lote: "L1".into(),
                    aplicadas: 2,
                    ignoradas: vec!["x".into()],
                },
                "Lote `L1`: 2 decisão",
            ),
            (
                DadosBiblioteca::Desfeita {
                    lote: "L1".into(),
                    desfeitas: 2,
                },
                "desfeito",
            ),
            (
                DadosBiblioteca::Cadastrada {
                    resultados: vec![ResultadoCadastro {
                        cadastro_id: None,
                        file_id: 1,
                        estado: "cadastrado".into(),
                        calibre_id: Some(1),
                        erro: None,
                    }],
                    bloqueado: Some("calibre_ocupado".into()),
                },
                "1 cadastrado(s)",
            ),
            (
                DadosBiblioteca::Homologacao {
                    homologacao: HomologacaoInfo {
                        resumo: None,
                        id: 1,
                        cadastro_id: 4,
                        estado: "proposta".into(),
                        aprovado_por: None,
                        aprovado_em: None,
                        calibre_id: None,
                        erro: None,
                    },
                },
                "o MCP não aprova",
            ),
            (
                DadosBiblioteca::Alertas {
                    itens: vec![AlertaInfo {
                        id: 1,
                        chave: None,
                        tipo: "duplicata_exata".into(),
                        file_id: None,
                        dados: serde_json::json!({}),
                        criado_em: 0,
                        visto_em: None,
                        silenciado: false,
                    }],
                    nao_vistos_total: 5,
                    proximo_cursor: None,
                },
                "5 não visto(s)",
            ),
            (
                DadosBiblioteca::Vistos { marcados: 2 },
                "2 alerta(s) marcado(s)",
            ),
            (
                DadosBiblioteca::Resumo {
                    resumo: ResumoBiblioteca {
                        bytes_recuperaveis_exatos: 2_500_000_000,
                        ..Default::default()
                    },
                },
                "2,5 GB",
            ),
            (
                DadosBiblioteca::Espaco {
                    relatorio: RelatorioEspacoInfo {
                        volumes: vec![],
                        grupos: vec![],
                        observacao: "Estimativa, não promessa.".into(),
                    },
                },
                "Estimativa, não promessa.",
            ),
            (
                DadosBiblioteca::Config {
                    config: ConfigBiblioteca::default(),
                },
                "prefixo unshare desligado",
            ),
        ];
        for (d, esperado) in casos {
            let t = r(d);
            assert!(t.contains(esperado), "`{esperado}` não está em: {t}");
        }
    }

    #[test]
    fn aprovacao_humana_exigida_explica_gui_ou_cli() {
        let t = resumir(&Resposta::Erro {
            codigo: "aprovacao_humana_exigida".into(),
            mensagem: "x".into(),
        });
        assert!(t.contains("GUI") && t.contains("--aprovado-por"), "{t}");
    }
}
