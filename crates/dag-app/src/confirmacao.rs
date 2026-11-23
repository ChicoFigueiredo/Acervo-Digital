//! Planejamento das ações de marcação do Duplicados: o que será afetado e quais avisos mostrar
//! na confirmação (inclusive ⚠ Única Cópia, A-29). Puro: só olha o [`DetalheGrupo`] e monta o
//! [`Comando`]; nada é gravado aqui e o DAG nunca apaga, move ou altera arquivos.

use dag_core::{Alvo, ArquivoId, Comando, CopiaGrupo, DetalheGrupo, GrupoId, TipoMarcacao};

/// Ação escolhida pelo usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcaoMarcacao {
    MarcarBackup,
    MarcarDelecao,
    Desmarcar,
    DefinirPrincipal,
}

impl AcaoMarcacao {
    #[must_use]
    pub fn rotulo(self) -> &'static str {
        match self {
            Self::MarcarBackup => "Marcar backup",
            Self::MarcarDelecao => "Marcar para deleção",
            Self::Desmarcar => "Desmarcar",
            Self::DefinirPrincipal => "Definir Cópia Principal",
        }
    }
}

/// A que a ação se aplica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlvoAcao {
    /// Uma cópia (arquivo físico) do grupo.
    Copia(ArquivoId),
    /// O grupo inteiro: Backup = todas as cópias; Deleção = todas menos a Cópia Principal.
    Grupo,
}

/// Gravidade de um aviso.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gravidade {
    Info,
    Atencao,
}

/// Um aviso da confirmação.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aviso {
    pub gravidade: Gravidade,
    pub texto: String,
}

/// O que a confirmação mostra e o que será enviado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plano {
    pub acao: AcaoMarcacao,
    pub alvo: AlvoAcao,
    pub grupo: GrupoId,
    /// Cópias afetadas (id e caminho), em ordem.
    pub afetadas: Vec<(ArquivoId, String)>,
    pub avisos: Vec<Aviso>,
    /// Alerta de Única Cópia: nenhuma cópia sem Marcação para Deleção restará.
    pub unica_copia: bool,
}

const TEXTO_SO_CATALOGO: &str =
    "Só o Catálogo é alterado: nenhum arquivo é apagado, movido ou modificado.";

fn eh_delecao_ativa(c: &CopiaGrupo) -> bool {
    c.marcacao.as_ref().is_some_and(|m| {
        m.tipo == TipoMarcacao::Delecao && m.estado == dag_core::EstadoMarcacao::Ativa
    })
}

/// Monta o plano de `acao` sobre `alvo` no grupo `detalhe`.
///
/// # Errors
/// Texto para o usuário quando a combinação não faz sentido (cópia fora do grupo, Cópia
/// Principal sobre o grupo inteiro, nada a marcar).
pub fn planejar(
    detalhe: &DetalheGrupo,
    acao: AcaoMarcacao,
    alvo: AlvoAcao,
) -> Result<Plano, String> {
    let copias = &detalhe.copias;
    let grupo = detalhe.grupo.id;
    let afetadas: Vec<&CopiaGrupo> = match (acao, alvo) {
        (AcaoMarcacao::DefinirPrincipal, AlvoAcao::Grupo) => {
            return Err("Escolha uma cópia para ser a Cópia Principal.".into());
        }
        (_, AlvoAcao::Copia(id)) => vec![
            copias
                .iter()
                .find(|c| c.arquivo == id)
                .ok_or("A cópia selecionada não pertence a este grupo.")?,
        ],
        (AcaoMarcacao::MarcarDelecao, AlvoAcao::Grupo) => {
            copias.iter().filter(|c| !c.principal).collect()
        }
        (_, AlvoAcao::Grupo) => copias.iter().collect(),
    };
    if afetadas.is_empty() {
        return Err("Não há cópias a marcar neste grupo.".into());
    }
    let mut avisos = vec![Aviso {
        gravidade: Gravidade::Info,
        texto: TEXTO_SO_CATALOGO.into(),
    }];
    let ids: Vec<ArquivoId> = afetadas.iter().map(|c| c.arquivo).collect();
    let mut unica_copia = false;
    if acao == AcaoMarcacao::MarcarDelecao {
        let restantes: Vec<&CopiaGrupo> = copias
            .iter()
            .filter(|c| !ids.contains(&c.arquivo) && !eh_delecao_ativa(c))
            .collect();
        if restantes.is_empty() {
            unica_copia = true;
            avisos.push(Aviso {
                gravidade: Gravidade::Atencao,
                texto: "⚠ Única Cópia: depois desta marcação nenhuma cópia deste conteúdo ficará \
                        sem Marcação para Deleção; o conteúdo existe só nas cópias marcadas."
                    .into(),
            });
        } else if restantes.iter().all(|c| !c.conectado) {
            avisos.push(Aviso {
                gravidade: Gravidade::Atencao,
                texto: "As cópias que restam estão em Volumes desconectados; não dá para \
                        conferir que continuam acessíveis."
                    .into(),
            });
        }
        if afetadas.iter().any(|c| c.principal) {
            avisos.push(Aviso {
                gravidade: Gravidade::Atencao,
                texto: "Entre as cópias há a Cópia Principal do grupo.".into(),
            });
        }
    }
    if matches!(
        acao,
        AcaoMarcacao::MarcarBackup | AcaoMarcacao::MarcarDelecao
    ) {
        let tipo = if acao == AcaoMarcacao::MarcarBackup {
            TipoMarcacao::Backup
        } else {
            TipoMarcacao::Delecao
        };
        let ja = afetadas
            .iter()
            .filter(|c| c.marcacao.as_ref().is_some_and(|m| m.tipo == tipo))
            .count();
        if ja > 0 {
            avisos.push(Aviso {
                gravidade: Gravidade::Info,
                texto: format!("{ja} cópia(s) já têm esta marcação e ficam como estão."),
            });
        }
        let contrarias = afetadas
            .iter()
            .filter(|c| c.marcacao.as_ref().is_some_and(|m| m.tipo != tipo))
            .count();
        if contrarias > 0 {
            avisos.push(Aviso {
                gravidade: Gravidade::Atencao,
                texto: format!(
                    "{contrarias} cópia(s) têm a marcação oposta; uma cópia nunca tem as duas, \
                     então o Catálogo pode recusar. Desmarque antes."
                ),
            });
        }
    }
    if acao == AcaoMarcacao::MarcarBackup && afetadas.iter().all(|c| c.cobertura.coberto) {
        avisos.push(Aviso {
            gravidade: Gravidade::Info,
            texto: "Estas cópias já têm Cobertura.".into(),
        });
    }
    Ok(Plano {
        acao,
        alvo,
        grupo,
        afetadas: afetadas
            .iter()
            .map(|c| (c.arquivo, c.caminho.clone()))
            .collect(),
        avisos,
        unica_copia,
    })
}

/// Comando que executa o plano.
#[must_use]
pub fn comando(plano: &Plano) -> Comando {
    let alvo = match plano.alvo {
        AlvoAcao::Copia(id) => Alvo::Arquivo(id),
        AlvoAcao::Grupo => Alvo::Grupo(plano.grupo),
    };
    match plano.acao {
        AcaoMarcacao::MarcarBackup => Comando::MarcarBackup { alvo },
        AcaoMarcacao::MarcarDelecao => Comando::MarcarDelecao { alvo },
        AcaoMarcacao::Desmarcar => Comando::Desmarcar { alvo },
        AcaoMarcacao::DefinirPrincipal => Comando::DefinirCopiaPrincipal {
            grupo: plano.grupo,
            arquivo: Some(alvo),
        },
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::{
        ClasseRecuperavel, CoberturaArquivo, EscopoMarcacao, EstadoMarcacao, GrupoDuplicados,
        HashCompleto, MarcacaoDaCopia, MarcacaoId, ModoPrincipal, VolumeId,
    };

    fn copia(id: i64, principal: bool, conectado: bool, marca: Option<TipoMarcacao>) -> CopiaGrupo {
        CopiaGrupo {
            arquivo: ArquivoId(id),
            volume: VolumeId(1),
            volume_rotulo: Some("v".into()),
            dispositivo: None,
            caminho: format!("/c/{id}"),
            conectado,
            visto_em: None,
            principal,
            outros_caminhos: 0,
            marcacao: marca.map(|tipo| MarcacaoDaCopia {
                id: MarcacaoId(id),
                tipo,
                estado: EstadoMarcacao::Ativa,
                escopo: EscopoMarcacao::ManualArquivo,
            }),
            cobertura: CoberturaArquivo {
                coberto: false,
                nao_verificada_desde: None,
            },
            recuperavel: ClasseRecuperavel::Nao,
        }
    }

    fn grupo(copias: Vec<CopiaGrupo>) -> DetalheGrupo {
        DetalheGrupo {
            grupo: GrupoDuplicados {
                id: GrupoId(9),
                hash: HashCompleto([1; 32]),
                tamanho: 10,
                copias: copias.len() as u32,
                copias_offline: 0,
                copia_principal: Some(ArquivoId(1)),
                modo_principal: ModoPrincipal::Auto,
                bytes_marcados_delecao: 0,
                bytes_redundantes_estimados: 0,
                espaco_recuperavel_bytes: 0,
            },
            copias,
        }
    }

    fn tem_unica(p: &Plano) -> bool {
        p.avisos.iter().any(|a| a.texto.contains("⚠ Única Cópia"))
    }

    #[test]
    fn deletar_o_grupo_preserva_a_principal_e_nao_alerta() {
        let g = grupo(vec![
            copia(1, true, true, None),
            copia(2, false, true, None),
        ]);
        let p = planejar(&g, AcaoMarcacao::MarcarDelecao, AlvoAcao::Grupo).unwrap();
        assert_eq!(p.afetadas, [(ArquivoId(2), "/c/2".to_string())]);
        assert!(!p.unica_copia && !tem_unica(&p));
        assert_eq!(
            comando(&p),
            Comando::MarcarDelecao {
                alvo: Alvo::Grupo(GrupoId(9))
            }
        );
    }

    #[test]
    fn deletar_a_ultima_copia_sem_marcacao_mostra_alerta_unica_copia() {
        let g = grupo(vec![
            copia(1, true, true, None),
            copia(2, false, true, Some(TipoMarcacao::Delecao)),
        ]);
        let p = planejar(
            &g,
            AcaoMarcacao::MarcarDelecao,
            AlvoAcao::Copia(ArquivoId(1)),
        )
        .unwrap();
        assert!(p.unica_copia && tem_unica(&p));
        assert!(
            p.avisos
                .iter()
                .any(|a| a.gravidade == Gravidade::Atencao && a.texto.contains("Principal"))
        );
        // Marcar backup nunca dispara o alerta.
        let b = planejar(
            &g,
            AcaoMarcacao::MarcarBackup,
            AlvoAcao::Copia(ArquivoId(1)),
        )
        .unwrap();
        assert!(!b.unica_copia);
    }

    #[test]
    fn restantes_offline_geram_aviso_sem_ser_unica_copia() {
        let g = grupo(vec![
            copia(1, true, false, None),
            copia(2, false, true, None),
        ]);
        let p = planejar(
            &g,
            AcaoMarcacao::MarcarDelecao,
            AlvoAcao::Copia(ArquivoId(2)),
        )
        .unwrap();
        assert!(!p.unica_copia);
        assert!(p.avisos.iter().any(|a| a.texto.contains("desconectados")));
    }

    #[test]
    fn todo_plano_diz_que_so_o_catalogo_muda() {
        let g = grupo(vec![
            copia(1, true, true, None),
            copia(2, false, true, None),
        ]);
        for acao in [
            AcaoMarcacao::MarcarBackup,
            AcaoMarcacao::MarcarDelecao,
            AcaoMarcacao::Desmarcar,
        ] {
            let p = planejar(&g, acao, AlvoAcao::Grupo).unwrap();
            assert!(p.avisos[0].texto.contains("nenhum arquivo é apagado"));
        }
    }

    #[test]
    fn backup_do_grupo_marca_todas_e_comando_correto() {
        let g = grupo(vec![
            copia(1, true, true, None),
            copia(2, false, true, None),
        ]);
        let p = planejar(&g, AcaoMarcacao::MarcarBackup, AlvoAcao::Grupo).unwrap();
        assert_eq!(p.afetadas.len(), 2);
        assert!(matches!(
            comando(&p),
            Comando::MarcarBackup {
                alvo: Alvo::Grupo(GrupoId(9))
            }
        ));
        let d = planejar(&g, AcaoMarcacao::Desmarcar, AlvoAcao::Copia(ArquivoId(2))).unwrap();
        assert_eq!(
            comando(&d),
            Comando::Desmarcar {
                alvo: Alvo::Arquivo(ArquivoId(2))
            }
        );
    }

    #[test]
    fn definir_principal_exige_uma_copia() {
        let g = grupo(vec![
            copia(1, true, true, None),
            copia(2, false, true, None),
        ]);
        assert!(planejar(&g, AcaoMarcacao::DefinirPrincipal, AlvoAcao::Grupo).is_err());
        let p = planejar(
            &g,
            AcaoMarcacao::DefinirPrincipal,
            AlvoAcao::Copia(ArquivoId(2)),
        )
        .unwrap();
        assert_eq!(
            comando(&p),
            Comando::DefinirCopiaPrincipal {
                grupo: GrupoId(9),
                arquivo: Some(Alvo::Arquivo(ArquivoId(2)))
            }
        );
    }

    #[test]
    fn copia_de_outro_grupo_e_recusada_e_marcacao_oposta_avisa() {
        let g = grupo(vec![
            copia(1, true, true, Some(TipoMarcacao::Backup)),
            copia(2, false, true, None),
        ]);
        assert!(planejar(&g, AcaoMarcacao::Desmarcar, AlvoAcao::Copia(ArquivoId(77))).is_err());
        let p = planejar(
            &g,
            AcaoMarcacao::MarcarDelecao,
            AlvoAcao::Copia(ArquivoId(1)),
        )
        .unwrap();
        assert!(p.avisos.iter().any(|a| a.texto.contains("marcação oposta")));
        let ja = planejar(
            &g,
            AcaoMarcacao::MarcarBackup,
            AlvoAcao::Copia(ArquivoId(1)),
        )
        .unwrap();
        assert!(
            ja.avisos
                .iter()
                .any(|a| a.texto.contains("já têm esta marcação"))
        );
    }
}
