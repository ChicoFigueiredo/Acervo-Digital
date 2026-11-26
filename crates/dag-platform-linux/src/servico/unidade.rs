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
    /// Catálogo servido (vira `--catalogo` no `ExecStart`); `None` = o padrão do ambiente.
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
         Description=DAG - agente em segundo plano (catálogo de discos; só move para a DAG-Lixeira, nunca apaga)\n\
         Documentation=man:dag(1)\n\n\
         [Service]\n\
         Type=simple\n",
    );
    match &c.catalogo {
        Some(cat) => u.push_str(&format!(
            "ExecStart={} --catalogo {} agente\n",
            aspas(bin),
            aspas(sem_controle(cat)?)
        )),
        None => u.push_str(&format!("ExecStart={} agente\n", aspas(bin))),
    }
    u.push_str(
        "Restart=on-failure\n\
         RestartSec=5\n\
         # Parada limpa (SIGTERM): dá tempo de gravar a fila de retomada do hash.\n\
         TimeoutStopSec=120\n\
         # Prioridade conservadora (A-21): não disputa CPU/disco com o uso normal.\n\
         Nice=10\n\
         IOSchedulingClass=idle\n\
         # Endurecimento (ADR 0005): o DAG nunca apaga nem altera conteúdo; a única escrita nos\n\
         # arquivos catalogados é mover para a DAG-Lixeira do mesmo Volume e de volta (rename).\n\
         # Por isso a unidade não deixa home nem os pontos de montagem somente leitura: eles deixariam os\n\
         # Volumes catalogados (home, /mnt, /media) somente leitura para o agente e o rename\n\
         # falharia. A garantia de não apagar está no código e no teste estático de segurança.\n\
         # ProtectSystem=full mantém /usr, /boot e /etc somente leitura.\n\
         NoNewPrivileges=yes\n\
         ProtectSystem=full\n\
         PrivateTmp=yes\n\
         # O systemd cria $XDG_RUNTIME_DIR/dag (socket do agente) antes de montar o namespace: sem\n\
         # isso, depois de um reboot a pasta não existe e ReadWritePaths falha (226/NAMESPACE).\n\
         RuntimeDirectory=dag\n\
         RuntimeDirectoryPreserve=yes\n",
    );
    if !rw.is_empty() {
        u.push_str(&format!("ReadWritePaths={}\n", rw.join(" ")));
    }
    u.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(u)
}
