//! Regras puras do domínio, sobre structs simples.

use crate::{
    ArquivoId, DispositivoId, EstadoMarcacao, HashCompleto, OrigemMarcacao, TipoMarcacao, VolumeId,
};

/// Cópia de um arquivo dentro de um Grupo de Duplicados.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Copia {
    pub arquivo: ArquivoId,
    pub volume: VolumeId,
    pub dispositivo: DispositivoId,
}

/// Escolhe a Cópia Principal: escolha manual (se for cópia do grupo) vence; senão vale a
/// Prioridade de Volume (índice menor em `prioridade` ganha; Volumes fora da lista vêm depois);
/// empates são resolvidos pelo menor `ArquivoId`, para ser determinístico.
#[must_use]
pub fn escolher_copia_principal(
    copias: &[Copia],
    prioridade: &[VolumeId],
    escolha_manual: Option<ArquivoId>,
) -> Option<ArquivoId> {
    if let Some(m) = escolha_manual
        && copias.iter().any(|c| c.arquivo == m)
    {
        return Some(m);
    }
    copias
        .iter()
        .min_by_key(|c| {
            let rank = prioridade
                .iter()
                .position(|v| *v == c.volume)
                .unwrap_or(usize::MAX);
            (rank, c.arquivo)
        })
        .map(|c| c.arquivo)
}

/// Marcação sobre uma cópia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marcacao {
    pub arquivo: ArquivoId,
    pub tipo: TipoMarcacao,
    pub estado: EstadoMarcacao,
    pub origem: OrigemMarcacao,
    /// Hash Completo do arquivo no momento da marcação.
    pub hash_na_marcacao: HashCompleto,
}

impl Marcacao {
    /// Só Marcações `Ativa` entram em contagens e na Cobertura.
    #[must_use]
    pub fn conta(&self) -> bool {
        self.estado == EstadoMarcacao::Ativa
    }

    /// Reavalia contra o hash atualmente guardado: se mudou, vira `Pendente`.
    /// Não volta sozinha a `Ativa`, mesmo que o hash volte ao original.
    pub fn reavaliar(&mut self, hash_atual: &HashCompleto) {
        if self.hash_na_marcacao != *hash_atual {
            self.estado = EstadoMarcacao::Pendente;
        }
    }

    /// Confirmação do usuário: adota o hash atual e reativa.
    pub fn confirmar(&mut self, hash_atual: HashCompleto) {
        self.hash_na_marcacao = hash_atual;
        self.estado = EstadoMarcacao::Ativa;
    }
}

/// Uma cópia só pode ter uma Marcação (Backup ou Deleção, nunca as duas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("o arquivo {arquivo} já tem Marcação ({existente:?})")]
pub struct CopiaJaMarcada {
    pub arquivo: ArquivoId,
    pub existente: TipoMarcacao,
}

/// Valida uma nova Marcação contra as existentes.
///
/// # Errors
/// [`CopiaJaMarcada`] se `arquivo` já tem Marcação de qualquer tipo ou estado.
pub fn validar_nova_marcacao(
    existentes: &[Marcacao],
    arquivo: ArquivoId,
) -> Result<(), CopiaJaMarcada> {
    match existentes.iter().find(|m| m.arquivo == arquivo) {
        Some(m) => Err(CopiaJaMarcada {
            arquivo,
            existente: m.tipo,
        }),
        None => Ok(()),
    }
}

/// Cobertura: o arquivo cuja cópia está em `dispositivo` tem Cobertura se alguma cópia do grupo
/// tem Marcação de Backup ativa em outro Dispositivo Físico. Outro Volume do mesmo dispositivo
/// não conta; Marcação Pendente também não.
#[must_use]
pub fn tem_cobertura(dispositivo: DispositivoId, copias: &[Copia], marcacoes: &[Marcacao]) -> bool {
    marcacoes
        .iter()
        .filter(|m| m.conta() && m.tipo == TipoMarcacao::Backup)
        .any(|m| {
            copias
                .iter()
                .any(|c| c.arquivo == m.arquivo && c.dispositivo != dispositivo)
        })
}

/// Cópia de um Grupo de Duplicados com a Marcação que vale para ela (própria ou de regra).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopiaMarcada {
    pub copia: Copia,
    pub marcacao: Option<(TipoMarcacao, EstadoMarcacao)>,
}

/// Divisão do espaço recuperável de um Grupo de Duplicados (A-31).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DivisaoEspaco {
    /// Cópias com Marcação para Deleção ativa.
    pub marcadas: Vec<ArquivoId>,
    /// Cópias redundantes sem marcação (estimativa).
    pub estimadas: Vec<ArquivoId>,
}

/// Espaço recuperável de um grupo. As cópias são Arquivos Físicos distintos (hard links já
/// contados uma vez), então cada cópia vale o tamanho do arquivo.
///
/// * `marcadas`: toda cópia com Marcação para Deleção **ativa**.
/// * `estimadas`: cópias sem Marcação além da Cópia Principal. Cópia com Marcação de Backup
///   fica de fora (preservada), Marcação Pendente sai de todas as contas, e ao menos uma cópia
///   sobrevive sempre: se nada fica de pé (Principal marcada para Deleção, sem Backup nem
///   Pendente), a de menor id entre as sem marcação é poupada.
#[must_use]
pub fn espaco_recuperavel(copias: &[CopiaMarcada], principal: Option<ArquivoId>) -> DivisaoEspaco {
    let mut r = DivisaoEspaco::default();
    let mut sem_marca: Vec<ArquivoId> = Vec::new();
    let mut sobreviventes = 0usize;
    for c in copias {
        match c.marcacao {
            Some((TipoMarcacao::Delecao, EstadoMarcacao::Ativa)) => {
                r.marcadas.push(c.copia.arquivo)
            }
            Some((TipoMarcacao::Backup, EstadoMarcacao::Ativa) | (_, EstadoMarcacao::Pendente)) => {
                sobreviventes += 1;
            }
            None => sem_marca.push(c.copia.arquivo),
        }
    }
    sem_marca.sort();
    for a in &sem_marca {
        if Some(*a) == principal {
            sobreviventes += 1;
        } else {
            r.estimadas.push(*a);
        }
    }
    if sobreviventes == 0 && !r.estimadas.is_empty() {
        // Nada ficaria de pé: poupa a de menor id.
        r.estimadas.remove(0);
    }
    r.marcadas.sort();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(arq: i64, vol: i64, disp: i64) -> Copia {
        Copia {
            arquivo: ArquivoId(arq),
            volume: VolumeId(vol),
            dispositivo: DispositivoId(disp),
        }
    }

    fn m(arq: i64, tipo: TipoMarcacao, hash: u8) -> Marcacao {
        Marcacao {
            arquivo: ArquivoId(arq),
            tipo,
            estado: EstadoMarcacao::Ativa,
            origem: OrigemMarcacao::Manual,
            hash_na_marcacao: HashCompleto([hash; 32]),
        }
    }

    #[test]
    fn principal_por_prioridade() {
        let copias = [c(1, 10, 1), c(2, 20, 2), c(3, 30, 3)];
        let prio = [VolumeId(30), VolumeId(10)];
        assert_eq!(
            escolher_copia_principal(&copias, &prio, None),
            Some(ArquivoId(3))
        );
    }

    #[test]
    fn principal_volume_fora_da_lista_vem_depois_e_empate_determinístico() {
        let copias = [c(5, 99, 1), c(4, 98, 2), c(6, 10, 3)];
        assert_eq!(
            escolher_copia_principal(&copias, &[VolumeId(10)], None),
            Some(ArquivoId(6))
        );
        assert_eq!(
            escolher_copia_principal(&copias, &[], None),
            Some(ArquivoId(4))
        );
        assert_eq!(escolher_copia_principal(&[], &[], None), None);
    }

    #[test]
    fn principal_manual_vence_prioridade() {
        let copias = [c(1, 10, 1), c(2, 20, 2)];
        let prio = [VolumeId(10)];
        assert_eq!(
            escolher_copia_principal(&copias, &prio, Some(ArquivoId(2))),
            Some(ArquivoId(2))
        );
        // escolha manual fora do grupo é ignorada
        assert_eq!(
            escolher_copia_principal(&copias, &prio, Some(ArquivoId(9))),
            Some(ArquivoId(1))
        );
    }

    #[test]
    fn cobertura_exige_outro_dispositivo() {
        let copias = [c(1, 10, 1), c(2, 11, 1), c(3, 20, 2)];
        // backup em outro volume do MESMO dispositivo: sem cobertura
        let ms = [m(2, TipoMarcacao::Backup, 1)];
        assert!(!tem_cobertura(DispositivoId(1), &copias, &ms));
        // backup em outro dispositivo: cobertura
        let ms = [m(3, TipoMarcacao::Backup, 1)];
        assert!(tem_cobertura(DispositivoId(1), &copias, &ms));
    }

    #[test]
    fn cobertura_ignora_delecao_e_pendente() {
        let copias = [c(1, 10, 1), c(3, 20, 2)];
        assert!(!tem_cobertura(
            DispositivoId(1),
            &copias,
            &[m(3, TipoMarcacao::Delecao, 1)]
        ));
        let mut pend = m(3, TipoMarcacao::Backup, 1);
        pend.estado = EstadoMarcacao::Pendente;
        assert!(!tem_cobertura(DispositivoId(1), &copias, &[pend]));
        assert!(!tem_cobertura(DispositivoId(1), &copias, &[]));
    }

    #[test]
    fn uma_marcacao_por_copia() {
        let ex = [m(1, TipoMarcacao::Backup, 1)];
        let e = validar_nova_marcacao(&ex, ArquivoId(1)).unwrap_err();
        assert_eq!(e.existente, TipoMarcacao::Backup);
        assert!(validar_nova_marcacao(&ex, ArquivoId(2)).is_ok());
    }

    #[test]
    fn hash_diferente_torna_pendente_e_fora_das_contas() {
        let mut mk = m(1, TipoMarcacao::Backup, 1);
        mk.reavaliar(&HashCompleto([1; 32]));
        assert!(mk.conta());
        mk.reavaliar(&HashCompleto([2; 32]));
        assert_eq!(mk.estado, EstadoMarcacao::Pendente);
        assert!(!mk.conta());
        // hash voltando ao original não reativa sozinho
        mk.reavaliar(&HashCompleto([1; 32]));
        assert!(!mk.conta());
        mk.confirmar(HashCompleto([2; 32]));
        assert!(mk.conta());
        assert_eq!(mk.hash_na_marcacao, HashCompleto([2; 32]));
    }

    fn cm(
        arq: i64,
        vol: i64,
        disp: i64,
        m: Option<(TipoMarcacao, EstadoMarcacao)>,
    ) -> CopiaMarcada {
        CopiaMarcada {
            copia: c(arq, vol, disp),
            marcacao: m,
        }
    }

    #[test]
    fn espaco_marcadas_estimadas_e_backup() {
        use EstadoMarcacao::{Ativa, Pendente};
        use TipoMarcacao::{Backup, Delecao};
        // 1 principal sem marca, 2 deleção, 3 backup, 4 sem marca, 5 deleção pendente
        let cs = [
            cm(1, 1, 1, None),
            cm(2, 2, 2, Some((Delecao, Ativa))),
            cm(3, 3, 3, Some((Backup, Ativa))),
            cm(4, 4, 4, None),
            cm(5, 5, 5, Some((Delecao, Pendente))),
        ];
        let d = espaco_recuperavel(&cs, Some(ArquivoId(1)));
        assert_eq!(d.marcadas, vec![ArquivoId(2)]);
        assert_eq!(d.estimadas, vec![ArquivoId(4)]);
    }

    #[test]
    fn espaco_sempre_poupa_uma_copia() {
        use EstadoMarcacao::Ativa;
        use TipoMarcacao::Delecao;
        // Principal marcada para deleção e as outras sem marca: poupa a de menor id.
        let cs = [
            cm(1, 1, 1, Some((Delecao, Ativa))),
            cm(2, 2, 2, None),
            cm(3, 3, 3, None),
        ];
        let d = espaco_recuperavel(&cs, Some(ArquivoId(1)));
        assert_eq!(d.marcadas, vec![ArquivoId(1)]);
        assert_eq!(d.estimadas, vec![ArquivoId(3)]);
    }
}
