//! Seção Biblioteca da Bancada: a GUI da Curadoria Bibliográfica (Fase B).
//!
//! Páginas: Visão Geral, Duplicatas, Revisão, A Classificar, Homologada (leitura), Alertas e
//! Resumo, Modelos IA, Configurações e Monitoramento. Tudo o que vem do agente chega pelos
//! `Comando::Biblioteca*` / `Resposta::Biblioteca` do protocolo (`docs/fase-b/PROTOCOLO-BIBLIOTECA.md`),
//! sempre por pedidos ao worker (a janela nunca bloqueia). A Biblioteca Homologada é somente
//! leitura e a GUI nunca move nem apaga arquivo: o descarte segue o fluxo da Fase A (Marcação
//! para Deleção e os dois diálogos da Lixeira do Volume, ADR 0005).
//!
//! Este módulo guarda o estado compartilhado ([`Estado`]), o recebimento das respostas
//! ([`Estado::receber`], puro e testável) e as Ações do Ribbon. Cada página fica em arquivo
//! próprio.

pub mod a_classificar;
pub mod alertas;
pub mod comandos;
pub mod configuracoes;
pub mod duplicatas;
pub mod homologada;
pub mod modelos_ia;
pub mod monitoramento;
pub mod revisao;
pub mod texto;
pub mod visao_geral;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use bancada_casca::{Tom, egui};
use dag_core::{
    AlertaInfo, BibliotecaInfo, CampoInfo, Comando, ConfigBiblioteca, ContagemFila,
    DadosBiblioteca, EstadoFilaInfo, GrupoDuplicata, HomologacaoInfo, ItemFilaInfo,
    ItemRevisaoInfo, PapelRegistro, PerfilResumo, ProgressoBiblioteca, RelatorioEspacoInfo,
    ResultadoCadastro, ResumoBiblioteca, ResumoEnfileiramento, ResumoProcessamento,
    ResumoSincronizacao, TipoJob,
};

use super::{DefAcao, DefGrupo, DefPagina};
use crate::app::{AppDag, Proposito};
use crate::formato::{bytes_humanos, inteiro_pt_br};

/// Páginas da seção, na ordem do menu.
#[must_use]
pub fn todas() -> [&'static DefPagina; 9] {
    [
        &visao_geral::PAGINA,
        &duplicatas::PAGINA,
        &revisao::PAGINA,
        &a_classificar::PAGINA,
        &homologada::PAGINA,
        &alertas::PAGINA,
        &modelos_ia::PAGINA,
        &configuracoes::PAGINA,
        &monitoramento::PAGINA,
    ]
}

/// Rótulo do grupo do menu de navegação.
pub const GRUPO_NAVEGACAO: &str = "Biblioteca";

/// Intervalo entre releituras da fila e do resumo enquanto um job Biblioteca roda.
const INTERVALO_JOB: Duration = Duration::from_secs(4);

/// Um dado que as páginas pedem ao agente (somente leitura, exceto `Duplicatas`, que também
/// registra candidatos e alertas, conforme o protocolo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Consulta {
    Bibliotecas,
    Resumo,
    Espaco,
    Fila,
    /// Itens concluídos da fila (os candidatos a cadastrar).
    Concluidos,
    /// Itens em exceção (Monitoramento).
    Excecoes,
    Duplicatas,
    Revisao,
    Alertas,
    Perfil,
    Config,
}

impl Consulta {
    pub const TODAS: [Self; 11] = [
        Self::Bibliotecas,
        Self::Resumo,
        Self::Espaco,
        Self::Fila,
        Self::Concluidos,
        Self::Excecoes,
        Self::Duplicatas,
        Self::Revisao,
        Self::Alertas,
        Self::Perfil,
        Self::Config,
    ];
}

/// Uma escrita ou trabalho que a GUI pediu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AcaoBib {
    Registrar,
    Sincronizar,
    Enfileirar,
    Processar,
    Decidir,
    Desfazer,
    Cadastrar,
    Homologar,
    MarcarVisto,
    ConfigDefinir,
}

impl AcaoBib {
    /// Trabalhos longos do agente (job `Biblioteca`).
    #[must_use]
    pub fn e_longa(self) -> bool {
        matches!(
            self,
            Self::Sincronizar
                | Self::Enfileirar
                | Self::Processar
                | Self::Cadastrar
                | Self::Homologar
        )
    }
}

/// Para que serve um pedido em voo (para tratar a resposta).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropBib {
    Consulta(Consulta),
    /// Próxima página de uma lista (anexa em vez de substituir).
    Mais(Consulta),
    Acao(AcaoBib),
}

/// Aviso a mostrar na faixa de mensagens.
pub type AvisoBib = (Tom, String);

/// A fila lida do agente.
#[derive(Debug, Clone, Default)]
pub struct DadosFila {
    pub contagem: ContagemFila,
    pub itens: Vec<ItemFilaInfo>,
    pub proximo: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DadosDuplicatas {
    pub grupos: Vec<GrupoDuplicata>,
    pub alertas_emitidos: u64,
    pub proximo: Option<String>,
}

impl DadosDuplicatas {
    /// Soma dos bytes das cópias dos grupos carregados.
    #[must_use]
    pub fn bytes_totais(&self) -> u64 {
        self.grupos.iter().map(|g| g.bytes_totais).sum()
    }

    /// Soma dos bytes das cópias com decisão `sugerir`.
    #[must_use]
    pub fn bytes_sugeridos(&self) -> u64 {
        self.grupos.iter().map(|g| g.bytes_sugeridos).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct DadosRevisao {
    pub itens: Vec<ItemRevisaoInfo>,
    pub total_pendentes: i64,
    pub proximo: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DadosAlertas {
    pub itens: Vec<AlertaInfo>,
    pub nao_vistos_total: i64,
    pub proximo: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DadosPerfil {
    pub biblioteca: BibliotecaInfo,
    pub perfil: PerfilResumo,
    pub campos: Vec<CampoInfo>,
}

/// Estado da seção Biblioteca: o que o agente respondeu, o que está em voo e o estado de tela de
/// cada página.
#[derive(Debug)]
pub struct Estado {
    pub bibliotecas: Option<Vec<BibliotecaInfo>>,
    pub resumo: Option<ResumoBiblioteca>,
    pub espaco: Option<RelatorioEspacoInfo>,
    pub fila: Option<DadosFila>,
    pub concluidos: Option<DadosFila>,
    pub excecoes: Option<DadosFila>,
    pub duplicatas: Option<DadosDuplicatas>,
    pub revisao: Option<DadosRevisao>,
    pub alertas: Option<DadosAlertas>,
    pub perfil: Option<DadosPerfil>,
    pub config: Option<ConfigBiblioteca>,
    /// Último lote de decisões (para "Desfazer último lote").
    pub ultimo_lote: Option<String>,
    pub ultima_decisao: Option<String>,
    pub sincronizacao: Option<ResumoSincronizacao>,
    pub enfileiramento: Option<ResumoEnfileiramento>,
    pub processamento: Option<ResumoProcessamento>,
    pub cadastro: Option<Vec<ResultadoCadastro>>,
    /// Calibre ou servidor ocupado: o que sobrou do cadastro continua pendente.
    pub bloqueio_calibre: Option<String>,
    pub homologacao: Option<HomologacaoInfo>,
    /// Último erro de cada consulta (código e mensagem).
    pub erros: HashMap<Consulta, (String, String)>,
    /// Último erro de uma ação.
    pub erro_acao: Option<(AcaoBib, String, String)>,
    /// Progresso do job Biblioteca (a última mensagem `biblioteca: ...` do agente).
    pub progresso: Option<ProgressoBiblioteca>,
    /// Alertas novos anunciados durante um job.
    pub alertas_recentes: Vec<String>,
    /// Sobe quando o que o leitor do Catálogo mostra (cadastros) pode ter mudado.
    pub geracao_leitor: u64,
    pendentes: HashSet<Consulta>,
    em_voo: HashSet<Consulta>,
    acoes_em_voo: HashSet<AcaoBib>,
    ultimo_refresco: Option<Instant>,
    pub visao: visao_geral::Ui,
    pub dup: duplicatas::Ui,
    pub rev: revisao::Ui,
    pub classificar: a_classificar::Ui,
    pub alerta: alertas::Ui,
    pub ia: modelos_ia::Ui,
    pub config_ui: configuracoes::Ui,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            bibliotecas: None,
            resumo: None,
            espaco: None,
            fila: None,
            concluidos: None,
            excecoes: None,
            duplicatas: None,
            revisao: None,
            alertas: None,
            perfil: None,
            config: None,
            ultimo_lote: None,
            ultima_decisao: None,
            sincronizacao: None,
            enfileiramento: None,
            processamento: None,
            cadastro: None,
            bloqueio_calibre: None,
            homologacao: None,
            erros: HashMap::new(),
            erro_acao: None,
            progresso: None,
            alertas_recentes: Vec::new(),
            geracao_leitor: 1,
            pendentes: Consulta::TODAS.into_iter().collect(),
            em_voo: HashSet::new(),
            acoes_em_voo: HashSet::new(),
            ultimo_refresco: None,
            visao: visao_geral::Ui::default(),
            dup: duplicatas::Ui::default(),
            rev: revisao::Ui::default(),
            classificar: a_classificar::Ui::default(),
            alerta: alertas::Ui::default(),
            ia: modelos_ia::Ui::default(),
            config_ui: configuracoes::Ui::default(),
        }
    }
}

/// Orientação em português para os erros com código estável do protocolo.
#[must_use]
pub fn orientacao_do_erro(codigo: &str) -> Option<&'static str> {
    Some(match codigo {
        "calibre_ocupado" => {
            "O Calibre (ou o calibre-server) está com a biblioteca aberta. Feche o Calibre ou \
             pare o servidor e tente de novo: nada foi gravado e o que ficou pendente continua na \
             fila (o cadastro é idempotente)."
        }
        "calibredb_ausente" => {
            "O calibredb não foi encontrado. Instale o Calibre (o DAG só usa a interface oficial \
             dele) e tente de novo."
        }
        "aprovacao_humana_exigida" => {
            "A homologação só vale com a aprovação de uma pessoa, dada aqui na GUI ou na CLI."
        }
        "aprovacao_invalida" => "Diga quem aprova a homologação.",
        "biblioteca_nao_registrada" => {
            "Registre a biblioteca na Visão Geral antes de usar esta função."
        }
        "sem_homologada" => "Registre a Biblioteca Homologada na Visão Geral primeiro.",
        "sem_a_classificar" => "Registre (ou crie) a biblioteca A Classificar na Visão Geral.",
        "cadastro_inexistente" => "Esse cadastro não existe mais: atualize a lista.",
        "cadastro_nao_pronto" => "O livro ainda não foi cadastrado na A Classificar.",
        "ja_existe_na_homologada" => {
            "Esse livro já está na Biblioteca Homologada (mesmo conteúdo): não há o que cadastrar."
        }
        "config_invalida" => "A configuração foi recusada: confira os valores e o endpoint da IA.",
        "papel_invalido" => "O papel da biblioteca precisa ser Homologada ou A Classificar.",
        _ => return None,
    })
}

impl Estado {
    // ------------------------------------------------------------------ consultas

    /// Precisa (re)pedir esta consulta agora?
    #[must_use]
    pub fn precisa(&self, c: Consulta) -> bool {
        self.pendentes.contains(&c) && !self.em_voo.contains(&c)
    }

    /// A consulta está em voo (ou pedida e ainda não respondida)?
    #[must_use]
    pub fn carregando(&self, c: Consulta) -> bool {
        self.em_voo.contains(&c)
    }

    /// Marca as consultas para serem relidas.
    pub fn atualizar(&mut self, cs: &[Consulta]) {
        self.pendentes.extend(cs.iter().copied());
    }

    /// Tudo relido na próxima visita (reconexão, job que terminou).
    pub fn tudo_obsoleto(&mut self) {
        self.pendentes.extend(Consulta::TODAS);
    }

    /// A Biblioteca Homologada registrada (a primeira).
    #[must_use]
    pub fn homologada(&self) -> Option<&BibliotecaInfo> {
        self.bibliotecas
            .as_ref()?
            .iter()
            .find(|b| b.papel == PapelRegistro::Homologada)
    }

    /// A biblioteca A Classificar registrada (a primeira).
    #[must_use]
    pub fn a_classificar(&self) -> Option<&BibliotecaInfo> {
        self.bibliotecas
            .as_ref()?
            .iter()
            .find(|b| b.papel == PapelRegistro::AClassificar)
    }

    /// Comando que lê `c` (com o cursor da próxima página, se houver). `None` quando ainda falta
    /// um dado para montá-lo (o perfil espera a lista de bibliotecas).
    #[must_use]
    pub fn comando_da_consulta(&self, c: Consulta, cursor: Option<String>) -> Option<Comando> {
        Some(match c {
            Consulta::Bibliotecas => comandos::listar(),
            Consulta::Resumo => comandos::resumo(self.alerta.desde()),
            Consulta::Espaco => comandos::relatorio_espaco(),
            Consulta::Fila => comandos::fila(None, cursor),
            Consulta::Concluidos => comandos::fila(Some(EstadoFilaInfo::Concluido), cursor),
            Consulta::Excecoes => comandos::fila(Some(EstadoFilaInfo::Excecao), cursor),
            Consulta::Duplicatas => comandos::duplicatas(cursor),
            Consulta::Revisao => comandos::revisao(cursor),
            Consulta::Alertas => comandos::alertas(self.alerta.so_nao_vistos, cursor),
            Consulta::Perfil => comandos::perfil(self.homologada()?.id),
            Consulta::Config => comandos::config_obter(),
        })
    }

    /// Anota que a consulta foi enviada.
    pub fn enviada(&mut self, c: Consulta) {
        self.pendentes.remove(&c);
        self.em_voo.insert(c);
        self.erros.remove(&c);
    }

    pub fn acao_enviada(&mut self, a: AcaoBib) {
        self.acoes_em_voo.insert(a);
        self.erro_acao = None;
    }

    #[must_use]
    pub fn acao_em_voo(&self, a: AcaoBib) -> bool {
        self.acoes_em_voo.contains(&a)
    }

    /// Algum trabalho longo da GUI está em voo?
    #[must_use]
    pub fn trabalho_em_voo(&self) -> bool {
        self.acoes_em_voo.iter().any(|a| a.e_longa()) || self.em_voo.contains(&Consulta::Duplicatas)
    }

    // ------------------------------------------------------------------ respostas

    /// Erro do agente a um pedido da seção. Devolve avisos extras (a orientação do erro).
    pub fn falhou(&mut self, prop: PropBib, codigo: &str, mensagem: &str) -> Vec<AvisoBib> {
        let mut avisos = Vec::new();
        match prop {
            PropBib::Consulta(c) | PropBib::Mais(c) => {
                self.em_voo.remove(&c);
                self.erros
                    .insert(c, (codigo.to_string(), mensagem.to_string()));
            }
            PropBib::Acao(a) => {
                self.acoes_em_voo.remove(&a);
                if codigo == "calibre_ocupado" {
                    self.bloqueio_calibre = Some(mensagem.to_string());
                }
                self.erro_acao = Some((a, codigo.to_string(), mensagem.to_string()));
            }
        }
        if let Some(o) = orientacao_do_erro(codigo) {
            avisos.push((Tom::Aviso, o.to_string()));
        }
        avisos
    }

    fn concluiu(&mut self, prop: Option<PropBib>, esperada: Consulta) -> bool {
        match prop {
            Some(PropBib::Mais(c)) if c == esperada => {
                self.em_voo.remove(&c);
                true
            }
            Some(PropBib::Consulta(c)) if c == esperada => {
                self.em_voo.remove(&c);
                false
            }
            _ => false,
        }
    }

    fn terminou(&mut self, a: AcaoBib) {
        self.acoes_em_voo.remove(&a);
    }

    /// Recebe a resposta do agente. Devolve os avisos para a faixa de mensagens.
    #[allow(clippy::too_many_lines)]
    pub fn receber(&mut self, dados: DadosBiblioteca, prop: Option<PropBib>) -> Vec<AvisoBib> {
        let mut avisos: Vec<AvisoBib> = Vec::new();
        match dados {
            DadosBiblioteca::Bibliotecas { itens } => {
                self.em_voo.remove(&Consulta::Bibliotecas);
                self.bibliotecas = Some(itens);
            }
            DadosBiblioteca::Registrada {
                biblioteca,
                criada,
                avisos: av,
            } => {
                self.terminou(AcaoBib::Registrar);
                avisos.push((
                    Tom::Sucesso,
                    format!(
                        "Biblioteca {} ({}) {}.",
                        texto::texto_puro(&biblioteca.nome, 80),
                        texto::rotulo_papel(biblioteca.papel),
                        if criada {
                            "criada e registrada"
                        } else {
                            "registrada"
                        }
                    ),
                ));
                for a in av {
                    avisos.push((Tom::Info, texto::texto_puro(&a, 300)));
                }
                self.atualizar(&[Consulta::Bibliotecas, Consulta::Resumo, Consulta::Perfil]);
                self.geracao_leitor += 1;
            }
            DadosBiblioteca::Perfil {
                biblioteca,
                perfil,
                campos,
            } => {
                self.em_voo.remove(&Consulta::Perfil);
                self.perfil = Some(DadosPerfil {
                    biblioteca,
                    perfil,
                    campos,
                });
            }
            DadosBiblioteca::Sincronizada { resumo } => {
                self.terminou(AcaoBib::Sincronizar);
                let (tom, t) = if resumo.biblioteca_intacta {
                    (
                        Tom::Sucesso,
                        format!(
                            "Sincronizada: {} livro(s), {} formato(s), {} hash(es) do Catálogo \
                             reaproveitado(s). A biblioteca não mudou (somente leitura).",
                            inteiro_pt_br(resumo.livros),
                            inteiro_pt_br(resumo.formatos),
                            inteiro_pt_br(resumo.hashes_importados),
                        ),
                    )
                } else {
                    (
                        Tom::Perigo,
                        "A sincronização notou diferenças na biblioteca durante a leitura \
                         (alguém a alterou ao mesmo tempo, p. ex. o Calibre aberto). Confira em \
                         Visão Geral."
                            .to_string(),
                    )
                };
                avisos.push((tom, t));
                self.sincronizacao = Some(resumo);
                self.progresso = None;
                self.atualizar(&[
                    Consulta::Bibliotecas,
                    Consulta::Resumo,
                    Consulta::Espaco,
                    Consulta::Perfil,
                    Consulta::Duplicatas,
                ]);
            }
            DadosBiblioteca::Duplicatas {
                grupos,
                alertas_emitidos,
                proximo_cursor,
                ..
            } => {
                let anexa = self.concluiu(prop, Consulta::Duplicatas);
                self.em_voo.remove(&Consulta::Duplicatas);
                let atual = self.duplicatas.get_or_insert_with(DadosDuplicatas::default);
                if anexa {
                    atual.grupos.extend(grupos);
                    atual.alertas_emitidos += alertas_emitidos;
                } else {
                    atual.grupos = grupos;
                    atual.alertas_emitidos = alertas_emitidos;
                }
                atual.proximo = proximo_cursor;
                if alertas_emitidos > 0 {
                    self.atualizar(&[Consulta::Alertas, Consulta::Resumo]);
                }
            }
            DadosBiblioteca::Enfileirada { resumo } => {
                self.terminou(AcaoBib::Enfileirar);
                avisos.push((
                    Tom::Sucesso,
                    format!(
                        "Fila: {} enfileirado(s), {} já estavam na fila, {} ignorado(s) de {} \
                         examinado(s).{}",
                        inteiro_pt_br(resumo.enfileirados),
                        inteiro_pt_br(resumo.ja_na_fila),
                        inteiro_pt_br(resumo.ignorados),
                        inteiro_pt_br(resumo.examinados),
                        if resumo.interrompido {
                            " (interrompido)"
                        } else {
                            ""
                        }
                    ),
                ));
                self.enfileiramento = Some(resumo);
                self.progresso = None;
                self.atualizar(&[Consulta::Fila, Consulta::Resumo, Consulta::Concluidos]);
            }
            DadosBiblioteca::Fila {
                contagem,
                itens,
                proximo_cursor,
            } => {
                let (destino, consulta) = match prop {
                    Some(PropBib::Consulta(c) | PropBib::Mais(c))
                        if matches!(
                            c,
                            Consulta::Fila | Consulta::Concluidos | Consulta::Excecoes
                        ) =>
                    {
                        (c, c)
                    }
                    _ => (Consulta::Fila, Consulta::Fila),
                };
                let anexa = self.concluiu(prop, consulta);
                self.em_voo.remove(&consulta);
                let campo = match destino {
                    Consulta::Concluidos => &mut self.concluidos,
                    Consulta::Excecoes => &mut self.excecoes,
                    _ => &mut self.fila,
                };
                let d = campo.get_or_insert_with(DadosFila::default);
                d.contagem = contagem;
                if anexa {
                    d.itens.extend(itens);
                } else {
                    d.itens = itens;
                }
                d.proximo = proximo_cursor;
            }
            DadosBiblioteca::Processada { resumo } => {
                self.terminou(AcaoBib::Processar);
                let mut t = format!(
                    "Processamento: {} item(ns) ({} duplicata(s) exata(s), {} extraído(s), {} \
                     reaproveitado(s)), {} para revisão, {} exceção(ões), {} falha(s).",
                    inteiro_pt_br(resumo.processados),
                    inteiro_pt_br(resumo.duplicatas_exatas),
                    inteiro_pt_br(resumo.extraidos),
                    inteiro_pt_br(resumo.reaproveitados),
                    inteiro_pt_br(resumo.revisao_pendente),
                    inteiro_pt_br(resumo.excecoes),
                    inteiro_pt_br(resumo.falhas),
                );
                if resumo.interrompido {
                    t.push_str(&format!(
                        " Interrompido; {} pendente(s) continuam na fila.",
                        inteiro_pt_br(u64::try_from(resumo.pendentes_restantes).unwrap_or(0))
                    ));
                }
                let tom = if resumo.falhas > 0 {
                    Tom::Aviso
                } else {
                    Tom::Sucesso
                };
                avisos.push((tom, t));
                self.processamento = Some(resumo);
                self.progresso = None;
                self.atualizar(&[
                    Consulta::Fila,
                    Consulta::Concluidos,
                    Consulta::Excecoes,
                    Consulta::Resumo,
                    Consulta::Revisao,
                    Consulta::Alertas,
                    Consulta::Espaco,
                    Consulta::Duplicatas,
                ]);
            }
            DadosBiblioteca::Revisao {
                itens,
                total_pendentes,
                proximo_cursor,
            } => {
                let anexa = self.concluiu(prop, Consulta::Revisao);
                self.em_voo.remove(&Consulta::Revisao);
                let d = self.revisao.get_or_insert_with(DadosRevisao::default);
                if anexa {
                    d.itens.extend(itens);
                } else {
                    d.itens = itens;
                }
                d.total_pendentes = total_pendentes;
                d.proximo = proximo_cursor;
                self.rev.reconciliar(&d.itens);
            }
            DadosBiblioteca::Decidida {
                lote,
                aplicadas,
                ignoradas,
            } => {
                self.terminou(AcaoBib::Decidir);
                let mut t = format!(
                    "Lote {}: {} decisão(ões) aplicada(s).",
                    texto::texto_puro(&lote, 40),
                    inteiro_pt_br(aplicadas)
                );
                if !ignoradas.is_empty() {
                    t.push_str(&format!(
                        " {} ignorada(s): {}",
                        ignoradas.len(),
                        ignoradas
                            .iter()
                            .take(3)
                            .map(|i| texto::texto_puro(i, 80))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                }
                avisos.push((
                    if ignoradas.is_empty() {
                        Tom::Sucesso
                    } else {
                        Tom::Aviso
                    },
                    t.clone(),
                ));
                self.ultima_decisao = Some(t);
                if aplicadas > 0 {
                    self.ultimo_lote = Some(lote);
                }
                self.rev.escolhidos.clear();
                self.atualizar(&[Consulta::Revisao, Consulta::Resumo, Consulta::Concluidos]);
            }
            DadosBiblioteca::Desfeita { lote, desfeitas } => {
                self.terminou(AcaoBib::Desfazer);
                let t = format!(
                    "Lote {} desfeito: {} decisão(ões) voltaram a pendente.",
                    texto::texto_puro(&lote, 40),
                    inteiro_pt_br(desfeitas)
                );
                avisos.push((Tom::Sucesso, t.clone()));
                self.ultima_decisao = Some(t);
                if self.ultimo_lote.as_deref() == Some(lote.as_str()) {
                    self.ultimo_lote = None;
                }
                self.atualizar(&[Consulta::Revisao, Consulta::Resumo, Consulta::Concluidos]);
            }
            DadosBiblioteca::Cadastrada {
                resultados,
                bloqueado,
            } => {
                self.terminou(AcaoBib::Cadastrar);
                let n = |e: &str| resultados.iter().filter(|r| r.estado == e).count();
                let mut t = format!(
                    "Cadastro na A Classificar: {} cadastrado(s), {} já cadastrado(s), {} já \
                     existia(m) na Homologada, {} falha(s).",
                    n("cadastrado"),
                    n("ja_cadastrado"),
                    n("ja_existe_na_homologada"),
                    n("falhou") + n("recusado"),
                );
                let tom = if bloqueado.is_some() {
                    t.push_str(
                        " O Calibre está ocupado: o que sobrou ficou pendente (feche o Calibre \
                         e cadastre de novo).",
                    );
                    Tom::Aviso
                } else if n("falhou") > 0 {
                    Tom::Aviso
                } else {
                    Tom::Sucesso
                };
                avisos.push((tom, t));
                self.bloqueio_calibre = bloqueado;
                self.cadastro = Some(resultados);
                self.geracao_leitor += 1;
                self.atualizar(&[
                    Consulta::Bibliotecas,
                    Consulta::Concluidos,
                    Consulta::Resumo,
                ]);
            }
            DadosBiblioteca::Homologacao { homologacao } => {
                self.terminou(AcaoBib::Homologar);
                let (tom, t) = match homologacao.estado.as_str() {
                    "aplicada" => (
                        Tom::Sucesso,
                        format!(
                            "Homologado: o livro entrou na Biblioteca Homologada (aprovado por {}).",
                            texto::texto_puro(
                                homologacao.aprovado_por.as_deref().unwrap_or("—"),
                                60
                            )
                        ),
                    ),
                    "falhou" => (
                        Tom::Perigo,
                        format!(
                            "A homologação falhou: {}",
                            texto::texto_puro(
                                homologacao.erro.as_deref().unwrap_or("sem detalhe"),
                                300
                            )
                        ),
                    ),
                    "proposta" => (
                        Tom::Info,
                        "Homologação apenas proposta; falta a aprovação.".to_string(),
                    ),
                    outro => (
                        Tom::Info,
                        format!("Homologação {}.", texto::texto_puro(outro, 40)),
                    ),
                };
                avisos.push((tom, t));
                self.homologacao = Some(homologacao);
                self.geracao_leitor += 1;
                self.atualizar(&[Consulta::Bibliotecas, Consulta::Perfil, Consulta::Resumo]);
            }
            DadosBiblioteca::Alertas {
                itens,
                nao_vistos_total,
                proximo_cursor,
            } => {
                let anexa = self.concluiu(prop, Consulta::Alertas);
                self.em_voo.remove(&Consulta::Alertas);
                let d = self.alertas.get_or_insert_with(DadosAlertas::default);
                if anexa {
                    d.itens.extend(itens);
                } else {
                    d.itens = itens;
                }
                d.nao_vistos_total = nao_vistos_total;
                d.proximo = proximo_cursor;
                self.alerta.geracao_leitor += 1;
            }
            DadosBiblioteca::Vistos { marcados } => {
                self.terminou(AcaoBib::MarcarVisto);
                avisos.push((
                    Tom::Sucesso,
                    format!(
                        "{} alerta(s) marcado(s) como visto(s).",
                        inteiro_pt_br(marcados)
                    ),
                ));
                self.alerta.selecionados.clear();
                self.atualizar(&[Consulta::Alertas, Consulta::Resumo]);
            }
            DadosBiblioteca::Resumo { resumo } => {
                self.em_voo.remove(&Consulta::Resumo);
                self.resumo = Some(resumo);
            }
            DadosBiblioteca::Espaco { relatorio } => {
                self.em_voo.remove(&Consulta::Espaco);
                self.espaco = Some(relatorio);
            }
            DadosBiblioteca::Config { config } => {
                let salvou = matches!(prop, Some(PropBib::Acao(AcaoBib::ConfigDefinir)));
                self.em_voo.remove(&Consulta::Config);
                self.terminou(AcaoBib::ConfigDefinir);
                if salvou {
                    avisos.push((
                        Tom::Sucesso,
                        "Configuração da Biblioteca salva.".to_string(),
                    ));
                }
                self.ia.carregar(&config);
                self.config_ui.carregar(&config);
                self.config = Some(config);
                if salvou {
                    self.atualizar(&[Consulta::Perfil]);
                }
            }
        }
        avisos
    }

    // ------------------------------------------------------------------ mensagens do job

    /// Mensagem de progresso do job Biblioteca (`biblioteca: ...`) ou alerta novo
    /// (`alerta: ...`). Devolve um aviso quando há um alerta novo para mostrar.
    pub fn mensagem(&mut self, texto_msg: &str) -> Option<AvisoBib> {
        if let Some(p) = texto::parse_progresso(texto_msg) {
            self.progresso = Some(p);
            return None;
        }
        let alerta = texto_msg.strip_prefix(texto::PREFIXO_ALERTA)?;
        let t = texto::texto_puro(alerta, 200);
        self.alertas_recentes.push(t.clone());
        let excesso = self.alertas_recentes.len().saturating_sub(5);
        self.alertas_recentes.drain(..excesso);
        self.atualizar(&[Consulta::Alertas, Consulta::Resumo]);
        Some((Tom::Info, format!("Biblioteca: {t}")))
    }

    /// O job Biblioteca terminou (de quem quer que seja).
    pub fn job_terminou(&mut self) {
        self.progresso = None;
        self.tudo_obsoleto();
        self.geracao_leitor += 1;
    }

    /// Enquanto um job roda, a fila e o resumo são relidos de tempos em tempos.
    pub fn refrescar_durante_job(&mut self, job_ativo: bool) {
        if !job_ativo {
            self.ultimo_refresco = None;
            return;
        }
        if self
            .ultimo_refresco
            .is_none_or(|t| t.elapsed() >= INTERVALO_JOB)
        {
            self.ultimo_refresco = Some(Instant::now());
            self.atualizar(&[Consulta::Fila, Consulta::Resumo]);
        }
    }

    /// Alertas ainda não vistos (do resumo ou da lista).
    #[must_use]
    pub fn nao_vistos(&self) -> Option<i64> {
        self.alertas
            .as_ref()
            .map(|a| a.nao_vistos_total)
            .or_else(|| self.resumo.as_ref().map(|r| r.alertas_nao_vistos))
    }
}

// ---------------------------------------------------------------------------------------------
// Ligação com o AppDag
// ---------------------------------------------------------------------------------------------

impl AppDag {
    /// Pede ao agente o que as páginas precisam e ainda não têm.
    pub fn bib_carregar(&mut self, cs: &[Consulta]) {
        for &c in cs {
            if self.biblioteca.precisa(c) {
                self.bib_pedir(c, None);
            }
        }
    }

    /// Envia a consulta `c` (com `cursor` = próxima página). `false` se não há como montá-la.
    pub fn bib_pedir(&mut self, c: Consulta, cursor: Option<String>) -> bool {
        let mais = cursor.is_some();
        let Some(cmd) = self.biblioteca.comando_da_consulta(c, cursor) else {
            return false;
        };
        // `Duplicatas` também grava candidatos e alertas: só o escritor pode pedir.
        if c == Consulta::Duplicatas && self.motivo_sem_escrita_config().is_some() {
            return false;
        }
        self.biblioteca.enviada(c);
        let prop = if mais {
            PropBib::Mais(c)
        } else {
            PropBib::Consulta(c)
        };
        self.enviar(cmd, Proposito::Biblioteca(prop));
        true
    }

    /// Envia uma escrita ou trabalho. `false` (com aviso) se não pode escrever agora.
    pub fn bib_acao(&mut self, acao: AcaoBib, cmd: Comando) -> bool {
        if let Some(m) = self.motivo_sem_escrita_config() {
            self.aviso(Tom::Aviso, m);
            return false;
        }
        if self.biblioteca.acao_em_voo(acao) {
            return false;
        }
        self.biblioteca.acao_enviada(acao);
        self.enviar(cmd, Proposito::Biblioteca(PropBib::Acao(acao)));
        true
    }

    /// Há um job Biblioteca rodando (no agente, de qualquer cliente) ou pedido por esta janela?
    #[must_use]
    pub fn bib_job_ativo(&self) -> bool {
        self.worker
            .jobs()
            .iter()
            .any(|j| j.tipo == TipoJob::Biblioteca)
            || self.biblioteca.trabalho_em_voo()
    }

    /// O job Biblioteca do agente, se há um.
    #[must_use]
    pub fn bib_job(&self) -> Option<&dag_core::JobInfo> {
        self.worker
            .jobs()
            .iter()
            .find(|j| j.tipo == TipoJob::Biblioteca)
    }

    /// Chamado a cada quadro de uma página da seção.
    pub fn bib_quadro(&mut self, ctx: &egui::Context) {
        let ativo = self.bib_job_ativo();
        self.biblioteca.refrescar_durante_job(ativo);
        if ativo {
            ctx.request_repaint_after(Duration::from_millis(1000));
        }
    }

    /// Segmento da barra de status: o job Biblioteca e os alertas não vistos.
    #[must_use]
    pub fn bib_segmento_de_status(&self) -> Option<bancada_casca::SegmentoStatus> {
        use bancada_casca::SegmentoStatus;
        if self.bib_job_ativo() {
            let t = match &self.biblioteca.progresso {
                Some(p) if p.itens_total > 0 => format!(
                    "Biblioteca: {}/{} itens",
                    inteiro_pt_br(p.itens_feitos),
                    inteiro_pt_br(p.itens_total)
                ),
                _ => "Biblioteca: trabalhando…".to_string(),
            };
            let dica = self
                .biblioteca
                .progresso
                .as_ref()
                .map_or_else(String::new, ProgressoBiblioteca::texto);
            return Some(
                SegmentoStatus::novo("biblioteca", t)
                    .tom(Tom::Info)
                    .dica(dica),
            );
        }
        match self.biblioteca.nao_vistos() {
            Some(n) if n > 0 => Some(
                SegmentoStatus::novo("biblioteca", format!("Biblioteca: {n} alerta(s) novo(s)"))
                    .tom(Tom::Aviso)
                    .dica("Veja em Biblioteca > Alertas e Resumo"),
            ),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Ações do Ribbon
// ---------------------------------------------------------------------------------------------

/// Uma única aba do Ribbon reúne as Ações de todas as páginas da seção.
pub const ABA_RIBBON: &str = "biblioteca";
pub const ROTULO_ABA: &str = "Biblioteca";
pub const KEYTIP_ABA: &str = "B";

/// Grupos do Ribbon da seção (os das páginas, na ordem do menu).
#[must_use]
pub fn grupos_do_ribbon() -> Vec<&'static DefGrupo> {
    todas().iter().flat_map(|p| p.grupos.iter()).collect()
}

/// Por que a Ação está desabilitada agora; `None` = habilitada.
#[must_use]
pub fn motivo_desabilitada(app: &AppDag, id: &str) -> Option<String> {
    let sem_escrita = || app.motivo_sem_escrita_config().map(str::to_string);
    let b = &app.biblioteca;
    match id {
        "biblioteca.sincronizar" => sem_escrita()
            .or_else(|| {
                b.homologada()
                    .is_none()
                    .then(|| "Registre a Biblioteca Homologada primeiro".into())
            })
            .or_else(|| {
                app.bib_job_ativo()
                    .then(|| "Há um trabalho da Biblioteca em andamento".into())
            }),
        "biblioteca.enfileirar" => sem_escrita()
            .or_else(|| {
                texto::raizes_de_texto(&b.visao.raizes)
                    .is_empty()
                    .then(|| "Digite ao menos uma raiz (caminho absoluto) na Visão Geral".into())
            })
            .or_else(|| {
                app.bib_job_ativo()
                    .then(|| "Há um trabalho da Biblioteca em andamento".into())
            }),
        "biblioteca.processar" => sem_escrita().or_else(|| {
            app.bib_job_ativo()
                .then(|| "Há um trabalho da Biblioteca em andamento".into())
        }),
        "biblioteca.duplicatas_buscar" => sem_escrita().or_else(|| {
            b.homologada()
                .is_none()
                .then(|| "Registre e sincronize a Biblioteca Homologada primeiro".into())
        }),
        "biblioteca.revisao_aceitar_altas" => sem_escrita().or_else(|| {
            b.revisao
                .as_ref()
                .is_none_or(|r| comandos::aceitar_confianca_alta(&r.itens).is_empty())
                .then(|| "Nenhum campo pendente de confiança alta na lista carregada".into())
        }),
        "biblioteca.revisao_desfazer" => sem_escrita().or_else(|| {
            b.ultimo_lote
                .is_none()
                .then(|| "Nenhum lote de decisões nesta sessão para desfazer".into())
        }),
        "biblioteca.cadastrar_prontos" => sem_escrita().or_else(|| {
            (b.classificar.prontos(b).is_empty())
                .then(|| "Nenhum livro pronto para cadastrar (processe e revise a fila)".into())
        }),
        "biblioteca.alertas_marcar_todos" => sem_escrita().or_else(|| {
            b.alertas
                .as_ref()
                .is_none_or(|a| a.itens.iter().all(|i| i.visto_em.is_some()))
                .then(|| "Nenhum alerta novo na lista".into())
        }),
        "biblioteca.job_pausar" | "biblioteca.job_retomar" | "biblioteca.job_cancelar" => app
            .bib_job()
            .is_none()
            .then(|| "Nenhum trabalho da Biblioteca em andamento".into()),
        _ => None,
    }
}

/// Executa uma Ação do Ribbon da seção. `true` se a Ação é da seção.
pub fn executar_acao(app: &mut AppDag, ctx: &egui::Context, id: &str) -> bool {
    if !id.starts_with("biblioteca.") {
        return false;
    }
    if let Some(m) = motivo_desabilitada(app, id) {
        app.aviso(Tom::Aviso, m);
        return true;
    }
    match id {
        "biblioteca.atualizar" | "biblioteca.classificar_atualizar" => {
            app.biblioteca.tudo_obsoleto();
            app.biblioteca.geracao_leitor += 1;
        }
        "biblioteca.sincronizar" => visao_geral::sincronizar(app),
        "biblioteca.enfileirar" => visao_geral::enfileirar(app),
        "biblioteca.processar" => visao_geral::processar(app),
        "biblioteca.duplicatas_buscar" => duplicatas::buscar(app),
        "biblioteca.revisao_aceitar_altas" => revisao::aceitar_altas(app),
        "biblioteca.revisao_desfazer" => revisao::desfazer_ultimo(app),
        "biblioteca.revisao_atualizar" => app
            .biblioteca
            .atualizar(&[Consulta::Revisao, Consulta::Resumo]),
        "biblioteca.cadastrar_prontos" => a_classificar::cadastrar_prontos(app),
        "biblioteca.perfil_atualizar" => {
            app.biblioteca
                .atualizar(&[Consulta::Bibliotecas, Consulta::Perfil]);
        }
        "biblioteca.alertas_marcar_todos" => alertas::marcar_todos(app),
        "biblioteca.alertas_atualizar" => {
            app.biblioteca
                .atualizar(&[Consulta::Alertas, Consulta::Resumo]);
        }
        "biblioteca.ia_recarregar" | "biblioteca.config_recarregar" => {
            app.biblioteca.atualizar(&[Consulta::Config]);
        }
        "biblioteca.job_pausar" => app.worker.pausar(),
        "biblioteca.job_retomar" => app.worker.continuar(),
        "biblioteca.job_cancelar" => {
            if let Some(j) = app.bib_job().map(|j| j.id) {
                app.worker.cancelar_job(j);
            }
        }
        "biblioteca.monitor_atualizar" => {
            app.biblioteca
                .atualizar(&[Consulta::Fila, Consulta::Excecoes, Consulta::Resumo]);
        }
        _ => {}
    }
    let _ = ctx;
    true
}

// ---------------------------------------------------------------------------------------------
// Peças de tela compartilhadas
// ---------------------------------------------------------------------------------------------

/// Moldura de uma seção da página.
pub fn quadro<R>(ui: &mut egui::Ui, titulo: &str, corpo: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(titulo);
            ui.add_space(2.0);
            corpo(ui)
        })
        .inner
}

/// Linha "Carregando…" ou o erro da consulta (com a orientação, se há).
pub fn estado_da_consulta(ui: &mut egui::Ui, b: &Estado, c: Consulta) {
    if let Some((codigo, msg)) = b.erros.get(&c) {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("Atenção: {} [{codigo}]", texto::texto_puro(msg, 300)),
        );
        if let Some(o) = orientacao_do_erro(codigo) {
            ui.weak(o);
        }
    } else if b.carregando(c) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.weak("Consultando o agente…");
        });
    }
}

/// Erro da última ação (com orientação), sem popup.
pub fn erro_da_acao(ui: &mut egui::Ui, b: &Estado, acoes: &[AcaoBib]) {
    if let Some((a, codigo, msg)) = &b.erro_acao
        && acoes.contains(a)
    {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Erro: {} [{codigo}]", texto::texto_puro(msg, 400)),
        );
        if let Some(o) = orientacao_do_erro(codigo) {
            ui.label(o);
        }
    }
}

/// O job Biblioteca: barra de progresso, contadores e Pausar/Retomar/Cancelar. Nada de popup.
pub fn progresso_do_job(ui: &mut egui::Ui, app: &mut AppDag) {
    let job = app.bib_job().cloned();
    let ativo = app.bib_job_ativo();
    if !ativo {
        ui.weak("Nenhum trabalho da Biblioteca em andamento.");
        return;
    }
    if let Some(j) = &job {
        ui.label(format!(
            "{} — {} (iniciado por {})",
            j.tipo.rotulo(),
            j.estado.rotulo(),
            j.origem.rotulo()
        ));
    } else {
        ui.label("Trabalho da Biblioteca pedido por esta janela…");
    }
    match app.biblioteca.progresso.clone() {
        Some(p) => {
            let barra = match texto::fracao_do_progresso(&p) {
                Some(f) => egui::ProgressBar::new(f).text(format!(
                    "{} de {} itens",
                    inteiro_pt_br(p.itens_feitos),
                    inteiro_pt_br(p.itens_total)
                )),
                None => egui::ProgressBar::new(0.0)
                    .animate(true)
                    .text(format!("{} itens", inteiro_pt_br(p.itens_feitos))),
            };
            ui.add(barra);
            ui.weak(format!(
                "{} lidos · {} páginas ({} com OCR) · {} duplicata(s) exata(s) · {} potenciais · \
                 {} falha(s) · {} exceção(ões)",
                bytes_humanos(p.bytes),
                inteiro_pt_br(p.paginas),
                inteiro_pt_br(p.paginas_ocr),
                inteiro_pt_br(p.duplicatas_exatas),
                bytes_humanos(p.espaco_potencial_bytes),
                inteiro_pt_br(p.falhas),
                inteiro_pt_br(p.excecoes),
            ));
            if let Some(i) = &p.item_atual {
                ui.weak(format!("Agora: {}", texto::texto_puro(i, 160)));
            }
        }
        None => {
            ui.add(
                egui::ProgressBar::new(0.0)
                    .animate(true)
                    .text("trabalhando…"),
            );
        }
    }
    ui.horizontal(|ui| {
        let tem_job = job.is_some();
        let pausado = job
            .as_ref()
            .is_some_and(|j| j.estado == dag_core::EstadoJob::Pausado);
        if pausado {
            if ui
                .add_enabled(tem_job, egui::Button::new("Retomar"))
                .clicked()
            {
                app.worker.continuar();
            }
        } else if ui
            .add_enabled(tem_job, egui::Button::new("Pausar"))
            .clicked()
        {
            app.worker.pausar();
        }
        if ui
            .add_enabled(tem_job, egui::Button::new("Cancelar"))
            .on_hover_text("Termina no próximo item; o que já foi feito fica gravado")
            .clicked()
            && let Some(j) = &job
        {
            app.worker.cancelar_job(j.id);
        }
    });
}

/// Definição compartilhada de uma Ação de página.
pub(crate) const fn acao(id: &'static str, rotulo: &'static str, dica: &'static str) -> DefAcao {
    DefAcao { id, rotulo, dica }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_core::{
        AlertaInfo, ConfigBiblioteca, ItemFilaInfo, LivroRef, MotivoFila, ResumoBiblioteca,
    };

    fn biblioteca(id: i64, papel: PapelRegistro) -> BibliotecaInfo {
        BibliotecaInfo {
            id,
            caminho: format!("/lib/{id}"),
            papel,
            nome: format!("b{id}"),
            estado: "ativa".into(),
            versao_calibre: None,
            library_uuid: None,
            livros: 10,
            formatos: 12,
            ultima_sincronizacao: Some(5),
            ultimo_resultado: None,
        }
    }

    fn livro() -> LivroRef {
        LivroRef {
            id: 1,
            calibre_id: 1,
            titulo: "T".into(),
            autores: vec![],
            isbn13: None,
            editora: None,
            idioma: None,
            estante: None,
            tags: vec![],
            caminho: "/lib/1/t".into(),
        }
    }

    fn grupo(bytes: u64, sug: u64) -> GrupoDuplicata {
        GrupoDuplicata {
            livro: livro(),
            formatos_homologados: vec![],
            copias: vec![],
            bytes_totais: bytes,
            bytes_sugeridos: sug,
        }
    }

    #[test]
    fn todas_as_consultas_comecam_pendentes_e_viram_em_voo() {
        let mut e = Estado::default();
        for c in Consulta::TODAS {
            assert!(e.precisa(c), "{c:?}");
        }
        e.enviada(Consulta::Resumo);
        assert!(!e.precisa(Consulta::Resumo));
        assert!(e.carregando(Consulta::Resumo));
        // Marcada de novo durante o voo: só relê depois da resposta.
        e.atualizar(&[Consulta::Resumo]);
        assert!(!e.precisa(Consulta::Resumo));
        e.receber(
            DadosBiblioteca::Resumo {
                resumo: ResumoBiblioteca::default(),
            },
            Some(PropBib::Consulta(Consulta::Resumo)),
        );
        assert!(e.precisa(Consulta::Resumo));
    }

    #[test]
    fn perfil_espera_a_lista_de_bibliotecas() {
        let mut e = Estado::default();
        assert!(e.comando_da_consulta(Consulta::Perfil, None).is_none());
        e.receber(
            DadosBiblioteca::Bibliotecas {
                itens: vec![
                    biblioteca(4, PapelRegistro::AClassificar),
                    biblioteca(7, PapelRegistro::Homologada),
                ],
            },
            Some(PropBib::Consulta(Consulta::Bibliotecas)),
        );
        match e.comando_da_consulta(Consulta::Perfil, None) {
            Some(Comando::BibliotecaPerfil { biblioteca }) => assert_eq!(biblioteca, "7"),
            o => panic!("{o:?}"),
        }
        assert_eq!(e.a_classificar().unwrap().id, 4);
    }

    #[test]
    fn duplicatas_anexam_na_proxima_pagina_e_somam_bytes() {
        let mut e = Estado::default();
        let pagina = |gs: Vec<GrupoDuplicata>, prox: Option<&str>| DadosBiblioteca::Duplicatas {
            grupos: gs,
            bytes_totais_pagina: 0,
            bytes_sugeridos_pagina: 0,
            alertas_emitidos: 0,
            proximo_cursor: prox.map(String::from),
        };
        e.enviada(Consulta::Duplicatas);
        e.receber(
            pagina(vec![grupo(100, 60), grupo(50, 0)], Some("c")),
            Some(PropBib::Consulta(Consulta::Duplicatas)),
        );
        let d = e.duplicatas.as_ref().unwrap();
        assert_eq!((d.bytes_totais(), d.bytes_sugeridos()), (150, 60));
        e.receber(
            pagina(vec![grupo(30, 30)], None),
            Some(PropBib::Mais(Consulta::Duplicatas)),
        );
        let d = e.duplicatas.as_ref().unwrap();
        assert_eq!(d.grupos.len(), 3);
        assert_eq!((d.bytes_totais(), d.bytes_sugeridos()), (180, 90));
        assert_eq!(d.proximo, None);
        // Uma nova busca substitui.
        e.receber(
            pagina(vec![grupo(1, 1)], None),
            Some(PropBib::Consulta(Consulta::Duplicatas)),
        );
        assert_eq!(e.duplicatas.as_ref().unwrap().grupos.len(), 1);
        assert!(!e.carregando(Consulta::Duplicatas));
    }

    #[test]
    fn fila_vai_para_o_campo_certo_pelo_proposito() {
        let mut e = Estado::default();
        let item = |id: i64, estado: EstadoFilaInfo| ItemFilaInfo {
            id,
            file_id: id,
            caminho: Some(format!("/x/{id}.pdf")),
            motivo: MotivoFila::Novo,
            prioridade: 0,
            estado,
            etapa: None,
            tentativas: 0,
            erro: None,
            enfileirado_em: 0,
        };
        let resposta = |itens: Vec<ItemFilaInfo>| DadosBiblioteca::Fila {
            contagem: ContagemFila {
                pendentes: 1,
                ..ContagemFila::default()
            },
            itens,
            proximo_cursor: None,
        };
        e.enviada(Consulta::Concluidos);
        e.receber(
            resposta(vec![item(1, EstadoFilaInfo::Concluido)]),
            Some(PropBib::Consulta(Consulta::Concluidos)),
        );
        e.enviada(Consulta::Excecoes);
        e.receber(
            resposta(vec![item(2, EstadoFilaInfo::Excecao)]),
            Some(PropBib::Consulta(Consulta::Excecoes)),
        );
        e.enviada(Consulta::Fila);
        e.receber(
            resposta(vec![item(3, EstadoFilaInfo::Pendente)]),
            Some(PropBib::Consulta(Consulta::Fila)),
        );
        assert_eq!(e.concluidos.as_ref().unwrap().itens[0].file_id, 1);
        assert_eq!(e.excecoes.as_ref().unwrap().itens[0].file_id, 2);
        assert_eq!(e.fila.as_ref().unwrap().itens[0].file_id, 3);
        assert_eq!(e.fila.as_ref().unwrap().contagem.pendentes, 1);
        for c in [Consulta::Fila, Consulta::Concluidos, Consulta::Excecoes] {
            assert!(!e.carregando(c));
        }
    }

    #[test]
    fn calibre_ocupado_guarda_o_bloqueio_e_orienta() {
        let mut e = Estado::default();
        e.acao_enviada(AcaoBib::Cadastrar);
        let av = e.falhou(
            PropBib::Acao(AcaoBib::Cadastrar),
            "calibre_ocupado",
            "biblioteca em uso",
        );
        assert_eq!(e.bloqueio_calibre.as_deref(), Some("biblioteca em uso"));
        assert!(!e.acao_em_voo(AcaoBib::Cadastrar));
        assert!(av.iter().any(|(_, t)| t.contains("Feche o Calibre")));
        // Um cadastro parcial também guarda o bloqueio e mantém o que foi feito.
        e.acao_enviada(AcaoBib::Cadastrar);
        let av = e.receber(
            DadosBiblioteca::Cadastrada {
                resultados: vec![
                    ResultadoCadastro {
                        file_id: 1,
                        estado: "cadastrado".into(),
                        calibre_id: Some(9),
                        erro: None,
                    },
                    ResultadoCadastro {
                        file_id: 2,
                        estado: "ja_existe_na_homologada".into(),
                        calibre_id: None,
                        erro: None,
                    },
                ],
                bloqueado: Some("servidor servindo".into()),
            },
            Some(PropBib::Acao(AcaoBib::Cadastrar)),
        );
        assert_eq!(e.bloqueio_calibre.as_deref(), Some("servidor servindo"));
        assert_eq!(av[0].0, Tom::Aviso);
        assert!(av[0].1.contains("1 cadastrado(s)"));
        assert!(av[0].1.contains("Calibre está ocupado"));
        // Cadastro limpo apaga o bloqueio.
        e.receber(
            DadosBiblioteca::Cadastrada {
                resultados: vec![],
                bloqueado: None,
            },
            Some(PropBib::Acao(AcaoBib::Cadastrar)),
        );
        assert_eq!(e.bloqueio_calibre, None);
    }

    #[test]
    fn decisao_guarda_o_lote_para_desfazer() {
        let mut e = Estado::default();
        e.acao_enviada(AcaoBib::Decidir);
        let av = e.receber(
            DadosBiblioteca::Decidida {
                lote: "lote-1".into(),
                aplicadas: 3,
                ignoradas: vec!["x: sem sugestão".into()],
            },
            Some(PropBib::Acao(AcaoBib::Decidir)),
        );
        assert_eq!(e.ultimo_lote.as_deref(), Some("lote-1"));
        assert_eq!(av[0].0, Tom::Aviso);
        assert!(av[0].1.contains("1 ignorada(s)"));
        e.receber(
            DadosBiblioteca::Desfeita {
                lote: "lote-1".into(),
                desfeitas: 3,
            },
            Some(PropBib::Acao(AcaoBib::Desfazer)),
        );
        assert_eq!(e.ultimo_lote, None);
        // Lote sem nada aplicado não vira "último lote".
        e.receber(
            DadosBiblioteca::Decidida {
                lote: "vazio".into(),
                aplicadas: 0,
                ignoradas: vec![],
            },
            Some(PropBib::Acao(AcaoBib::Decidir)),
        );
        assert_eq!(e.ultimo_lote, None);
    }

    #[test]
    fn sincronizacao_com_diferencas_e_perigo() {
        let mut e = Estado::default();
        let resumo = |intacta: bool| ResumoSincronizacao {
            biblioteca: 1,
            livros: 10,
            removidos: 0,
            formatos: 12,
            formatos_ligados_ao_catalogo: 5,
            hashes_importados: 4,
            hashes_calculados: 0,
            formatos_sem_arquivo_no_catalogo: 0,
            formatos_sem_hash: 0,
            comparacao_metadata_db: "igual".into(),
            biblioteca_intacta: intacta,
            diferencas: vec![],
            duracao_ms: 5,
        };
        let av = e.receber(
            DadosBiblioteca::Sincronizada {
                resumo: resumo(true),
            },
            Some(PropBib::Acao(AcaoBib::Sincronizar)),
        );
        assert_eq!(av[0].0, Tom::Sucesso);
        let av = e.receber(
            DadosBiblioteca::Sincronizada {
                resumo: resumo(false),
            },
            Some(PropBib::Acao(AcaoBib::Sincronizar)),
        );
        assert_eq!(av[0].0, Tom::Perigo);
        assert!(e.precisa(Consulta::Duplicatas));
    }

    #[test]
    fn mensagens_do_job_viram_progresso_e_alerta() {
        let mut e = Estado::default();
        let p = ProgressoBiblioteca {
            itens_feitos: 3,
            itens_total: 9,
            ..ProgressoBiblioteca::default()
        };
        assert!(e.mensagem(&p.texto()).is_none());
        assert_eq!(e.progresso.as_ref().unwrap().itens_feitos, 3);
        let a = e
            .mensagem("alerta: você já tem este livro — O\u{202E}Livro")
            .unwrap();
        assert_eq!(a.0, Tom::Info);
        assert!(a.1.contains("OLivro"), "{}", a.1);
        assert_eq!(e.alertas_recentes.len(), 1);
        assert!(e.mensagem("qualquer outra coisa").is_none());
        e.job_terminou();
        assert!(e.progresso.is_none());
    }

    #[test]
    fn trabalho_em_voo_segue_as_acoes_longas_e_a_busca_de_duplicatas() {
        let mut e = Estado::default();
        assert!(!e.trabalho_em_voo());
        e.acao_enviada(AcaoBib::Decidir);
        assert!(!e.trabalho_em_voo(), "decidir é curto");
        e.acao_enviada(AcaoBib::Processar);
        assert!(e.trabalho_em_voo());
        e.receber(
            DadosBiblioteca::Processada {
                resumo: ResumoProcessamento::default(),
            },
            Some(PropBib::Acao(AcaoBib::Processar)),
        );
        assert!(!e.trabalho_em_voo());
        e.enviada(Consulta::Duplicatas);
        assert!(e.trabalho_em_voo());
    }

    #[test]
    fn config_recebida_so_avisa_quando_foi_salva() {
        let mut e = Estado::default();
        let cfg = ConfigBiblioteca::default();
        let av = e.receber(
            DadosBiblioteca::Config {
                config: cfg.clone(),
            },
            Some(PropBib::Consulta(Consulta::Config)),
        );
        assert!(av.is_empty());
        e.acao_enviada(AcaoBib::ConfigDefinir);
        let av = e.receber(
            DadosBiblioteca::Config { config: cfg },
            Some(PropBib::Acao(AcaoBib::ConfigDefinir)),
        );
        assert_eq!(av.len(), 1);
        assert!(e.config.is_some());
    }

    #[test]
    fn alertas_anexam_e_contam_nao_vistos() {
        let mut e = Estado::default();
        let a = |id: i64| AlertaInfo {
            id,
            chave: None,
            tipo: "ja_tem_este_livro".into(),
            file_id: Some(id),
            dados: serde_json::json!({"livro": "L"}),
            criado_em: 1,
            visto_em: None,
            silenciado: false,
        };
        e.receber(
            DadosBiblioteca::Alertas {
                itens: vec![a(1), a(2)],
                nao_vistos_total: 5,
                proximo_cursor: Some("p".into()),
            },
            Some(PropBib::Consulta(Consulta::Alertas)),
        );
        e.receber(
            DadosBiblioteca::Alertas {
                itens: vec![a(3)],
                nao_vistos_total: 4,
                proximo_cursor: None,
            },
            Some(PropBib::Mais(Consulta::Alertas)),
        );
        let d = e.alertas.as_ref().unwrap();
        assert_eq!(d.itens.len(), 3);
        assert_eq!(e.nao_vistos(), Some(4));
    }

    #[test]
    fn orientacoes_cobrem_os_codigos_do_protocolo() {
        for c in [
            "calibre_ocupado",
            "calibredb_ausente",
            "aprovacao_humana_exigida",
            "aprovacao_invalida",
            "biblioteca_nao_registrada",
            "sem_homologada",
            "sem_a_classificar",
            "cadastro_inexistente",
            "cadastro_nao_pronto",
            "ja_existe_na_homologada",
            "config_invalida",
            "papel_invalido",
        ] {
            assert!(orientacao_do_erro(c).is_some(), "{c}");
        }
        assert!(orientacao_do_erro("outro").is_none());
    }

    #[test]
    fn as_paginas_tem_ids_unicos_e_acoes() {
        let mut ids = HashSet::new();
        let mut acoes = HashSet::new();
        for p in todas() {
            assert!(ids.insert(p.id), "{}", p.id);
            assert!(p.id.starts_with("biblioteca"));
            assert!(p.acoes().next().is_some(), "{} sem ações", p.id);
            for a in p.acoes() {
                assert!(acoes.insert(a.id), "ação repetida {}", a.id);
                assert!(a.id.starts_with("biblioteca."));
            }
        }
        assert_eq!(ids.len(), 9);
    }
}
