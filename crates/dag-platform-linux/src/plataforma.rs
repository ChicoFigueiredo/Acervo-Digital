//! Implementação Linux do contrato [`dag_core::Plataforma`] (A-22).

use dag_core::{ErroPlataforma, Mecanismos, Plataforma, SistemaOperacional, VolumeDescoberto};

use crate::descobrir_volumes;

/// A única plataforma implementada na Fase A.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlataformaLinux;

impl Plataforma for PlataformaLinux {
    fn sistema(&self) -> SistemaOperacional {
        SistemaOperacional::Linux
    }

    fn mecanismos(&self) -> Mecanismos {
        Mecanismos {
            monitor: "inotify",
            servico: "systemd (serviço de usuário)",
            elevacao: "Polkit (pkexec) + helper restrito",
        }
    }

    fn descobrir_volumes(&self) -> Result<Vec<VolumeDescoberto>, ErroPlataforma> {
        descobrir_volumes().map_err(|e| ErroPlataforma(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrato_linux_descreve_os_mecanismos_e_descobre_volumes() {
        let p: &dyn Plataforma = &PlataformaLinux;
        assert_eq!(p.sistema(), SistemaOperacional::Linux);
        assert_eq!(p.sistema().nome(), "linux");
        let m = p.mecanismos();
        assert_eq!(m.monitor, "inotify");
        assert!(m.servico.contains("systemd") && m.elevacao.contains("Polkit"));
        assert!(!p.descobrir_volumes().unwrap().is_empty());
    }
}
