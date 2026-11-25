//! Elegibilidade para SUGERIR a Lixeira do Volume de um livro (B-16, CA-B16, CA-B17, CA-B18).
//!
//! Função pura: devolve uma recomendação ou o bloqueio com as razões. Nunca executa nada; mesmo
//! a recomendação exige validação final de identidade/estado e dupla confirmação humana.
//! Iniciativa de IA/MCP sozinha é sempre bloqueada. Só uma cópia com Cobertura (cópia com
//! Marcação de Backup em outro Dispositivo Físico), acessível e com hash conferido serve.

use serde::{Deserialize, Serialize};

use crate::tipos::TipoDuplicidade;

/// Quem pediu a sugestão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Iniciativa {
    /// Pessoa, pela interface (GUI/CLI).
    Humana,
    /// IA, MCP ou regra automática.
    Automatica,
}

/// Onde a cópia candidata está.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelCopia {
    /// Formato da própria Biblioteca Homologada.
    Homologada,
    /// Cópia inteira da biblioteca (espelho): candidata a Cobertura, nunca a descarte.
    EspelhoDeBiblioteca,
    /// Pasta de download ou A Classificar: onde o descarte deve ser sugerido.
    DownloadOuAClassificar,
    #[default]
    Outro,
}

/// Cópia a descartar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiaCandidata {
    #[serde(default)]
    pub papel: PapelCopia,
    pub arquivo_id: i64,
    pub volume_id: i64,
    pub dispositivo_id: i64,
    pub hash: Option<String>,
    pub tamanho: u64,
}

/// Outra cópia (a que ficaria).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiaPreservada {
    pub arquivo_id: i64,
    pub volume_id: i64,
    pub dispositivo_id: i64,
    pub hash: Option<String>,
    pub tamanho: u64,
    /// Hash conferido de novo (não só o do Catálogo): a cópia existe e é íntegra.
    pub hash_conferido: bool,
    /// Volume conectado e acessível agora.
    pub volume_conectado: bool,
    /// Tem Marcação de Backup (para Membro: o Contêiner tem).
    pub marcacao_backup: bool,
    /// Está Na Lixeira (não conta como cópia).
    pub na_lixeira: bool,
}

/// Situação da verificação de Cobertura da Biblioteca Homologada (B-26) para o descarte.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "situacao", rename_all = "snake_case")]
pub enum VerificacaoCobertura {
    /// A exigência está desligada (validade 0) ou não se aplica.
    #[default]
    NaoExigida,
    /// Verificação aprovada e dentro da validade.
    Aprovada,
    /// Nunca houve verificação.
    Ausente,
    /// A última aprovada é antiga demais.
    Vencida { dias: u32 },
    /// A última verificação não foi aprovada.
    Reprovada,
}

/// Pedido de sugestão.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PedidoDescarte {
    pub candidata: CopiaCandidata,
    /// Classificação bibliográfica/de hash do candidato contra a preservada.
    pub classificacao: TipoDuplicidade,
    /// Todas as outras cópias conhecidas do mesmo conteúdo.
    pub copias: Vec<CopiaPreservada>,
    pub iniciativa: Iniciativa,
    /// Arquivos que HOJE dão Cobertura ao Formato homologado correspondente (inclui a
    /// candidata, se for o caso). Se a candidata é a única, descartá-la deixa o Formato
    /// homologado sem Cobertura.
    #[serde(default)]
    pub cobertura_do_formato_homologado: Vec<i64>,
    /// Verificação de Cobertura (backup verificado) recente e aprovada da Homologada.
    #[serde(default)]
    pub verificacao_cobertura: VerificacaoCobertura,
}

/// Razão de bloqueio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "razao", rename_all = "snake_case")]
pub enum RazaoBloqueio {
    /// IA/MCP/regra nunca descarta sozinha.
    IniciativaAutomatica,
    /// Só duplicidade Exata (hash + tamanho) autoriza o fluxo.
    NaoExata { classificacao: TipoDuplicidade },
    /// Edição distinta (mesma obra, tradução...): preservar (CA-B17).
    EdicaoDistinta,
    /// Candidata sem hash.
    CandidataSemHash,
    /// Sem verificação de Cobertura recente e aprovada da Biblioteca Homologada (B-26).
    CoberturaNaoVerificada { situacao: VerificacaoCobertura },
    /// Está dentro da Homologada (livro repetido): revisão humana, não descarte.
    RepetidoNaHomologada,
    /// Espelho de biblioteca: serve de Cobertura, não de descarte.
    EspelhoDeBiblioteca,
    /// É a única Cobertura do Formato homologado.
    UnicaCoberturaDoHomologado,
    /// Cópia única: sem outra cópia, o descarte é outro fluxo (confirmação extra de Única Cópia).
    UnicaCopia,
    /// O hash da outra cópia não bate com o da candidata.
    HashDiferente { arquivo_id: i64 },
    /// Hash da cópia preservada não foi conferido.
    HashNaoConferido { arquivo_id: i64 },
    /// Volume da cópia preservada desconectado/inacessível.
    VolumeDesconectado { arquivo_id: i64 },
    /// Cópia preservada está Na Lixeira.
    CopiaNaLixeira { arquivo_id: i64 },
    /// Cópia preservada sem Marcação de Backup.
    SemMarcacaoDeBackup { arquivo_id: i64 },
    /// Cópia no mesmo Dispositivo Físico: sem Cobertura (outra partição do mesmo disco não conta).
    MesmoDispositivo { arquivo_id: i64 },
}

impl RazaoBloqueio {
    /// Frase para mostrar ao usuário.
    #[must_use]
    pub fn texto(&self) -> String {
        match self {
            Self::IniciativaAutomatica => {
                "descarte nunca é iniciado por IA, MCP ou regra automática".into()
            }
            Self::NaoExata { classificacao } => {
                format!("a duplicidade é {classificacao:?}, não Exata")
            }
            Self::EdicaoDistinta => "são edições distintas da mesma obra: preservar".into(),
            Self::CoberturaNaoVerificada { situacao } => match situacao {
                VerificacaoCobertura::Ausente => {
                    "a Cobertura da Biblioteca Homologada nunca foi verificada".into()
                }
                VerificacaoCobertura::Vencida { dias } => format!(
                    "a última verificação de Cobertura aprovada tem {dias} dias: verifique de novo"
                ),
                VerificacaoCobertura::Reprovada => {
                    "a última verificação de Cobertura da Biblioteca Homologada não foi aprovada"
                        .into()
                }
                _ => "a Cobertura da Biblioteca Homologada não está verificada".into(),
            },
            Self::CandidataSemHash => "a cópia candidata não tem Hash Completo".into(),
            Self::RepetidoNaHomologada => {
                "livro repetido dentro da Homologada: revisar, nunca descartar automaticamente"
                    .into()
            }
            Self::EspelhoDeBiblioteca => {
                "cópia espelho da biblioteca: é Cobertura, não candidata a descarte".into()
            }
            Self::UnicaCoberturaDoHomologado => "é a única Cobertura do Formato homologado".into(),
            Self::UnicaCopia => "é a única cópia".into(),
            Self::HashDiferente { arquivo_id } => {
                format!("a cópia {arquivo_id} tem hash diferente")
            }
            Self::HashNaoConferido { arquivo_id } => {
                format!("a cópia {arquivo_id} não teve o hash conferido")
            }
            Self::VolumeDesconectado { arquivo_id } => {
                format!("o volume da cópia {arquivo_id} não está acessível")
            }
            Self::CopiaNaLixeira { arquivo_id } => {
                format!("a cópia {arquivo_id} está na Lixeira e não conta")
            }
            Self::SemMarcacaoDeBackup { arquivo_id } => {
                format!("a cópia {arquivo_id} não tem Marcação de Backup")
            }
            Self::MesmoDispositivo { arquivo_id } => {
                format!("a cópia {arquivo_id} está no mesmo Dispositivo Físico: sem Cobertura")
            }
        }
    }
}

/// Recomendação: ainda não é autorização.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recomendacao {
    /// Cópia preservada que sustenta a Cobertura.
    pub copia_preservada: i64,
    /// Sempre `true`: a execução exige dupla confirmação humana (CA-B18).
    pub exige_dupla_confirmacao: bool,
    /// Passos obrigatórios antes de mover para a Lixeira do Volume.
    pub passos_obrigatorios: Vec<String>,
}

/// Resultado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decisao", rename_all = "snake_case")]
pub enum DecisaoDescarte {
    Sugerir(Recomendacao),
    Bloqueado { razoes: Vec<RazaoBloqueio> },
}

impl DecisaoDescarte {
    #[must_use]
    pub fn e_sugestao(&self) -> bool {
        matches!(self, Self::Sugerir(_))
    }
}

/// Avalia o pedido.
#[must_use]
pub fn avaliar(p: &PedidoDescarte) -> DecisaoDescarte {
    let mut razoes: Vec<RazaoBloqueio> = Vec::new();
    if p.iniciativa == Iniciativa::Automatica {
        razoes.push(RazaoBloqueio::IniciativaAutomatica);
    }
    match p.classificacao {
        TipoDuplicidade::Exata => {}
        TipoDuplicidade::MesmaObra => razoes.push(RazaoBloqueio::EdicaoDistinta),
        outro => razoes.push(RazaoBloqueio::NaoExata {
            classificacao: outro,
        }),
    }
    if p.candidata.hash.is_none() {
        razoes.push(RazaoBloqueio::CandidataSemHash);
    }
    if !matches!(
        p.verificacao_cobertura,
        VerificacaoCobertura::NaoExigida | VerificacaoCobertura::Aprovada
    ) {
        razoes.push(RazaoBloqueio::CoberturaNaoVerificada {
            situacao: p.verificacao_cobertura.clone(),
        });
    }
    match p.candidata.papel {
        PapelCopia::Homologada => razoes.push(RazaoBloqueio::RepetidoNaHomologada),
        PapelCopia::EspelhoDeBiblioteca => razoes.push(RazaoBloqueio::EspelhoDeBiblioteca),
        _ => {}
    }
    if p.cobertura_do_formato_homologado == [p.candidata.arquivo_id] {
        razoes.push(RazaoBloqueio::UnicaCoberturaDoHomologado);
    }
    if !razoes.is_empty() {
        return DecisaoDescarte::Bloqueado { razoes };
    }

    if p.copias.is_empty() {
        return DecisaoDescarte::Bloqueado {
            razoes: vec![RazaoBloqueio::UnicaCopia],
        };
    }
    let mut todas: Vec<RazaoBloqueio> = Vec::new();
    for c in &p.copias {
        let mut r: Vec<RazaoBloqueio> = Vec::new();
        if c.hash != p.candidata.hash || c.tamanho != p.candidata.tamanho {
            r.push(RazaoBloqueio::HashDiferente {
                arquivo_id: c.arquivo_id,
            });
        }
        if c.na_lixeira {
            r.push(RazaoBloqueio::CopiaNaLixeira {
                arquivo_id: c.arquivo_id,
            });
        }
        if !c.volume_conectado {
            r.push(RazaoBloqueio::VolumeDesconectado {
                arquivo_id: c.arquivo_id,
            });
        }
        if !c.hash_conferido {
            r.push(RazaoBloqueio::HashNaoConferido {
                arquivo_id: c.arquivo_id,
            });
        }
        if !c.marcacao_backup {
            r.push(RazaoBloqueio::SemMarcacaoDeBackup {
                arquivo_id: c.arquivo_id,
            });
        }
        if c.dispositivo_id == p.candidata.dispositivo_id {
            r.push(RazaoBloqueio::MesmoDispositivo {
                arquivo_id: c.arquivo_id,
            });
        }
        if r.is_empty() {
            return DecisaoDescarte::Sugerir(Recomendacao {
                copia_preservada: c.arquivo_id,
                exige_dupla_confirmacao: true,
                passos_obrigatorios: vec![
                    "revalidar identidade (hash) e estado das duas cópias".into(),
                    "primeira confirmação humana".into(),
                    "segunda confirmação humana".into(),
                    "mover para a Lixeira do próprio Volume com registro em log".into(),
                    "testar a restauração; nunca esvaziar a Lixeira automaticamente".into(),
                ],
            });
        }
        for x in r {
            if !todas.contains(&x) {
                todas.push(x);
            }
        }
    }
    DecisaoDescarte::Bloqueado { razoes: todas }
}

/// Avalia vários pedidos e devolve as SUGESTÕES na ordem de preferência: primeiro cópias em
/// download/A Classificar, depois as demais, e dentro de cada grupo as maiores. Bloqueados
/// ficam de fora (use [`avaliar`] para ver as razões).
#[must_use]
pub fn sugestoes_em_ordem(pedidos: &[PedidoDescarte]) -> Vec<(usize, Recomendacao)> {
    let mut v: Vec<(usize, Recomendacao)> = pedidos
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match avaliar(p) {
            DecisaoDescarte::Sugerir(r) => Some((i, r)),
            DecisaoDescarte::Bloqueado { .. } => None,
        })
        .collect();
    v.sort_by_key(|(i, _)| {
        let p = &pedidos[*i];
        (
            u8::from(p.candidata.papel != PapelCopia::DownloadOuAClassificar),
            std::cmp::Reverse(p.candidata.tamanho),
            *i,
        )
    });
    v
}

#[cfg(test)]
mod testes {
    use super::*;

    fn candidata() -> CopiaCandidata {
        CopiaCandidata {
            papel: PapelCopia::DownloadOuAClassificar,
            arquivo_id: 1,
            volume_id: 10,
            dispositivo_id: 100,
            hash: Some("h".into()),
            tamanho: 50,
        }
    }

    fn boa() -> CopiaPreservada {
        CopiaPreservada {
            arquivo_id: 2,
            volume_id: 20,
            dispositivo_id: 200,
            hash: Some("h".into()),
            tamanho: 50,
            hash_conferido: true,
            volume_conectado: true,
            marcacao_backup: true,
            na_lixeira: false,
        }
    }

    fn pedido(copias: Vec<CopiaPreservada>) -> PedidoDescarte {
        PedidoDescarte {
            candidata: candidata(),
            classificacao: TipoDuplicidade::Exata,
            copias,
            iniciativa: Iniciativa::Humana,
            cobertura_do_formato_homologado: vec![],
            verificacao_cobertura: VerificacaoCobertura::Aprovada,
        }
    }

    #[test]
    fn sem_verificacao_de_cobertura_recente_o_descarte_e_bloqueado() {
        let mut p = pedido(vec![boa()]);
        p.candidata.papel = PapelCopia::Outro;
        assert!(avaliar(&p).e_sugestao());
        for v in [
            VerificacaoCobertura::Ausente,
            VerificacaoCobertura::Vencida { dias: 40 },
            VerificacaoCobertura::Reprovada,
        ] {
            p.verificacao_cobertura = v.clone();
            assert!(
                razoes(&avaliar(&p))
                    .contains(&RazaoBloqueio::CoberturaNaoVerificada { situacao: v })
            );
        }
        p.verificacao_cobertura = VerificacaoCobertura::NaoExigida;
        assert!(avaliar(&p).e_sugestao());
    }

    #[test]
    fn homologada_e_espelho_nunca_sao_descartados() {
        let mut p = pedido(vec![boa()]);
        p.candidata.papel = PapelCopia::Homologada;
        assert!(razoes(&avaliar(&p)).contains(&RazaoBloqueio::RepetidoNaHomologada));
        p.candidata.papel = PapelCopia::EspelhoDeBiblioteca;
        assert!(razoes(&avaliar(&p)).contains(&RazaoBloqueio::EspelhoDeBiblioteca));
    }

    #[test]
    fn unica_cobertura_do_formato_homologado_nunca_e_descartada() {
        let mut p = pedido(vec![boa()]);
        p.candidata.papel = PapelCopia::Outro;
        p.cobertura_do_formato_homologado = vec![1];
        assert!(razoes(&avaliar(&p)).contains(&RazaoBloqueio::UnicaCoberturaDoHomologado));
        p.cobertura_do_formato_homologado = vec![1, 2];
        assert!(avaliar(&p).e_sugestao());
    }

    #[test]
    fn sugestoes_preferem_downloads_depois_maiores() {
        let mk = |id, papel, tam| {
            let mut p = pedido(vec![boa()]);
            p.candidata.arquivo_id = id;
            p.candidata.papel = papel;
            p.candidata.tamanho = tam;
            p.copias[0].tamanho = tam;
            p
        };
        let ped = [
            mk(1, PapelCopia::Outro, 900),
            mk(2, PapelCopia::DownloadOuAClassificar, 10),
            mk(3, PapelCopia::EspelhoDeBiblioteca, 5000),
            mk(4, PapelCopia::DownloadOuAClassificar, 50),
        ];
        let o: Vec<usize> = sugestoes_em_ordem(&ped)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        assert_eq!(o, [3, 1, 0]);
    }

    fn razoes(d: &DecisaoDescarte) -> Vec<RazaoBloqueio> {
        match d {
            DecisaoDescarte::Bloqueado { razoes } => razoes.clone(),
            DecisaoDescarte::Sugerir(_) => vec![],
        }
    }

    #[test]
    fn caminho_feliz_sugere_e_exige_dupla_confirmacao() {
        let d = avaliar(&pedido(vec![boa()]));
        let DecisaoDescarte::Sugerir(r) = d else {
            panic!("{d:?}")
        };
        assert!(r.exige_dupla_confirmacao);
        assert_eq!(r.copia_preservada, 2);
        assert!(
            r.passos_obrigatorios
                .iter()
                .any(|p| p.contains("restauração"))
        );
    }

    #[test]
    fn ca_b16_copia_inacessivel_bloqueia() {
        let mut c = boa();
        c.volume_conectado = false;
        let d = avaliar(&pedido(vec![c]));
        assert!(!d.e_sugestao());
        assert!(razoes(&d).contains(&RazaoBloqueio::VolumeDesconectado { arquivo_id: 2 }));
    }

    #[test]
    fn ca_b16_hash_nao_conferido_ou_backup_nao_verificado_bloqueia() {
        let mut c = boa();
        c.hash_conferido = false;
        assert!(
            razoes(&avaliar(&pedido(vec![c])))
                .contains(&RazaoBloqueio::HashNaoConferido { arquivo_id: 2 })
        );
        let mut c = boa();
        c.marcacao_backup = false;
        assert!(
            razoes(&avaliar(&pedido(vec![c])))
                .contains(&RazaoBloqueio::SemMarcacaoDeBackup { arquivo_id: 2 })
        );
    }

    #[test]
    fn sem_cobertura_no_mesmo_dispositivo_fisico() {
        let mut c = boa();
        c.dispositivo_id = 100;
        c.volume_id = 11; // outra partição do mesmo disco
        let d = avaliar(&pedido(vec![c]));
        assert!(razoes(&d).contains(&RazaoBloqueio::MesmoDispositivo { arquivo_id: 2 }));
    }

    #[test]
    fn ca_b17_unica_copia_bloqueia() {
        let d = avaliar(&pedido(vec![]));
        assert_eq!(razoes(&d), [RazaoBloqueio::UnicaCopia]);
    }

    #[test]
    fn ca_b17_edicao_distinta_e_nao_exata_bloqueiam() {
        let mut p = pedido(vec![boa()]);
        p.classificacao = TipoDuplicidade::MesmaObra;
        assert_eq!(razoes(&avaliar(&p)), [RazaoBloqueio::EdicaoDistinta]);
        for t in [TipoDuplicidade::MesmaEdicao, TipoDuplicidade::Incerta] {
            p.classificacao = t;
            assert_eq!(
                razoes(&avaliar(&p)),
                [RazaoBloqueio::NaoExata { classificacao: t }]
            );
        }
    }

    #[test]
    fn ia_ou_mcp_sozinha_nunca() {
        let mut p = pedido(vec![boa()]);
        p.iniciativa = Iniciativa::Automatica;
        let d = avaliar(&p);
        assert!(!d.e_sugestao());
        assert!(razoes(&d).contains(&RazaoBloqueio::IniciativaAutomatica));
    }

    #[test]
    fn copia_na_lixeira_ou_hash_diferente_nao_serve() {
        let mut c = boa();
        c.na_lixeira = true;
        assert!(
            razoes(&avaliar(&pedido(vec![c])))
                .contains(&RazaoBloqueio::CopiaNaLixeira { arquivo_id: 2 })
        );
        let mut c = boa();
        c.hash = Some("outro".into());
        assert!(
            razoes(&avaliar(&pedido(vec![c])))
                .contains(&RazaoBloqueio::HashDiferente { arquivo_id: 2 })
        );
    }

    #[test]
    fn basta_uma_copia_boa_entre_varias() {
        let mut ruim = boa();
        ruim.arquivo_id = 3;
        ruim.volume_conectado = false;
        let d = avaliar(&pedido(vec![ruim, boa()]));
        let DecisaoDescarte::Sugerir(r) = d else {
            panic!()
        };
        assert_eq!(r.copia_preservada, 2);
    }

    #[test]
    fn candidata_sem_hash_bloqueia_e_razoes_tem_texto() {
        let mut p = pedido(vec![boa()]);
        p.candidata.hash = None;
        let rs = razoes(&avaliar(&p));
        assert!(rs.contains(&RazaoBloqueio::CandidataSemHash));
        assert!(rs.iter().all(|r| !r.texto().is_empty()));
    }
}
