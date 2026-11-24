//! Geração do arquivo de unidade `dag-agente.service` (serviço **de usuário**).

use std::path::{Path, PathBuf};

use super::ErroServico;

/// Nome da unidade.
pub const NOME_UNIDADE: &str = "dag-agente.service";

/// Parâmetros para gerar a unidade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigUnidade {
    /// Caminho absoluto do binário `dag` (o `ExecStart` roda `<binario> agente`).
    pub binario: PathBuf,
    /// Diretórios de dados/estado/runtime do DAG: únicos graváveis (`ReadWritePaths`).
    pub dirs_gravaveis: Vec<PathBuf>,
    /// Catálogo escolhido explicitamente (vira `Environment=DAG_CATALOGO=`), se não for o padrão.
    pub catalogo: Option<PathBuf>,
}

fn sem_controle(p: &Path) -> Result<&str, ErroServico> {
    let s = p
        .to_str()
        .ok_or_else(|| ErroServico::Invalido(format!("caminho não UTF-8: {}", p.display())))?;
    if s.chars().any(char::is_control) {
        return Err(ErroServico::Invalido(format!(
            "caminho com caracteres de controle: {s:?}"
        )));
    }
    Ok(s)
}

/// Entre aspas no estilo systemd; `%` e `$` são expandidos por ele, então são dobrados.
fn aspas(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '%' => o.push_str("%%"),
            '$' => o.push_str("$$"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Conteúdo da unidade. Não há `[Install]` com `WantedBy` ativado: instalar só escreve o arquivo;
/// habilitar é uma ação separada (`autostart`).
///
/// # Errors
/// [`ErroServico::Invalido`] se o binário não for absoluto ou algum caminho for inválido.
pub fn gerar(c: &ConfigUnidade) -> Result<String, ErroServico> {
    if !c.binario.is_absolute() {
        return Err(ErroServico::Invalido(format!(
            "o binário precisa de caminho absoluto: {}",
            c.binario.display()
        )));
    }
    let bin = sem_controle(&c.binario)?;
    let mut rw = Vec::new();
    for d in &c.dirs_gravaveis {
        if !d.is_absolute() {
            return Err(ErroServico::Invalido(format!(
                "diretório gravável precisa ser absoluto: {}",
                d.display()
            )));
        }
        rw.push(aspas(sem_controle(d)?));
    }
    let mut u = String::new();
    u.push_str(
        "[Unit]\n\
         Description=DAG - agente em segundo plano (catálogo de discos, somente leitura sobre o acervo)\n\
         Documentation=man:dag(1)\n\n\
         [Service]\n\
         Type=simple\n",
    );
    u.push_str(&format!("ExecStart={} agente\n", aspas(bin)));
    if let Some(cat) = &c.catalogo {
        u.push_str(&format!(
            "Environment={}\n",
            aspas(&format!("DAG_CATALOGO={}", sem_controle(cat)?))
        ));
    }
    u.push_str(
        "Restart=on-failure\n\
         RestartSec=5\n\
         # Prioridade conservadora (A-21): não disputa CPU/disco com o uso normal.\n\
         Nice=10\n\
         IOSchedulingClass=idle\n\
         # Endurecimento: o acervo é somente leitura; só os diretórios do DAG são graváveis.\n\
         NoNewPrivileges=yes\n\
         ProtectSystem=strict\n\
         ProtectHome=read-only\n\
         PrivateTmp=yes\n",
    );
    if !rw.is_empty() {
        u.push_str(&format!("ReadWritePaths={}\n", rw.join(" ")));
    }
    u.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(u)
}
