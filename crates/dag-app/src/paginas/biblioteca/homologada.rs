//! Biblioteca > Homologada (somente leitura): o perfil da biblioteca Calibre que você já usa:
//! colunas, estantes candidatas (inclusive as `#pasta`), vocabulário de tags com contagens,
//! tags de controle (`!Catalogar`) e a compatibilidade do que o DAG clona. Nada aqui escreve.

use bancada_casca::egui;
use dag_core::CampoInfo;

use super::texto::texto_puro;
use super::{Consulta, acao, estado_da_consulta, quadro};
use crate::app::AppDag;
use crate::formato::{data_s, inteiro_pt_br};
use crate::paginas::tabela::{Chave, Coluna, tabela};
use crate::paginas::{DefGrupo, DefPagina, cabecalho, estado_vazio};

pub static PAGINA: DefPagina = DefPagina {
    id: "biblioteca-homologada",
    rotulo: "Homologada",
    keytip: "H",
    icone: "homologada",
    grupos: &[DefGrupo {
        id: "biblioteca.homologada",
        rotulo: "Homologada",
        acoes: &[acao(
            "biblioteca.perfil_atualizar",
            "Atualizar perfil",
            "Relê o perfil da Biblioteca Homologada (não escreve nada nela)",
        )],
    }],
    resumo: "O perfil da sua biblioteca Calibre, só leitura: colunas, estantes, tags e o que o DAG consegue clonar.",
    desenhar,
};

/// Tags de controle: as que começam com `!` (por exemplo `!Catalogar`) dizem ao Calibre ou a
/// você o que fazer com o livro; o DAG as mostra à parte.
#[must_use]
pub fn tags_de_controle(tags: &[(String, u64)]) -> Vec<&(String, u64)> {
    tags.iter().filter(|(t, _)| t.starts_with('!')).collect()
}

/// Estante candidata que vem de uma coluna personalizada `#pasta`.
#[must_use]
pub fn e_estante_de_pasta(nome: &str) -> bool {
    nome.starts_with('#')
}

/// O campo é compatível (o DAG o clona) ou fica de fora, com o motivo.
#[must_use]
pub fn compatibilidade(c: &CampoInfo) -> String {
    if c.clonavel {
        "clonado".into()
    } else {
        format!(
            "não clonado: {}",
            texto_puro(c.motivo.as_deref().unwrap_or("sem motivo informado"), 120)
        )
    }
}

const COLUNAS_FORMATOS: [Coluna; 2] =
    [Coluna::esq("Formato", 120.0), Coluna::dir("Arquivos", 90.0)];
const COLUNAS_TAGS: [Coluna; 2] = [Coluna::esq("Tag", 260.0), Coluna::dir("Livros", 80.0)];
const COLUNAS_ESTANTES: [Coluna; 3] = [
    Coluna::esq("Estante candidata", 240.0),
    Coluna::dir("Pontuação", 80.0),
    Coluna::esq("Razões", 380.0),
];
const COLUNAS_CAMPOS: [Coluna; 6] = [
    Coluna::esq("Rótulo", 150.0),
    Coluna::esq("Nome", 180.0),
    Coluna::esq("Tipo", 90.0),
    Coluna::esq("Múltiplo", 70.0),
    Coluna::esq("Compatibilidade", 380.0),
    Coluna::esq("", 10.0).sem_ordem(),
];

fn desenhar(ui: &mut egui::Ui, app: &mut AppDag) {
    cabecalho(ui, &PAGINA);
    app.bib_quadro(ui.ctx());
    app.bib_carregar(&[Consulta::Bibliotecas, Consulta::Perfil]);
    estado_da_consulta(ui, &app.biblioteca, Consulta::Perfil);
    if app.biblioteca.bibliotecas.is_some() && app.biblioteca.homologada().is_none() {
        estado_vazio(
            ui,
            "Nenhuma Biblioteca Homologada registrada",
            PAGINA.resumo,
            "Registre-a na Visão Geral. O DAG a lê em modo somente leitura e nunca a altera.",
        );
        return;
    }
    let Some(d) = app.biblioteca.perfil.clone() else {
        return;
    };
    quadro(ui, "Biblioteca Homologada (somente leitura)", |ui| {
        ui.label(format!(
            "{} — {}",
            texto_puro(&d.biblioteca.nome, 80),
            texto_puro(&d.biblioteca.caminho, 200)
        ));
        ui.weak(format!(
            "Calibre {} · esquema {} · última sincronização {}",
            d.biblioteca.versao_calibre.as_deref().unwrap_or("—"),
            d.perfil
                .versao_esquema
                .map_or_else(|| "—".into(), |v| v.to_string()),
            data_s(d.biblioteca.ultima_sincronizacao)
        ));
        egui::Grid::new("bib-perfil-totais")
            .num_columns(4)
            .spacing([24.0, 4.0])
            .show(ui, |ui| {
                let par = |ui: &mut egui::Ui, r: &str, n: u64| {
                    ui.label(r);
                    ui.strong(inteiro_pt_br(n));
                };
                par(ui, "Livros", d.perfil.total_livros);
                par(ui, "Autores", d.perfil.total_autores);
                ui.end_row();
                par(ui, "Tags", d.perfil.total_tags);
                par(ui, "Séries", d.perfil.total_series);
                ui.end_row();
                par(ui, "Editoras", d.perfil.total_editoras);
                par(ui, "Arquivos de formato", d.perfil.total_arquivos_formato);
                ui.end_row();
            });
    });
    ui.add_space(6.0);
    ui.columns(2, |cols| {
        quadro(&mut cols[0], "Formatos", |ui| {
            let f = &d.perfil.formatos;
            tabela(
                ui,
                "bib-perfil-formatos",
                &COLUNAS_FORMATOS,
                f.len(),
                200.0,
                |c, i| match c {
                    0 => Chave::texto(&f[i].0),
                    _ => Chave::num(f[i].1),
                },
                |l, i| {
                    l.texto(texto_puro(&f[i].0.to_uppercase(), 30));
                    l.texto(inteiro_pt_br(f[i].1));
                },
            );
        });
        quadro(&mut cols[1], "Vocabulário de tags", |ui| {
            let t = &d.perfil.tags_principais;
            tabela(
                ui,
                "bib-perfil-tags",
                &COLUNAS_TAGS,
                t.len(),
                200.0,
                |c, i| match c {
                    0 => Chave::texto(&t[i].0),
                    _ => Chave::num(t[i].1),
                },
                |l, i| {
                    l.texto(texto_puro(&t[i].0, 60));
                    l.texto(inteiro_pt_br(t[i].1));
                },
            );
            let controle = tags_de_controle(t);
            if !controle.is_empty() {
                ui.separator();
                ui.strong("Tags de controle");
                for (tag, n) in controle {
                    ui.label(format!(
                        "{} — {} livro(s)",
                        texto_puro(tag, 60),
                        inteiro_pt_br(*n)
                    ));
                }
            }
        });
    });
    ui.add_space(6.0);
    quadro(ui, "Estantes candidatas", |ui| {
        let e = &d.perfil.candidatos_estante;
        if e.is_empty() {
            ui.weak("Nenhuma estante candidata encontrada.");
            return;
        }
        tabela(
            ui,
            "bib-perfil-estantes",
            &COLUNAS_ESTANTES,
            e.len(),
            220.0,
            |c, i| match c {
                0 => Chave::texto(&e[i].0),
                1 => Chave::num(e[i].1),
                _ => Chave::texto(&e[i].2.join("; ")),
            },
            |l, i| {
                let (nome, pontos, razoes) = &e[i];
                l.texto(if e_estante_de_pasta(nome) {
                    format!("{} (coluna #pasta)", texto_puro(nome, 60))
                } else {
                    texto_puro(nome, 60)
                });
                l.texto(pontos.to_string());
                l.texto(texto_puro(&razoes.join("; "), 200));
            },
        );
        ui.weak(
            "O DAG só sugere estantes que já existem; nunca cria uma estante nova em silêncio.",
        );
    });
    ui.add_space(6.0);
    quadro(ui, "Compatibilidade das colunas", |ui| {
        let c = &d.campos;
        tabela(
            ui,
            "bib-perfil-campos",
            &COLUNAS_CAMPOS,
            c.len(),
            260.0,
            |col, i| match col {
                0 => Chave::texto(&c[i].rotulo),
                1 => Chave::texto(&c[i].nome),
                2 => Chave::texto(&c[i].datatype),
                3 => Chave::num(i128::from(c[i].multiplo)),
                4 => Chave::texto(&compatibilidade(&c[i])),
                _ => Chave::Nulo,
            },
            |l, i| {
                l.texto(texto_puro(&c[i].rotulo, 40));
                l.texto(texto_puro(&c[i].nome, 60));
                l.texto(texto_puro(&c[i].datatype, 30));
                l.texto(if c[i].multiplo { "sim" } else { "não" });
                l.texto(compatibilidade(&c[i]));
                l.texto("");
            },
        );
        if !d.perfil.nao_clonaveis.is_empty() {
            ui.separator();
            ui.weak("O DAG não clona (nada se perde em silêncio):");
            for n in &d.perfil.nao_clonaveis {
                ui.label(format!("• {}", texto_puro(n, 200)));
            }
        }
    });
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn tags_de_controle_sao_as_que_comecam_com_exclamacao() {
        let tags = vec![
            ("Matemática".to_string(), 10),
            ("!Catalogar".to_string(), 3),
            ("xadrez".to_string(), 2),
            ("!Revisar".to_string(), 1),
        ];
        let c = tags_de_controle(&tags);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].0, "!Catalogar");
    }

    #[test]
    fn estante_de_pasta_e_compatibilidade() {
        assert!(e_estante_de_pasta("#pasta"));
        assert!(!e_estante_de_pasta("Matemática"));
        let mut c = CampoInfo {
            rotulo: "pasta".into(),
            nome: "#pasta".into(),
            datatype: "text".into(),
            multiplo: false,
            clonavel: true,
            motivo: None,
        };
        assert_eq!(compatibilidade(&c), "clonado");
        c.clonavel = false;
        c.motivo = Some("composta".into());
        assert_eq!(compatibilidade(&c), "não clonado: composta");
        c.motivo = None;
        assert!(compatibilidade(&c).contains("sem motivo"));
    }
}
