//! Página Discos: volumes conhecidos, capacidade, uso, dispositivo físico e estado.

use super::tabela::{Coluna, tabela};
use super::{DefAcao, DefGrupo, DefPagina, cabecalho, estado_da_carga, estado_vazio};
use crate::app::AppDag;
use crate::formato::{bytes_humanos, data_s, percentual};
use crate::leitor::{Carga, Leitor, erro};
use bancada_casca::egui;
use dag_store::consulta_gui::{LinhaVolume, listar_volumes};

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
            acoes: &[DefAcao {
                id: "discos.incluir",
                rotulo: "Incluir no catálogo",
                dica: "Inclui o volume selecionado nas raízes da próxima varredura",
            }],
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
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            volumes: Carga::default(),
            vista: 0,
            selecionado: None,
            detalhes: true,
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

const COLUNAS: [Coluna; 9] = [
    Coluna::esq("Montagem", 200.0),
    Coluna::esq("Rótulo", 120.0),
    Coluna::esq("Sistema", 70.0),
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
    let motivo = app.motivo_sem_escrita();
    let mut atualizar = false;
    let mut incluir = false;
    ui.horizontal(|ui| {
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
    });
    ui.separator();
    estado_da_carga(ui, &app.discos.volumes);
    let altura = (ui.available_height() - 140.0).max(160.0);
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
        Some(volumes) if !volumes.is_empty() => {
            let mut sel = est.selecionado;
            tabela(ui, "discos", &COLUNAS, volumes.len(), altura, |l, i| {
                let v = &volumes[i];
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
            if est.detalhes {
                ui.separator();
                if let Some(v) = sel.and_then(|id| volumes.iter().find(|v| v.id == id)) {
                    ui.strong(format!(
                        "Volume {} — {}",
                        v.id,
                        v.rotulo.as_deref().unwrap_or("sem rótulo")
                    ));
                    ui.label(format!(
                        "{} ponto(s) de montagem (bind mounts contam como o mesmo volume); {}.",
                        v.montagens,
                        if v.conectado {
                            "conectado"
                        } else {
                            "desconectado"
                        }
                    ));
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
}
