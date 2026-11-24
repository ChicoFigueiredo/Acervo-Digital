//! Página Configurações.

use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga};
use crate::app::AppDag;
use crate::leitor::{Carga, Leitor, erro};
use crate::worker::ModoWorker;
use bancada_casca::egui;
use dag_agent::servico as srv;
use dag_core::{AcaoServico, Resposta};
use dag_core::{
    MAX_DISCOS_MAX, MAX_DISCOS_PADRAO, OpcoesHash, THREADS_POR_DISCO_MAX, THREADS_POR_DISCO_PADRAO,
};
use dag_platform_linux::servico::EstadoServico;
use dag_store::configuracao;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// Resultado de uma tarefa do serviço feita fora da thread da interface.
enum Msg {
    Estado(Result<EstadoServico, String>),
    Acao(Box<Resposta>),
}

/// O serviço em segundo plano (`dag-agente.service`) visto pela GUI: estado consultado e ações
/// disparadas em threads (systemctl pode demorar; a entrega do Catálogo espera a parada limpa).
#[derive(Default)]
pub struct ServicoGui {
    pub estado: Option<Result<EstadoServico, String>>,
    rx: Option<Receiver<Msg>>,
    ocupado: Option<&'static str>,
    mensagem: Option<(bool, String)>,
    consultado_em: Option<Instant>,
    /// O aviso da página Início foi dispensado nesta sessão.
    pub dispensado: bool,
}

impl std::fmt::Debug for ServicoGui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServicoGui").finish_non_exhaustive()
    }
}

impl ServicoGui {
    /// O serviço está instalado? `None` enquanto não se sabe.
    #[must_use]
    pub fn instalado(&self) -> Option<bool> {
        match &self.estado {
            Some(Ok(e)) => Some(e.instalado),
            _ => None,
        }
    }

    /// Recolhe resultados e consulta o estado de tempos em tempos.
    pub fn atualizar(&mut self, catalogo: &Path, ctx: &egui::Context) {
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Msg::Estado(e)) => {
                    self.estado = Some(e);
                    self.rx = None;
                    self.ocupado = None;
                }
                Ok(Msg::Acao(r)) => {
                    self.mensagem = Some(match *r {
                        Resposta::Ok { mensagem } => (true, mensagem.unwrap_or_default()),
                        Resposta::Erro { mensagem, .. } => (false, mensagem),
                        o => (true, format!("{o:?}")),
                    });
                    self.rx = None;
                    self.ocupado = None;
                    self.consultado_em = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.ocupado = None;
                }
            }
        }
        let velho = self
            .consultado_em
            .is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if velho && self.rx.is_none() {
            self.consultado_em = Some(Instant::now());
            let c = catalogo.to_path_buf();
            let (tx, rx) = channel();
            self.rx = Some(rx);
            std::thread::spawn(move || {
                let r = srv::gerenciador_real(&c)
                    .map_err(|e| e.to_string())
                    .and_then(|g| g.estado().map_err(|e| e.to_string()));
                let _ = tx.send(Msg::Estado(r));
            });
        }
    }

    fn disparar(&mut self, rotulo: &'static str, acao: AcaoServico, catalogo: PathBuf) {
        if self.rx.is_some() {
            return;
        }
        self.ocupado = Some(rotulo);
        self.mensagem = None;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Acao(Box::new(srv::executar(acao, &catalogo, None))));
        });
    }
}

/// Resumo de uma linha do estado, para a página e para testes.
#[must_use]
pub fn resumo_do_estado(e: &EstadoServico) -> String {
    if !e.instalado {
        return "não instalado".into();
    }
    let mut p = vec![
        "instalado".to_string(),
        e.atividade.rotulo().to_string(),
        if e.autostart {
            "inicia no login".into()
        } else {
            "não inicia no login".into()
        },
    ];
    p.push(match e.linger {
        Some(true) => "sobe no boot (linger)".into(),
        Some(false) => "sem linger".into(),
        None => "linger desconhecido".into(),
    });
    if let Some(pid) = e.main_pid.filter(|p| *p != 0) {
        p.push(format!("pid {pid}"));
    }
    p.join(", ")
}

/// Seção "Serviço em segundo plano" da página Configurações.
fn secao_servico(ui: &mut egui::Ui, app: &mut AppDag) {
    ui.heading("Serviço em segundo plano");
    let ctx = ui.ctx().clone();
    let catalogo = app.caminho.clone();
    app.configuracoes.servico.atualizar(&catalogo, &ctx);
    let s = &mut app.configuracoes.servico;
    match &s.estado {
        None => {
            ui.weak("consultando o systemd…");
        }
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().warn_fg_color, format!("indisponível: {e}"));
        }
        Some(Ok(e)) => {
            ui.label(format!("Estado: {}", resumo_do_estado(e)));
            ui.label(format!("Catálogo: {}", catalogo.display()));
        }
    }
    let (instalado, ativo, boot) = match &s.estado {
        Some(Ok(e)) => (
            e.instalado,
            e.atividade == dag_platform_linux::servico::Atividade::Ativo,
            e.linger == Some(true),
        ),
        _ => (false, false, false),
    };
    let livre = s.ocupado.is_none();
    let mut pedido: Option<(&'static str, AcaoServico)> = None;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(livre, egui::Button::new("Instalar e ativar"))
            .clicked()
        {
            pedido = Some(("instalando", AcaoServico::InstalarEAtivar { boot }));
        }
        if ui
            .add_enabled(livre && instalado && !ativo, egui::Button::new("Iniciar"))
            .clicked()
        {
            pedido = Some(("iniciando", AcaoServico::Iniciar));
        }
        if ui
            .add_enabled(livre && instalado && ativo, egui::Button::new("Parar"))
            .clicked()
        {
            pedido = Some(("parando", AcaoServico::Parar));
        }
        if ui
            .add_enabled(livre && instalado, egui::Button::new("Remover"))
            .clicked()
        {
            pedido = Some(("removendo", AcaoServico::Remover));
        }
    });
    let mut linger = boot;
    if ui
        .add_enabled(
            livre && instalado,
            egui::Checkbox::new(&mut linger, "Manter ativo desde o boot, sem login (linger)"),
        )
        .changed()
    {
        pedido = Some((
            "ajustando o boot",
            AcaoServico::PersistirSemSessao { ativo: linger },
        ));
    }
    if let Some(r) = s.ocupado {
        ui.weak(format!(
            "{r}… (pode levar um tempo se um hash está sendo entregue ao serviço)"
        ));
    }
    if let Some((ok, m)) = &s.mensagem {
        if *ok {
            ui.label(m);
        } else {
            ui.colored_label(ui.visuals().error_fg_color, m);
        }
    }
    ui.weak(
        "Com o serviço ativo, o catálogo continua sendo atualizado (monitor, retomada, hash) \
         mesmo com o aplicativo fechado. Instalar encerra com limpeza um agente que o aplicativo \
         tenha lançado e o serviço retoma o trabalho de onde parou.",
    );
    if let Some((rotulo, acao)) = pedido {
        s.disparar(rotulo, acao, catalogo);
    }
}

/// Aviso não bloqueante da página Início, enquanto o serviço não está instalado.
pub fn aviso_de_servico(ui: &mut egui::Ui, app: &mut AppDag) {
    let ctx = ui.ctx().clone();
    let catalogo = app.caminho.clone();
    let s = &mut app.configuracoes.servico;
    s.atualizar(&catalogo, &ctx);
    if s.dispensado || s.instalado() != Some(false) {
        return;
    }
    let mut instalar = false;
    let mut dispensar = false;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label("Para manter o catálogo atualizado mesmo com o app fechado, instale o serviço.");
        ui.horizontal(|ui| {
            instalar = ui
                .add_enabled(s.ocupado.is_none(), egui::Button::new("Instalar e ativar"))
                .clicked();
            dispensar = ui.button("Agora não").clicked();
        });
    });
    if instalar {
        s.disparar(
            "instalando",
            AcaoServico::InstalarEAtivar { boot: false },
            catalogo,
        );
    }
    if dispensar {
        s.dispensado = true;
    }
}

/// Estado da página: o que está gravado no Catálogo e o que o usuário está editando.
#[derive(Debug)]
pub struct Estado {
    pub gravado: Carga<OpcoesHash>,
    vista: u64,
    /// Valores nos campos de edição.
    pub threads_por_disco: u32,
    pub max_discos: u32,
    /// Os campos já receberam o valor gravado (não sobrescreve a edição em curso).
    inicializado: bool,
    /// O serviço de segundo plano (systemd de usuário).
    pub servico: ServicoGui,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            gravado: Carga::default(),
            vista: 0,
            threads_por_disco: THREADS_POR_DISCO_PADRAO,
            max_discos: MAX_DISCOS_PADRAO,
            inicializado: false,
            servico: ServicoGui::default(),
        }
    }
}

impl Estado {
    /// Recolhe a leitura e a dispara quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.inicializado = false;
            self.gravado
                .pedir(leitor.consultar(|c| configuracao::ler_hash(c).map_err(erro)));
        }
        if self.gravado.atualizar()
            && !self.inicializado
            && let Some(o) = self.gravado.dado
        {
            let p = o.resolver().unwrap_or_default();
            self.threads_por_disco = p.threads_por_disco;
            self.max_discos = p.max_discos;
            self.inicializado = true;
        }
    }
}

pub static PAGINA: DefPagina = DefPagina {
    id: "configuracoes",
    rotulo: "Configurações",
    keytip: "C",
    icone: "configuracoes",
    grupos: &[
        DefGrupo {
            id: "configuracoes.servico",
            rotulo: "Serviço",
            acoes: &[DefAcao {
                id: "configuracoes.servico",
                rotulo: "Serviço opcional",
                dica: "Instala ou remove o serviço de segundo plano (opt-in)",
            }],
        },
        DefGrupo {
            id: "configuracoes.varredura",
            rotulo: "Varredura",
            acoes: &[
                DefAcao {
                    id: "configuracoes.limites",
                    rotulo: "Scan e limites",
                    dica: "Limites de recursos da varredura",
                },
                DefAcao {
                    id: "configuracoes.permissoes",
                    rotulo: "Permissões",
                    dica: "Níveis de permissão; elevação só sob demanda via Polkit",
                },
                DefAcao {
                    id: "configuracoes.exclusoes",
                    rotulo: "Política de exclusão",
                    dica: "Política padrão de exclusões",
                },
            ],
        },
    ],
    resumo: "Serviço opcional, limites de varredura, permissões, observadores e preferências.",
    desenhar,
};

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.configuracoes.atualizar(&app.leitor, app.geracao);
    informacoes_da_sessao(ui, app);
    ui.add_space(8.0);
    secao_servico(ui, app);
    ui.add_space(8.0);
    hash_em_paralelo(ui, app);
    ui.add_space(8.0);
    ui.label("Limites de varredura, permissões e política de exclusão chegam em incrementos posteriores.");
    ui.weak("Nenhum serviço é instalado nem ativado sem o seu comando explícito.");
}

/// Catálogo, worker e trava, comuns às páginas que ainda são marcadores.
pub fn informacoes_da_sessao(ui: &mut egui::Ui, app: &AppDag) {
    egui::Grid::new(concat!("sessao-", file!()))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Catálogo");
            ui.monospace(app.caminho.display().to_string());
            ui.end_row();
            ui.label("Trava do escritor");
            ui.monospace(
                crate::app::caminho_da_trava(&app.caminho)
                    .display()
                    .to_string(),
            );
            ui.end_row();
            ui.label("Agente");
            ui.label(app.worker.modo().texto_status());
            ui.end_row();
            if let ModoWorker::SomenteLeitura { motivo } | ModoWorker::Falhou { motivo } =
                app.worker.modo()
            {
                ui.label("Motivo");
                ui.label(motivo);
                ui.end_row();
            }
            ui.label("Varredura");
            ui.label(if app.scan_ativo() {
                "em andamento"
            } else {
                "ociosa"
            });
            ui.end_row();
        });
}

/// Paralelismo do Hash Completo: threads por disco e discos em paralelo.
fn hash_em_paralelo(ui: &mut egui::Ui, app: &mut AppDag) {
    ui.heading("Hash Completo");
    estado_da_carga(ui, &app.configuracoes.gravado);
    let mut salvar = false;
    egui::Grid::new("config-hash")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Threads de hash por disco");
            ui.add(
                egui::DragValue::new(&mut app.configuracoes.threads_por_disco)
                    .range(1..=THREADS_POR_DISCO_MAX),
            );
            ui.end_row();
            ui.label("Discos em paralelo");
            ui.add(
                egui::DragValue::new(&mut app.configuracoes.max_discos).range(1..=MAX_DISCOS_MAX),
            );
            ui.end_row();
        });
    ui.weak(
        "HDD: 1 thread por disco é o melhor (mais leituras simultâneas só aumentam as buscas). \
         SSD/NVMe podem ganhar com 2 a 4. Padrão: 1 thread por disco e 16 discos em paralelo.",
    );
    ui.weak(
        "A mudança vale a partir da próxima varredura, verificação ou retomada; \
         o que já está rodando mantém o valor com que começou.",
    );
    let motivo = app.motivo_sem_escrita_config();
    ui.horizontal(|ui| {
        let botao = ui.add_enabled(motivo.is_none(), egui::Button::new("Salvar"));
        if botao.clicked() {
            salvar = true;
        }
        if ui.button("Restaurar padrão").clicked() {
            app.configuracoes.threads_por_disco = THREADS_POR_DISCO_PADRAO;
            app.configuracoes.max_discos = MAX_DISCOS_PADRAO;
        }
        if let Some(m) = motivo {
            ui.weak(m);
        }
    });
    if salvar {
        let (t, d) = (
            app.configuracoes.threads_por_disco,
            app.configuracoes.max_discos,
        );
        app.salvar_config_hash(t, d);
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use dag_platform_linux::servico::Atividade;

    #[test]
    fn resumo_do_servico() {
        let mut e = EstadoServico::nao_instalado("/u".into(), None);
        assert_eq!(resumo_do_estado(&e), "não instalado");
        e.instalado = true;
        e.atividade = Atividade::Ativo;
        e.autostart = true;
        e.linger = Some(true);
        e.main_pid = Some(42);
        let t = resumo_do_estado(&e);
        assert!(
            t.contains("ativo")
                && t.contains("login")
                && t.contains("pid 42")
                && t.contains("boot")
        );
    }
}
