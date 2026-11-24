//! Página Discos: volumes conhecidos, capacidade, uso, dispositivo físico e estado.

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, percentual};
use crate::leitor::{Carga, Leitor, erro};
use crate::selecao::{
    LinhaDisco, aviso_raiz_sistema, montar_linhas, raizes_dos_selecionados, resumo,
};
use bancada_casca::egui;
use dag_core::{ClasseVolume, ModoVarredura};
use dag_store::consulta_gui::{LinhaVolume, listar_volumes};
use std::collections::BTreeSet;

pub static PAGINA: DefPagina = DefPagina {
    id: "discos",
    rotulo: "Discos",
    keytip: "D",
    icone: "discos",
    grupos: &[
        DefGrupo {
            id: "discos.volumes",
            rotulo: "Volumes",
            acoes: &[
                DefAcao {
                    id: "discos.detectar",
                    rotulo: "Atualizar volumes",
                    dica: "Detecta volumes, partições e pontos de montagem e atualiza o Catálogo",
                },
                DefAcao {
                    id: "discos.propriedades",
                    rotulo: "Propriedades",
                    dica: "Mostra os detalhes do volume selecionado",
                },
            ],
        },
        DefGrupo {
            id: "discos.catalogo",
            rotulo: "Catálogo",
            acoes: &[
                DefAcao {
                    id: "discos.incluir",
                    rotulo: "Incluir no catálogo",
                    dica: "Inclui o volume selecionado nas raízes da próxima varredura",
                },
                DefAcao {
                    id: "discos.usar_na_varredura",
                    rotulo: "Usar na Varredura",
                    dica: "Adiciona os volumes marcados às raízes da Varredura e abre a página",
                },
            ],
        },
    ],
    resumo: "Volumes detectados, capacidade, espaço livre e pontos de montagem.",
    desenhar,
};

/// Estado da página.
#[derive(Debug)]
pub struct Estado {
    pub volumes: Carga<Vec<LinhaVolume>>,
    vista: u64,
    pub selecionado: Option<i64>,
    pub detalhes: bool,
    /// Volumes marcados (caixa de seleção) para varrer.
    pub marcados: BTreeSet<i64>,
    /// Mostrar também os volumes de imagem/virtuais.
    pub mostrar_todos: bool,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            volumes: Carga::default(),
            vista: 0,
            selecionado: None,
            detalhes: true,
            marcados: BTreeSet::new(),
            mostrar_todos: false,
        }
    }
}

impl Estado {
    /// Recolhe resultados e dispara a leitura quando o Catálogo mudou.
    pub fn atualizar(&mut self, leitor: &Leitor, geracao: u64) {
        if self.vista != geracao {
            self.vista = geracao;
            self.volumes
                .pedir(leitor.consultar(|c| listar_volumes(c).map_err(erro)));
        }
        self.volumes.atualizar();
    }

    /// Ponto de montagem do volume selecionado.
    #[must_use]
    pub fn montagem_selecionada(&self) -> Option<String> {
        let id = self.selecionado?;
        self.volumes
            .dado
            .as_ref()?
            .iter()
            .find(|v| v.id == id)?
            .montagem
            .clone()
    }
}

const COLUNAS: [Coluna; 11] = [
    Coluna::esq("Varrer", 55.0),
    Coluna::esq("Classe", 105.0),
    Coluna::esq("Montagem", 200.0),
    Coluna::esq("Rótulo", 120.0),
    Coluna::esq("FS", 70.0),
    Coluna::dir("Capacidade", 95.0),
    Coluna::dir("Livre", 95.0),
    Coluna::esq("Uso", 150.0),
    Coluna::esq("Dispositivo físico", 170.0),
    Coluna::esq("Estado", 110.0),
    Coluna::dir("Catalogado", 100.0),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.discos.atualizar(&app.leitor, app.geracao);
    app.garantir_classificacao();
    let motivo = app.motivo_sem_escrita();
    let mut atualizar = false;
    let mut incluir = false;
    let mut destino: Option<(Vec<String>, Option<ModoVarredura>)> = None;
    let (linhas, ocultos) = match app.discos.volumes.dado.as_deref() {
        Some(v) => montar_linhas(v, &app.classificacao, app.discos.mostrar_todos),
        None => (Vec::new(), 0),
    };
    let marcadas = raizes_dos_selecionados(&linhas, &app.discos.marcados);
    ui.horizontal_wrapped(|ui| {
        let b = ui.add_enabled(motivo.is_none(), egui::Button::new("Atualizar volumes"));
        let b = match motivo {
            Some(m) => b.on_disabled_hover_text(m),
            None => b.on_hover_text("Detecta os volumes agora (o worker grava no Catálogo)"),
        };
        atualizar = b.clicked();
        let tem_montagem = app.discos.montagem_selecionada().is_some();
        incluir = ui
            .add_enabled(tem_montagem, egui::Button::new("Incluir no catálogo"))
            .on_disabled_hover_text("Selecione um volume com ponto de montagem")
            .on_hover_text("Adiciona o ponto de montagem às raízes da próxima varredura")
            .clicked();
        ui.checkbox(&mut app.discos.detalhes, "Detalhes");
        ui.checkbox(
            &mut app.discos.mostrar_todos,
            format!("Mostrar todos ({ocultos} ocultos)"),
        )
        .on_hover_text("Inclui volumes de imagem (snap, loop) e virtuais (zram, overlay...)");
    });
    ui.horizontal_wrapped(|ui| {
        let sem_marca = marcadas.is_empty();
        let motivo_scan = motivo.or(sem_marca.then_some("Marque ao menos um volume"));
        for (rotulo, modo) in [
            ("Full Scan dos selecionados", Some(ModoVarredura::FullScan)),
            ("Fast Scan dos selecionados", Some(ModoVarredura::FastScan)),
        ] {
            let b = ui.add_enabled(motivo_scan.is_none(), egui::Button::new(rotulo));
            let b = match motivo_scan {
                Some(m) => b.on_disabled_hover_text(m),
                None => {
                    b.on_hover_text("Varre o ponto de montagem principal de cada volume marcado")
                }
            };
            if b.clicked() {
                destino = Some((marcadas.clone(), modo));
            }
        }
        if ui
            .add_enabled(!sem_marca, egui::Button::new("Usar na Varredura"))
            .on_disabled_hover_text("Marque ao menos um volume")
            .on_hover_text("Adiciona às raízes da página Varredura e abre a página")
            .clicked()
        {
            destino = Some((marcadas.clone(), None));
        }
        ui.weak(format!("{} marcado(s)", marcadas.len()));
    });
    if let Some(a) = aviso_raiz_sistema(&marcadas) {
        ui.colored_label(ui.visuals().warn_fg_color, a);
    }
    ui.separator();
    estado_da_carga(ui, &app.discos.volumes);
    if app.discos.volumes.dado.is_some() {
        ui.weak(resumo(&linhas, ocultos));
    }
    let altura = (super::altura_livre(ui) - 140.0).max(160.0);
    let est = &mut app.discos;
    match est.volumes.dado.as_deref() {
        Some([]) | None if est.volumes.erro.is_none() && !est.volumes.carregando() => {
            estado_vazio(
                ui,
                "Nenhum volume no Catálogo",
                PAGINA.resumo,
                "Clique em \"Atualizar volumes\" para detectar os volumes deste computador.",
            );
        }
        Some(_) if !linhas.is_empty() => {
            let mut sel = est.selecionado;
            let mut marcados = est.marcados.clone();
            tabela(ui, "discos", &COLUNAS, linhas.len(), altura, |l, i| {
                let d: &LinhaDisco = &linhas[i];
                let v = &d.volume;
                l.ui(|ui| {
                    let mut m = marcados.contains(&v.id);
                    if ui.checkbox(&mut m, "").changed() {
                        if m {
                            marcados.insert(v.id);
                        } else {
                            marcados.remove(&v.id);
                        }
                    }
                });
                l.ui(|ui| {
                    ui.label(d.classe.rotulo());
                    if d.sistema {
                        ui.small("sistema")
                            .on_hover_text("Partição de sistema (inicialização) de um disco real");
                    }
                });
                let nome = v
                    .montagem
                    .clone()
                    .unwrap_or_else(|| "(sem montagem)".into());
                let extra = if v.montagens > 1 {
                    format!("{nome}  (+{})", v.montagens - 1)
                } else {
                    nome
                };
                if l.selecionavel(sel == Some(v.id), extra).clicked() {
                    sel = Some(v.id);
                }
                l.texto(v.rotulo.clone().unwrap_or_default());
                l.texto(v.fs_type.clone().unwrap_or_default());
                l.texto(v.capacidade.map_or_else(|| "—".into(), bytes_humanos));
                l.texto(v.livre.map_or_else(|| "—".into(), bytes_humanos));
                l.ui(|ui| match v.uso_percentual() {
                    Some(p) => {
                        ui.add(
                            egui::ProgressBar::new((p / 100.0) as f32)
                                .desired_width(130.0)
                                .text(percentual(Some(p))),
                        );
                    }
                    None => {
                        ui.weak("—");
                    }
                });
                l.texto(match (&v.dispositivo, &v.tipo_dispositivo) {
                    (Some(d), Some(t)) => format!("{d} ({t})"),
                    (Some(d), None) => d.clone(),
                    (None, Some(t)) => t.clone(),
                    (None, None) => "não informado".into(),
                });
                l.texto(if v.conectado {
                    "Conectado".to_string()
                } else {
                    format!("Offline (visto {})", data_s(v.visto_em))
                });
                l.texto(
                    v.bytes_catalogados
                        .map_or_else(|| "—".into(), bytes_humanos),
                );
            });
            est.selecionado = sel;
            est.marcados = marcados;
            if est.detalhes {
                ui.separator();
                if let Some(d) = sel.and_then(|id| linhas.iter().find(|d| d.volume.id == id)) {
                    let v = &d.volume;
                    ui.strong(format!(
                        "Volume {} — {}",
                        v.id,
                        v.rotulo.as_deref().unwrap_or("sem rótulo")
                    ));
                    ui.label(format!(
                        "Classe: {}. {} ponto(s) de montagem (bind mounts contam como o mesmo \
                         volume); {}.",
                        d.classe.rotulo(),
                        v.montagens,
                        if v.conectado {
                            "conectado"
                        } else {
                            "desconectado"
                        }
                    ));
                    if matches!(d.classe, ClasseVolume::Imagem | ClasseVolume::Virtual) {
                        ui.weak("Volume de imagem/virtual: não é um disco real.");
                    }
                    ui.weak(
                        "Dois volumes no mesmo Dispositivo Físico falham juntos; a Cobertura só conta \
                         cópias em dispositivos diferentes.",
                    );
                } else {
                    ui.weak("Selecione um volume para ver os detalhes.");
                }
            }
        }
        _ => {}
    }
    if atualizar {
        app.atualizar_volumes();
    }
    if incluir && let Some(m) = app.discos.montagem_selecionada() {
        app.varredura.adicionar_raiz(m);
        app.aviso(
            bancada_casca::Tom::Info,
            "Ponto de montagem adicionado às raízes; abra a página Varredura para iniciar.",
        );
    }
    match destino {
        Some((raizes, Some(modo))) => app.varrer_raizes(raizes, modo),
        Some((raizes, None)) => {
            for r in raizes {
                app.varredura.adicionar_raiz(r);
            }
            bancada_casca::abrir(ui.ctx(), "varredura");
        }
        None => {}
    }
}
