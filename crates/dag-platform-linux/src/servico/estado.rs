//! Estado do serviço e interpretação da saída de `systemctl --user show`/`loginctl show-user`.

use std::collections::HashMap;

/// `ActiveState` do systemd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Atividade {
    Ativo,
    Inativo,
    Falhou,
    /// `activating`, `deactivating`, `reloading` ou desconhecido.
    Outro(String),
}

impl Atividade {
    #[must_use]
    pub fn rotulo(&self) -> &str {
        match self {
            Self::Ativo => "ativo",
            Self::Inativo => "inativo",
            Self::Falhou => "falhou",
            Self::Outro(s) => s,
        }
    }
}

/// Modo persistido em `service_state` (`parado`, `sob_demanda`, `continuo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModoServico {
    /// Sem serviço instalado.
    Parado,
    /// Instalado, sem inicialização automática.
    SobDemanda,
    /// Instalado com inicialização automática.
    Continuo,
}

impl ModoServico {
    #[must_use]
    pub fn como_texto(self) -> &'static str {
        match self {
            Self::Parado => "parado",
            Self::SobDemanda => "sob_demanda",
            Self::Continuo => "continuo",
        }
    }
}

/// Estado completo, para `dag servico estado`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstadoServico {
    pub instalado: bool,
    /// `UnitFileState == enabled`.
    pub autostart: bool,
    pub atividade: Atividade,
    pub main_pid: Option<u32>,
    /// `ActiveEnterTimestamp` bruto do systemd (vazio se nunca ativou).
    pub desde: Option<String>,
    /// `None` = não foi possível consultar o loginctl.
    pub linger: Option<bool>,
    pub caminho_unidade: String,
}

impl EstadoServico {
    #[must_use]
    pub fn nao_instalado(caminho_unidade: String, linger: Option<bool>) -> Self {
        Self {
            instalado: false,
            autostart: false,
            atividade: Atividade::Inativo,
            main_pid: None,
            desde: None,
            linger,
            caminho_unidade,
        }
    }

    /// Modo desejado: o que a configuração do usuário pede.
    #[must_use]
    pub fn modo_desejado(&self) -> ModoServico {
        match (self.instalado, self.autostart) {
            (false, _) => ModoServico::Parado,
            (true, false) => ModoServico::SobDemanda,
            (true, true) => ModoServico::Continuo,
        }
    }

    /// Modo atual: o que de fato está acontecendo agora.
    #[must_use]
    pub fn modo_atual(&self) -> ModoServico {
        match (self.atividade == Atividade::Ativo, self.autostart) {
            (false, _) => ModoServico::Parado,
            (true, false) => ModoServico::SobDemanda,
            (true, true) => ModoServico::Continuo,
        }
    }

    /// Texto para humanos, explicando a diferença entre serviço de usuário e linger.
    #[must_use]
    pub fn descrever(&self) -> String {
        if !self.instalado {
            let mut t = format!(
                "serviço: não instalado ({}). Nada roda em segundo plano; o monitor funciona só enquanto a GUI/CLI estiver aberta. Instale com `dag servico instalar`.",
                self.caminho_unidade
            );
            if let Some(l) = self.linger {
                t.push_str(&format!("\nlinger do usuário: {}", sim_nao(l)));
            }
            return t;
        }
        let mut t = format!(
            "serviço: instalado ({})\ninicialização automática: {}\nsituação: {}",
            self.caminho_unidade,
            if self.autostart {
                "ligada"
            } else {
                "desligada"
            },
            self.atividade.rotulo()
        );
        if let Some(p) = self.main_pid.filter(|p| *p != 0) {
            t.push_str(&format!("\nPID principal: {p}"));
        }
        if let Some(d) = self.desde.as_deref().filter(|d| !d.is_empty()) {
            t.push_str(&format!("\nativo desde: {d}"));
        }
        match self.linger {
            Some(l) => {
                t.push_str(&format!("\nlinger do usuário: {}", sim_nao(l)));
                if self.autostart && !l {
                    t.push_str(
                        "\naviso: sem linger o serviço de usuário só sobe no seu primeiro login e para no último logout. Para mantê-lo desde o boot, sem sessão aberta, ative explicitamente: `dag servico linger on` (pode pedir autorização do Polkit).",
                    );
                }
            }
            None => t.push_str("\nlinger do usuário: desconhecido (loginctl indisponível)"),
        }
        t
    }
}

fn sim_nao(b: bool) -> &'static str {
    if b { "ligado" } else { "desligado" }
}

/// Interpreta linhas `Chave=valor`.
#[must_use]
pub fn parse_show(texto: &str) -> HashMap<String, String> {
    texto
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Monta o estado de uma unidade que existe, a partir de `systemctl --user show`.
#[must_use]
pub fn de_show(texto: &str, caminho_unidade: String, linger: Option<bool>) -> EstadoServico {
    let m = parse_show(texto);
    let get = |k: &str| m.get(k).map(String::as_str).unwrap_or("");
    let atividade = match get("ActiveState") {
        "active" => Atividade::Ativo,
        "inactive" | "" => Atividade::Inativo,
        "failed" => Atividade::Falhou,
        o => Atividade::Outro(o.to_string()),
    };
    EstadoServico {
        instalado: get("LoadState") != "not-found",
        autostart: get("UnitFileState") == "enabled",
        atividade,
        main_pid: get("MainPID").parse().ok(),
        desde: Some(get("ActiveEnterTimestamp").to_string()).filter(|s| !s.is_empty()),
        linger,
        caminho_unidade,
    }
}

/// `loginctl show-user -p Linger` → `Linger=yes`.
#[must_use]
pub fn parse_linger(texto: &str) -> Option<bool> {
    match parse_show(texto).get("Linger").map(String::as_str) {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ => None,
    }
}
