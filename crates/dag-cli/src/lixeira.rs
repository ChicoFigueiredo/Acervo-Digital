//! `dag lixeira`: mover para a `DAG-Lixeira` do Volume, restaurar e listar (ADR 0005).
//!
//! Mover exige **dupla confirmação**: o CLI pede ao agente a simulação, mostra o que iria
//! (quantidade, bytes, Volume, destino, Única Cópia) e só envia o pedido de verdade depois de
//! duas respostas explícitas do usuário. `--sim --sim` (ou `--confirmar-duas-vezes`) dispensa as
//! perguntas, para scripts. Sem terminal e sem `--sim --sim`, a resposta vazia (EOF) cancela.

use std::io::{BufRead, Write};

use dag_core::ResultadoLixeira;

/// Quantas confirmações o usuário já deu por flag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Confirmacao {
    /// Vezes que `--sim` apareceu.
    pub sim: u8,
    /// `--confirmar-duas-vezes`: equivale a `--sim --sim`.
    pub duas_vezes: bool,
}

impl Confirmacao {
    /// Quantas perguntas ainda precisam ser feitas (0, 1 ou 2).
    #[must_use]
    pub fn perguntas(self) -> u8 {
        if self.duas_vezes {
            0
        } else {
            2u8.saturating_sub(self.sim.min(2))
        }
    }
}

/// Palavra que o usuário digita na segunda confirmação.
pub const PALAVRA: &str = "MOVER";

/// Pergunta o que falta. `Ok(())` só se todas as confirmações foram dadas.
///
/// # Errors
/// Texto explicando que o pedido foi cancelado (resposta negativa ou fim da entrada).
pub fn confirmar(
    r: &ResultadoLixeira,
    conf: Confirmacao,
    entrada: &mut dyn BufRead,
    saida: &mut dyn Write,
) -> Result<(), String> {
    let destinos = r.destinos.join(", ");
    let mut perguntas = conf.perguntas();
    if perguntas == 2 {
        let _ = write!(
            saida,
            "Mover {} arquivo(s) ({}) para {destinos}? Nada é apagado; você pode restaurar. [s/N] ",
            r.total_arquivos,
            dag_core::formato::bytes_humanos(r.total_bytes)
        );
        let _ = saida.flush();
        let resp = linha(entrada)?;
        if !matches!(resp.to_lowercase().as_str(), "s" | "sim") {
            return Err("cancelado na primeira confirmação; nada foi movido".into());
        }
        perguntas -= 1;
    }
    if perguntas == 1 {
        if r.unicas_copias > 0 {
            let _ = writeln!(
                saida,
                "ATENÇÃO: {} arquivo(s) são Única Cópia: não existe outra cópia deles fora da Lixeira.",
                r.unicas_copias
            );
        }
        let _ = write!(
            saida,
            "Segunda confirmação. Esvaziar a DAG-Lixeira é ato seu, fora do DAG. Digite {PALAVRA} para continuar: "
        );
        let _ = saida.flush();
        let resp = linha(entrada)?;
        if resp != PALAVRA {
            return Err("cancelado na segunda confirmação; nada foi movido".into());
        }
    }
    Ok(())
}

fn linha(entrada: &mut dyn BufRead) -> Result<String, String> {
    let mut s = String::new();
    match entrada.read_line(&mut s) {
        Ok(0) => Err(
            "sem confirmação (entrada encerrada); nada foi movido. Em scripts use `--sim --sim`"
                .into(),
        ),
        Ok(_) => Ok(s.trim().to_string()),
        Err(e) => Err(format!("não consegui ler a confirmação: {e}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod testes {
    use super::*;
    use dag_core::AcaoLixeira;

    fn res(unicas: u64) -> ResultadoLixeira {
        ResultadoLixeira {
            acao: AcaoLixeira::Mover,
            simulado: true,
            movidos: vec![],
            recusados: vec![],
            total_arquivos: 3,
            total_bytes: 3000,
            unicas_copias: unicas,
            marcados_agora: 0,
            destinos: vec!["/mnt/d/DAG-Lixeira".into()],
            volumes: vec!["D".into()],
        }
    }

    fn rodar(conf: Confirmacao, entrada: &str, unicas: u64) -> (Result<(), String>, String) {
        let mut ent = entrada.as_bytes();
        let mut sai = Vec::new();
        let r = confirmar(&res(unicas), conf, &mut ent, &mut sai);
        (r, String::from_utf8(sai).unwrap())
    }

    #[test]
    fn duas_perguntas_por_padrao() {
        let (r, saida) = rodar(Confirmacao::default(), "s\nMOVER\n", 0);
        assert!(r.is_ok());
        assert!(saida.contains("[s/N]") && saida.contains("Segunda confirmação"));
    }

    #[test]
    fn uma_resposta_nao_basta_nem_a_palavra_errada() {
        assert!(rodar(Confirmacao::default(), "s\n", 0).0.is_err());
        assert!(rodar(Confirmacao::default(), "s\nmover\n", 0).0.is_err());
        assert!(rodar(Confirmacao::default(), "n\nMOVER\n", 0).0.is_err());
        assert!(rodar(Confirmacao::default(), "", 0).0.is_err());
    }

    #[test]
    fn sim_sim_dispensa_as_perguntas_e_um_sim_dispensa_a_primeira() {
        let (r, saida) = rodar(
            Confirmacao {
                sim: 2,
                duas_vezes: false,
            },
            "",
            0,
        );
        assert!(r.is_ok() && saida.is_empty());
        let (r, _) = rodar(
            Confirmacao {
                sim: 0,
                duas_vezes: true,
            },
            "",
            0,
        );
        assert!(r.is_ok());
        assert!(
            rodar(
                Confirmacao {
                    sim: 1,
                    duas_vezes: false
                },
                "",
                0
            )
            .0
            .is_err()
        );
        assert!(
            rodar(
                Confirmacao {
                    sim: 1,
                    duas_vezes: false
                },
                "MOVER\n",
                0
            )
            .0
            .is_ok()
        );
    }

    #[test]
    fn unica_copia_aparece_na_segunda_pergunta() {
        let (_, saida) = rodar(Confirmacao::default(), "s\nMOVER\n", 2);
        assert!(saida.contains("Única Cópia"));
    }
}
