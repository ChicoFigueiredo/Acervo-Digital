//! Contêineres no Explorador (ADR 0006): cada compactado da pasta aberta se expande como pasta
//! virtual, lida da tabela de Membros do Catálogo. Nada é aberto no disco.

use std::collections::HashMap;

use crate::formato::bytes_humanos;
use crate::leitor::{Carga, Leitor, erro};
use bancada_casca::egui;
use dag_store::conteineres::{self, EntradaMembro};

/// Máximo de entradas por pasta virtual carregada de uma vez.
const POR_PAGINA: u32 = 500;

/// Estado de um contêiner: situação, formato e número de Membros.
type Selo = Option<(String, Option<String>, i64)>;

/// Uma pasta virtual: contêiner (`file_id`), Membro aninhado pai e prefixo dentro dele.
type Chave = (i64, Option<i64>, Vec<u8>);

/// Pastas virtuais já abertas e suas listas.
#[derive(Debug, Default)]
pub struct Estado {
    pastas: HashMap<Chave, Carga<Vec<EntradaMembro>>>,
    /// Estado do contêiner (`lido`, `aguardando_senha`...), por arquivo.
    selos: HashMap<i64, Carga<Selo>>,
}

impl Estado {
    /// Descarta o que foi lido quando o Catálogo muda.
    pub fn limpar(&mut self) {
        self.pastas.clear();
        self.selos.clear();
    }

    fn pasta(&mut self, leitor: &Leitor, chave: &Chave) -> &Carga<Vec<EntradaMembro>> {
        self.pastas.entry(chave.clone()).or_insert_with(|| {
            let (id, pai, prefixo) = chave.clone();
            let mut c = Carga::default();
            c.pedir(leitor.consultar(move |con| {
                conteineres::listar_filhos(con, id, pai, &prefixo, None, POR_PAGINA).map_err(erro)
            }));
            c
        })
    }

    fn selo(&mut self, leitor: &Leitor, id: i64) -> &Carga<Selo> {
        self.selos.entry(id).or_insert_with(|| {
            let mut c = Carga::default();
            c.pedir(
                leitor
                    .consultar(move |con| conteineres::estado_do_conteiner(con, id).map_err(erro)),
            );
            c
        })
    }

    fn atualizar(&mut self) {
        self.pastas.values_mut().for_each(|c| {
            c.atualizar();
        });
        self.selos.values_mut().for_each(|c| {
            c.atualizar();
        });
    }
}

/// Lista os compactados `(id, nome)` entre os arquivos da pasta aberta como pastas virtuais.
pub fn desenhar(
    ui: &mut egui::Ui,
    est: &mut Estado,
    leitor: &Leitor,
    arquivos: impl Iterator<Item = (i64, String)>,
) {
    est.atualizar();
    let conteineres: Vec<(i64, String)> = arquivos
        .filter(|(_, n)| conteineres::nome_de_conteiner(n))
        .collect();
    if conteineres.is_empty() {
        return;
    }
    ui.separator();
    ui.strong("Compactados nesta pasta");
    for (id, nome) in conteineres {
        let selo = est.selo(leitor, id).dado.clone().flatten();
        let (titulo, aberto) = match &selo {
            Some((estado, formato, membros)) if estado == "lido" => (
                format!(
                    "{nome} — {} ({membros} item(ns))",
                    formato.as_deref().unwrap_or("contêiner")
                ),
                true,
            ),
            Some((estado, ..)) if estado == "aguardando_senha" => {
                (format!("{nome} — cifrado: veja a Fila de Serviço"), false)
            }
            Some((estado, ..)) if estado == "falha" => (
                format!("{nome} — falha de leitura (Fila de Serviço)"),
                false,
            ),
            Some(_) => (format!("{nome} — conteúdo ainda não lido"), false),
            None => (nome.clone(), false),
        };
        if aberto {
            egui::CollapsingHeader::new(titulo)
                .id_salt(("conteiner", id))
                .show(ui, |ui| {
                    pasta_virtual(ui, est, leitor, (id, None, Vec::new()), 0)
                });
        } else {
            ui.label(titulo);
        }
    }
}

fn pasta_virtual(ui: &mut egui::Ui, est: &mut Estado, leitor: &Leitor, chave: Chave, nivel: usize) {
    if nivel > 12 {
        ui.weak("…");
        return;
    }
    let carga = est.pasta(leitor, &chave);
    if let Some(e) = &carga.erro {
        ui.colored_label(ui.visuals().warn_fg_color, e);
        return;
    }
    let Some(entradas) = carga.dado.clone() else {
        ui.spinner();
        return;
    };
    for e in entradas {
        let nome = String::from_utf8_lossy(&e.nome).into_owned();
        if e.navegavel {
            let (id, pai, prefixo) = &chave;
            let filha: Chave = match e.aninhado_ordem {
                // Contêiner aninhado: entra no espaço de nomes do Membro.
                Some(ordem) => (*id, Some(ordem), Vec::new()),
                None => {
                    let mut p = prefixo.clone();
                    p.extend_from_slice(&e.nome);
                    p.push(b'/');
                    (*id, *pai, p)
                }
            };
            egui::CollapsingHeader::new(format!("{nome}/  ({})", bytes_humanos(e.tamanho)))
                .id_salt(("virtual", &chave, &e.nome))
                .show(ui, |ui| pasta_virtual(ui, est, leitor, filha, nivel + 1));
        } else {
            ui.label(format!("{nome}  ({})", bytes_humanos(e.tamanho)))
                .on_hover_text(match &e.estado {
                    Some(s) if s != "lido" => format!("estado: {s}"),
                    _ => "BLAKE3 calculado em memória".to_string(),
                });
        }
    }
}
