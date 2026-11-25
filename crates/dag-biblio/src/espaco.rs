//! Relatório de espaço por Volume (B-15, CA-B19, WOW-02).
//!
//! Distingue bytes lógicos, alocados (blocos), estimativa recuperável, economia incerta e o
//! que já foi efetivamente recuperado (Descartado). Sem dupla contagem: caminhos do Mesmo
//! Arquivo Físico (hard links) contam uma vez; cópias que compartilham blocos (reflink) vão
//! para "economia incerta"; Membros de Contêiner nunca somam (`CONTEXT.md`).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Uma entrada do Catálogo, preenchida pelo chamador.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArquivoEspaco {
    pub volume_id: i64,
    pub arquivo_id: i64,
    /// (dispositivo, inode): igual = Mesmo Arquivo Físico.
    #[serde(default)]
    pub id_fisico: Option<(u64, u64)>,
    /// `nlink` informado pelo sistema de arquivos.
    #[serde(default = "um")]
    pub links: u32,
    pub tamanho_logico: u64,
    /// Bytes alocados (blocos); `None` = arredondar o lógico ao bloco.
    #[serde(default)]
    pub tamanho_alocado: Option<u64>,
    #[serde(default)]
    pub hash: Option<String>,
    /// Membro de Contêiner: nunca soma.
    #[serde(default)]
    pub membro_de_conteiner: bool,
    /// O arquivo compartilha blocos com outro (reflink/clone).
    #[serde(default)]
    pub reflink: bool,
    /// Já Descartado (estava na Lixeira e sumiu).
    #[serde(default)]
    pub descartado: bool,
    /// Preferência para ser a cópia mantida (Formato homologado).
    #[serde(default)]
    pub homologado: bool,
}

fn um() -> u32 {
    1
}

/// Totais de um Volume.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatorioVolume {
    pub volume_id: i64,
    /// Entradas ativas informadas (caminhos), exceto membros e descartados.
    pub caminhos: u64,
    /// Arquivos físicos distintos.
    pub arquivos_fisicos: u64,
    pub bytes_logicos: u64,
    pub bytes_alocados: u64,
    /// Redundância exata com recuperação provável (cópia inteira, sem reflink/hard link externo).
    pub estimativa_recuperavel: u64,
    /// Redundância exata cuja economia é duvidosa (reflink compartilha blocos; hard link com
    /// outros caminhos fora do Catálogo).
    pub economia_incerta: u64,
    /// Já recuperado (Descartado).
    pub efetivamente_recuperado: u64,
    pub caminhos_hardlink_ignorados: u64,
    pub membros_ignorados: u64,
}

/// Uma cópia redundante de um grupo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiaRedundante {
    pub volume_id: i64,
    pub arquivo_id: i64,
    pub bytes_alocados: u64,
    /// `true` = economia incerta (reflink ou hard link externo).
    pub incerta: bool,
    pub motivo_incerteza: Option<String>,
}

/// Grupo de conteúdo idêntico com uma cópia a manter (WOW-02: por que cada item é redundante).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoRecuperavel {
    pub hash: String,
    pub tamanho_logico: u64,
    pub mantida: (i64, i64),
    pub redundantes: Vec<CopiaRedundante>,
    pub bytes_estimados: u64,
    pub bytes_incertos: u64,
}

/// Relatório completo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatorioEspaco {
    pub volumes: Vec<RelatorioVolume>,
    /// Ordenado por bytes estimados decrescentes.
    pub grupos: Vec<GrupoRecuperavel>,
}

fn alocado(a: &ArquivoEspaco, bloco: u64) -> u64 {
    a.tamanho_alocado.unwrap_or_else(|| {
        if bloco == 0 {
            a.tamanho_logico
        } else {
            a.tamanho_logico.div_ceil(bloco) * bloco
        }
    })
}

fn vol(m: &mut BTreeMap<i64, RelatorioVolume>, id: i64) -> &mut RelatorioVolume {
    m.entry(id).or_insert_with(|| RelatorioVolume {
        volume_id: id,
        ..Default::default()
    })
}

struct Fisico<'a> {
    rep: &'a ArquivoEspaco,
    caminhos: u32,
}

/// Gera o relatório. `bloco` é o tamanho de bloco para estimar o alocado quando ausente.
#[must_use]
pub fn relatorio(arquivos: &[ArquivoEspaco], bloco: u64) -> RelatorioEspaco {
    let mut vols: BTreeMap<i64, RelatorioVolume> = BTreeMap::new();

    // Descartados: recuperado de verdade (uma vez por arquivo físico).
    let mut descartados_vistos: HashSet<(i64, u64, u64)> = HashSet::new();
    let mut ordenados: Vec<&ArquivoEspaco> = arquivos.iter().collect();
    ordenados.sort_by_key(|a| (a.volume_id, a.arquivo_id));
    for a in ordenados
        .iter()
        .filter(|a| a.descartado && !a.membro_de_conteiner)
    {
        let novo = a
            .id_fisico
            .is_none_or(|(d, i)| descartados_vistos.insert((a.volume_id, d, i)));
        let v = vol(&mut vols, a.volume_id);
        if novo {
            v.efetivamente_recuperado += alocado(a, bloco);
        }
    }

    // Vivos: físicos distintos, sem membros.
    let mut fisicos: Vec<Fisico> = Vec::new();
    let mut por_fisico: HashMap<(i64, u64, u64), usize> = HashMap::new();
    for a in ordenados.iter().filter(|a| !a.descartado) {
        let v = vol(&mut vols, a.volume_id);
        if a.membro_de_conteiner {
            v.membros_ignorados += 1;
            continue;
        }
        v.caminhos += 1;
        match a.id_fisico {
            Some((d, i)) => match por_fisico.get(&(a.volume_id, d, i)) {
                Some(idx) => {
                    fisicos[*idx].caminhos += 1;
                    v.caminhos_hardlink_ignorados += 1;
                }
                None => {
                    por_fisico.insert((a.volume_id, d, i), fisicos.len());
                    fisicos.push(Fisico {
                        rep: a,
                        caminhos: 1,
                    });
                }
            },
            None => fisicos.push(Fisico {
                rep: a,
                caminhos: 1,
            }),
        }
    }
    for f in &fisicos {
        let v = vol(&mut vols, f.rep.volume_id);
        v.arquivos_fisicos += 1;
        v.bytes_logicos += f.rep.tamanho_logico;
        v.bytes_alocados += alocado(f.rep, bloco);
    }

    // Grupos de conteúdo idêntico (hash + tamanho).
    let mut grupos_idx: BTreeMap<(String, u64), Vec<usize>> = BTreeMap::new();
    for (i, f) in fisicos.iter().enumerate() {
        if let Some(h) = &f.rep.hash {
            grupos_idx
                .entry((h.clone(), f.rep.tamanho_logico))
                .or_default()
                .push(i);
        }
    }
    let mut grupos = Vec::new();
    for ((hash, tamanho), mut idx) in grupos_idx {
        if idx.len() < 2 {
            continue;
        }
        // Mantida: homologada primeiro, depois a de menor (volume, arquivo).
        idx.sort_by_key(|i| {
            let r = fisicos[*i].rep;
            (!r.homologado, r.volume_id, r.arquivo_id)
        });
        let mantida = fisicos[idx[0]].rep;
        let mut g = GrupoRecuperavel {
            hash,
            tamanho_logico: tamanho,
            mantida: (mantida.volume_id, mantida.arquivo_id),
            redundantes: Vec::new(),
            bytes_estimados: 0,
            bytes_incertos: 0,
        };
        for i in &idx[1..] {
            let f = &fisicos[*i];
            let r = f.rep;
            if r.homologado {
                // Outro Formato da própria Homologada com o mesmo conteúdo: é assunto de revisão
                // dentro da biblioteca, nunca espaço "recuperável" (o DAG não mexe nela).
                continue;
            }
            let bytes = alocado(r, bloco);
            let mut motivo = None;
            if r.reflink || (mantida.reflink && mantida.volume_id == r.volume_id) {
                motivo = Some(
                    "reflink: as cópias compartilham blocos, a economia real é incerta".to_owned(),
                );
            } else if r.links > f.caminhos {
                motivo = Some("hard link com outros caminhos fora do Catálogo: apagar este caminho não libera blocos".to_owned());
            }
            let incerta = motivo.is_some();
            let v = vol(&mut vols, r.volume_id);
            if incerta {
                v.economia_incerta += bytes;
                g.bytes_incertos += bytes;
            } else {
                v.estimativa_recuperavel += bytes;
                g.bytes_estimados += bytes;
            }
            g.redundantes.push(CopiaRedundante {
                volume_id: r.volume_id,
                arquivo_id: r.arquivo_id,
                bytes_alocados: bytes,
                incerta,
                motivo_incerteza: motivo,
            });
        }
        grupos.push(g);
    }
    grupos.sort_by(|a, b| {
        b.bytes_estimados
            .cmp(&a.bytes_estimados)
            .then(b.bytes_incertos.cmp(&a.bytes_incertos))
            .then(a.hash.cmp(&b.hash))
    });
    RelatorioEspaco {
        volumes: vols.into_values().collect(),
        grupos,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn segundo_formato_homologado_nao_conta_como_recuperavel() {
        let mut a = arq(1, 1, 10_000, Some("h"));
        a.homologado = true;
        let mut b = arq(1, 2, 10_000, Some("h"));
        b.homologado = true;
        let c = arq(1, 3, 10_000, Some("h"));
        let r = relatorio(&[a, b, c], 4096);
        assert_eq!(r.grupos.len(), 1);
        let g = &r.grupos[0];
        assert_eq!(g.mantida, (1, 1));
        assert_eq!(g.redundantes.len(), 1, "só a cópia de fora");
        assert_eq!(g.redundantes[0].arquivo_id, 3);
    }

    fn arq(vol: i64, id: i64, logico: u64, hash: Option<&str>) -> ArquivoEspaco {
        ArquivoEspaco {
            volume_id: vol,
            arquivo_id: id,
            id_fisico: Some((1, id as u64)),
            links: 1,
            tamanho_logico: logico,
            tamanho_alocado: None,
            hash: hash.map(Into::into),
            membro_de_conteiner: false,
            reflink: false,
            descartado: false,
            homologado: false,
        }
    }

    fn vol_de(r: &RelatorioEspaco, id: i64) -> &RelatorioVolume {
        r.volumes.iter().find(|v| v.volume_id == id).unwrap()
    }

    #[test]
    fn logico_alocado_e_estimativa_sao_distintos() {
        let r = relatorio(
            &[arq(1, 1, 5000, Some("h")), arq(1, 2, 5000, Some("h"))],
            4096,
        );
        let v = vol_de(&r, 1);
        assert_eq!(v.bytes_logicos, 10_000);
        assert_eq!(v.bytes_alocados, 16_384); // 2 x 2 blocos de 4096
        assert_eq!(v.estimativa_recuperavel, 8192);
        assert_eq!(v.efetivamente_recuperado, 0);
        assert_eq!(r.grupos.len(), 1);
    }

    #[test]
    fn hard_links_nao_contam_em_dobro() {
        // Dois caminhos, mesmo inode: um arquivo físico, nenhuma economia.
        let mut a = arq(1, 1, 5000, Some("h"));
        let mut b = arq(1, 2, 5000, Some("h"));
        a.id_fisico = Some((1, 7));
        b.id_fisico = Some((1, 7));
        a.links = 2;
        b.links = 2;
        let r = relatorio(&[a, b], 4096);
        let v = vol_de(&r, 1);
        assert_eq!(v.arquivos_fisicos, 1);
        assert_eq!(v.caminhos, 2);
        assert_eq!(v.caminhos_hardlink_ignorados, 1);
        assert_eq!(v.bytes_logicos, 5000);
        assert_eq!(v.estimativa_recuperavel, 0);
        assert!(r.grupos.is_empty());
    }

    #[test]
    fn hard_link_com_caminho_fora_do_catalogo_e_incerto() {
        let a = arq(1, 1, 4096, Some("h"));
        let mut b = arq(1, 2, 4096, Some("h"));
        b.links = 2; // outro caminho fora do catálogo
        let r = relatorio(&[a, b], 4096);
        let v = vol_de(&r, 1);
        assert_eq!(v.estimativa_recuperavel, 0);
        assert_eq!(v.economia_incerta, 4096);
        assert!(r.grupos[0].redundantes[0].incerta);
    }

    #[test]
    fn reflink_e_economia_incerta() {
        let a = arq(1, 1, 4096, Some("h"));
        let mut b = arq(1, 2, 4096, Some("h"));
        b.reflink = true;
        let r = relatorio(&[a, b], 4096);
        let v = vol_de(&r, 1);
        assert_eq!(v.estimativa_recuperavel, 0);
        assert_eq!(v.economia_incerta, 4096);
        assert!(
            r.grupos[0].redundantes[0]
                .motivo_incerteza
                .as_ref()
                .unwrap()
                .contains("reflink")
        );
    }

    #[test]
    fn membros_de_conteiner_nunca_somam() {
        let mut m = arq(1, 3, 1_000_000, Some("h"));
        m.membro_de_conteiner = true;
        let r = relatorio(&[arq(1, 1, 1_000_000, Some("h")), m], 4096);
        let v = vol_de(&r, 1);
        assert_eq!(v.membros_ignorados, 1);
        assert_eq!(v.bytes_logicos, 1_000_000);
        assert_eq!(v.estimativa_recuperavel, 0);
        assert!(r.grupos.is_empty());
    }

    #[test]
    fn descartado_vira_efetivamente_recuperado_sem_dupla_contagem() {
        let mut d1 = arq(1, 9, 10_000, Some("h"));
        d1.descartado = true;
        d1.id_fisico = Some((1, 99));
        let mut d2 = d1.clone();
        d2.arquivo_id = 10; // outro caminho do mesmo inode já descartado
        let r = relatorio(&[arq(1, 1, 10_000, Some("h")), d1, d2], 4096);
        let v = vol_de(&r, 1);
        assert_eq!(v.efetivamente_recuperado, 12_288);
        assert_eq!(
            v.bytes_logicos, 10_000,
            "descartado não está mais no volume"
        );
        assert_eq!(v.estimativa_recuperavel, 0);
    }

    #[test]
    fn homologado_e_a_mantida_e_grupos_ordenados_por_bytes() {
        let mut h = arq(2, 50, 1000, Some("a"));
        h.homologado = true;
        let r = relatorio(
            &[
                arq(1, 1, 1000, Some("a")),
                h,
                arq(1, 2, 9000, Some("b")),
                arq(1, 3, 9000, Some("b")),
                arq(1, 4, 9000, None),
            ],
            0,
        );
        assert_eq!(r.grupos[0].hash, "b");
        assert_eq!(r.grupos[0].bytes_estimados, 9000);
        let ga = r.grupos.iter().find(|g| g.hash == "a").unwrap();
        assert_eq!(ga.mantida, (2, 50));
        assert_eq!(ga.redundantes[0].volume_id, 1);
        // Recuperação é atribuída ao volume da cópia redundante.
        assert_eq!(vol_de(&r, 1).estimativa_recuperavel, 10_000);
        assert_eq!(vol_de(&r, 2).estimativa_recuperavel, 0);
    }

    #[test]
    fn tamanhos_diferentes_com_mesmo_hash_nao_formam_grupo() {
        let r = relatorio(&[arq(1, 1, 100, Some("h")), arq(1, 2, 101, Some("h"))], 0);
        assert!(r.grupos.is_empty());
    }

    #[test]
    fn alocado_informado_vence_o_arredondamento() {
        let mut a = arq(1, 1, 100, None);
        a.tamanho_alocado = Some(512);
        assert_eq!(vol_de(&relatorio(&[a], 4096), 1).bytes_alocados, 512);
    }
}
