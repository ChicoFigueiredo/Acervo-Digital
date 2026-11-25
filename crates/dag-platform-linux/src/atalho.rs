//! Atalho de desktop: ícones, entrada `.desktop` no menu e lançador na área de trabalho
//! (KDE Plasma e GNOME). Escreve só em diretórios do usuário (`~/.local/share`, área de
//! trabalho); nunca toca nos arquivos catalogados. Os ativos vão embutidos no binário, então
//! funciona a partir de um binário instalado, sem o repositório.
//!
//! A lógica pura (renderizar o `.desktop`, calcular caminhos, copiar, remover) recebe um
//! [`Ambiente`] explícito e é testada com HOME falso; os utilitários do sistema
//! (`update-desktop-database`, `gio`...) só rodam quando pedido e as falhas são ignoradas.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Identificador do aplicativo: nome do ícone, do `.desktop` e `app_id`/`WM_CLASS` da janela.
pub const ID_APP: &str = "dag-app";

/// Tamanhos PNG embutidos, em pixels.
pub const TAMANHOS: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// Ícone vetorial.
pub const SVG: &[u8] = include_bytes!("../../../packaging/icons/dag-app.svg");

/// PNG de 256 px (ícone da janela do eframe).
pub const PNG_256: &[u8] = include_bytes!("../../../packaging/icons/dag-app-256.png");

const PNGS: [(u32, &[u8]); 8] = [
    (
        16,
        include_bytes!("../../../packaging/icons/dag-app-16.png"),
    ),
    (
        24,
        include_bytes!("../../../packaging/icons/dag-app-24.png"),
    ),
    (
        32,
        include_bytes!("../../../packaging/icons/dag-app-32.png"),
    ),
    (
        48,
        include_bytes!("../../../packaging/icons/dag-app-48.png"),
    ),
    (
        64,
        include_bytes!("../../../packaging/icons/dag-app-64.png"),
    ),
    (
        128,
        include_bytes!("../../../packaging/icons/dag-app-128.png"),
    ),
    (256, PNG_256),
    (
        512,
        include_bytes!("../../../packaging/icons/dag-app-512.png"),
    ),
];

/// Modelo do `.desktop`; `@EXEC@` vira o caminho absoluto do `dag-app`.
const MODELO_DESKTOP: &str = include_str!("../../../packaging/linux/dag-app.desktop");

/// Dica para o GNOME, que por padrão não mostra ícones na área de trabalho.
pub const DICA_GNOME: &str = "No GNOME, ícones na área de trabalho exigem a extensão \
\"Desktop Icons NG (DING)\" (pacote gnome-shell-extension-desktop-icons-ng); o atalho do menu \
de aplicativos funciona sem ela.";

/// Onde instalar. Em testes aponta para diretórios temporários.
#[derive(Debug, Clone)]
pub struct Ambiente {
    /// `XDG_DATA_HOME` (padrão `~/.local/share`).
    pub dados: PathBuf,
    /// Área de trabalho do usuário, se existir uma.
    pub desktop: Option<PathBuf>,
    /// Roda `update-desktop-database`, `gio` etc. (falhas ignoradas).
    pub rodar_utilitarios: bool,
}

impl Ambiente {
    /// Ambiente real: `XDG_DATA_HOME`/`HOME` e `xdg-user-dir DESKTOP` (com `~/Desktop` e
    /// `~/Área de Trabalho` de reserva).
    #[must_use]
    pub fn real() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?);
        let dados = std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map_or_else(|| home.join(".local/share"), PathBuf::from);
        Some(Self {
            dados,
            desktop: descobrir_desktop(&home),
            // DAG_SEM_UTILITARIOS: não chama update-desktop-database, gio etc. (testes).
            rodar_utilitarios: std::env::var_os("DAG_SEM_UTILITARIOS").is_none(),
        })
    }
}

fn descobrir_desktop(home: &Path) -> Option<PathBuf> {
    let xdg = Command::new("xdg-user-dir")
        .arg("DESKTOP")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        // xdg-user-dir devolve o próprio HOME quando não há diretório configurado.
        .filter(|p| !p.as_os_str().is_empty() && p != home && p.is_dir());
    xdg.or_else(|| {
        ["Desktop", "Área de Trabalho", "Área de trabalho"]
            .iter()
            .map(|n| home.join(n))
            .find(|p| p.is_dir())
    })
}

/// Caminho do `dag-app`: ao lado do executável atual (`dag`) ou no `PATH`.
#[must_use]
pub fn localizar_dag_app() -> Option<PathBuf> {
    let vizinho = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join(ID_APP)))
        .filter(|p| p.is_file());
    vizinho.or_else(|| {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(ID_APP))
            .find(|p| p.is_file())
    })
}

/// Texto do `.desktop` para o executável `exec`.
#[must_use]
pub fn renderizar_desktop(exec: &Path) -> String {
    // Na chave Exec, espaços e aspas exigem aspas e escapes (Desktop Entry Spec).
    let e = exec.to_string_lossy();
    let exec = if e.chars().any(|c| " \t\n\"'\\><~|&;$*?#()`".contains(c)) {
        let esc: String = e
            .chars()
            .flat_map(|c| {
                if "\"`$\\".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        format!("\"{esc}\"")
    } else {
        e.into_owned()
    };
    MODELO_DESKTOP.replace("@EXEC@", &exec)
}

/// Caminhos que a instalação escreve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caminhos {
    pub icones: Vec<PathBuf>,
    pub svg: PathBuf,
    pub menu: PathBuf,
    pub desktop: Option<PathBuf>,
}

impl Caminhos {
    #[must_use]
    pub fn de(amb: &Ambiente) -> Self {
        let hicolor = amb.dados.join("icons/hicolor");
        let arquivo = format!("{ID_APP}.desktop");
        Self {
            icones: TAMANHOS
                .iter()
                .map(|t| hicolor.join(format!("{t}x{t}/apps/{ID_APP}.png")))
                .collect(),
            svg: hicolor.join(format!("scalable/apps/{ID_APP}.svg")),
            menu: amb.dados.join("applications").join(&arquivo),
            desktop: amb.desktop.as_ref().map(|d| d.join(&arquivo)),
        }
    }

    fn todos(&self) -> impl Iterator<Item = &PathBuf> {
        self.icones
            .iter()
            .chain([&self.svg, &self.menu])
            .chain(self.desktop.iter())
    }
}

/// O que foi feito, para mostrar ao usuário.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relatorio {
    pub escritos: Vec<PathBuf>,
    pub removidos: Vec<PathBuf>,
    pub dicas: Vec<String>,
}

fn escrever(caminho: &Path, bytes: &[u8], modo: u32) -> io::Result<()> {
    if let Some(pai) = caminho.parent() {
        fs::create_dir_all(pai)?;
    }
    // Idempotente: conteúdo igual não é reescrito (mantém o mtime e evita ruído nos caches).
    if fs::read(caminho).is_ok_and(|atual| atual == bytes) {
        return fs::set_permissions(caminho, fs::Permissions::from_mode(modo));
    }
    fs::write(caminho, bytes)?;
    fs::set_permissions(caminho, fs::Permissions::from_mode(modo))
}

fn rodar(programa: &str, args: &[&str]) {
    let _ = Command::new(programa)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn atualizar_caches(amb: &Ambiente, caminhos: &Caminhos) {
    if !amb.rodar_utilitarios {
        return;
    }
    if let Some(apps) = caminhos.menu.parent() {
        rodar("update-desktop-database", &[&apps.to_string_lossy()]);
    }
    let hicolor = amb.dados.join("icons/hicolor");
    rodar(
        "gtk-update-icon-cache",
        &["-f", "-t", &hicolor.to_string_lossy()],
    );
    rodar("kbuildsycoca6", &[]);
    rodar("kbuildsycoca5", &[]);
}

/// Instala ícones, entrada de menu e lançador na área de trabalho. Idempotente.
///
/// # Errors
/// Falha de E/S ao escrever em `~/.local/share` ou na área de trabalho.
pub fn instalar(amb: &Ambiente, exec: &Path) -> io::Result<Relatorio> {
    let caminhos = Caminhos::de(amb);
    let mut rel = Relatorio::default();
    for ((_, png), destino) in PNGS.iter().zip(&caminhos.icones) {
        escrever(destino, png, 0o644)?;
        rel.escritos.push(destino.clone());
    }
    escrever(&caminhos.svg, SVG, 0o644)?;
    rel.escritos.push(caminhos.svg.clone());
    let texto = renderizar_desktop(exec);
    escrever(&caminhos.menu, texto.as_bytes(), 0o644)?;
    rel.escritos.push(caminhos.menu.clone());
    match &caminhos.desktop {
        Some(d) => {
            // Executável: o KDE só confia no lançador assim; o GNOME também pede o metadado.
            escrever(d, texto.as_bytes(), 0o755)?;
            if amb.rodar_utilitarios {
                rodar(
                    "gio",
                    &["set", &d.to_string_lossy(), "metadata::trusted", "true"],
                );
            }
            rel.escritos.push(d.clone());
            rel.dicas.push(DICA_GNOME.to_string());
        }
        None => rel.dicas.push(
            "Não achei a pasta da área de trabalho (xdg-user-dir DESKTOP, ~/Desktop); \
             só o menu de aplicativos foi configurado."
                .into(),
        ),
    }
    atualizar_caches(amb, &caminhos);
    Ok(rel)
}

/// Desfaz [`instalar`]: remove só os arquivos que ela cria; o que não existe é ignorado.
///
/// # Errors
/// Falha de E/S diferente de "não encontrado" ao remover.
pub fn remover(amb: &Ambiente) -> io::Result<Relatorio> {
    let caminhos = Caminhos::de(amb);
    let mut rel = Relatorio::default();
    for p in caminhos.todos() {
        match fs::remove_file(p) {
            Ok(()) => rel.removidos.push(p.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    atualizar_caches(amb, &caminhos);
    Ok(rel)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn ambiente(tmp: &Path) -> Ambiente {
        Ambiente {
            dados: tmp.join("home/.local/share"),
            desktop: Some(tmp.join("home/Desktop")),
            rodar_utilitarios: false,
        }
    }

    #[test]
    fn desktop_tem_os_campos_esperados() {
        let t = renderizar_desktop(Path::new("/home/u/.local/bin/dag-app"));
        assert!(t.contains("Name=DAG — Acervo Digital"));
        assert!(t.contains("Exec=/home/u/.local/bin/dag-app\n"));
        assert!(t.contains("Icon=dag-app"));
        assert!(t.contains("StartupWMClass=dag-app"));
        assert!(t.contains("Categories=Utility;System;FileTools;"));
        assert!(t.contains("Keywords=") && t.contains("acervo"));
        assert!(!t.contains("@EXEC@"));
    }

    #[test]
    fn exec_com_espaco_vai_entre_aspas() {
        let t = renderizar_desktop(Path::new("/home/u/meus apps/dag-app"));
        assert!(t.contains("Exec=\"/home/u/meus apps/dag-app\"\n"));
    }

    #[test]
    fn caminhos_seguem_o_padrao_xdg() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Caminhos::de(&ambiente(tmp.path()));
        assert_eq!(c.icones.len(), TAMANHOS.len());
        assert!(c.icones[5].ends_with("icons/hicolor/128x128/apps/dag-app.png"));
        assert!(c.svg.ends_with("icons/hicolor/scalable/apps/dag-app.svg"));
        assert!(c.menu.ends_with("share/applications/dag-app.desktop"));
        assert!(c.desktop.unwrap().ends_with("Desktop/dag-app.desktop"));
    }

    #[test]
    fn instalar_escreve_tudo_e_e_idempotente() {
        let tmp = tempfile::tempdir().unwrap();
        let amb = ambiente(tmp.path());
        fs::create_dir_all(amb.desktop.as_ref().unwrap()).unwrap();
        let exec = Path::new("/opt/bin/dag-app");
        let r1 = instalar(&amb, exec).unwrap();
        let c = Caminhos::de(&amb);
        assert_eq!(r1.escritos.len(), 8 + 1 + 1 + 1);
        for p in c.todos() {
            assert!(p.is_file(), "{p:?}");
        }
        let modo = fs::metadata(c.desktop.as_ref().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(modo & 0o111, 0o111, "lançador executável");
        assert!(r1.dicas.iter().any(|d| d.contains("Desktop Icons NG")));
        assert!(fs::read(&c.icones[0]).unwrap().starts_with(b"\x89PNG"));
        let antes = fs::read_to_string(&c.menu).unwrap();
        assert!(antes.contains("Exec=/opt/bin/dag-app"));
        // Segunda vez: mesmo resultado. Trocar o executável atualiza o Exec.
        instalar(&amb, exec).unwrap();
        assert_eq!(fs::read_to_string(&c.menu).unwrap(), antes);
        instalar(&amb, Path::new("/outro/dag-app")).unwrap();
        assert!(
            fs::read_to_string(&c.menu)
                .unwrap()
                .contains("Exec=/outro/dag-app")
        );
    }

    #[test]
    fn sem_area_de_trabalho_instala_so_o_menu() {
        let tmp = tempfile::tempdir().unwrap();
        let mut amb = ambiente(tmp.path());
        amb.desktop = None;
        let r = instalar(&amb, Path::new("/x/dag-app")).unwrap();
        assert!(Caminhos::de(&amb).menu.is_file());
        assert_eq!(r.escritos.len(), 10);
        assert_eq!(r.dicas.len(), 1);
    }

    #[test]
    fn remover_desfaz_e_e_idempotente() {
        let tmp = tempfile::tempdir().unwrap();
        let amb = ambiente(tmp.path());
        fs::create_dir_all(amb.desktop.as_ref().unwrap()).unwrap();
        // Arquivo alheio na área de trabalho não é tocado.
        let alheio = amb.desktop.as_ref().unwrap().join("outro.desktop");
        fs::write(&alheio, "x").unwrap();
        instalar(&amb, Path::new("/x/dag-app")).unwrap();
        let r = remover(&amb).unwrap();
        assert_eq!(r.removidos.len(), 11);
        for p in Caminhos::de(&amb).todos() {
            assert!(!p.exists(), "{p:?}");
        }
        assert!(alheio.is_file());
        assert!(remover(&amb).unwrap().removidos.is_empty());
    }

    #[test]
    fn ativos_embutidos_sao_validos() {
        assert!(SVG.starts_with(b"<svg"));
        for (t, png) in PNGS {
            assert!(png.starts_with(b"\x89PNG"), "{t}");
        }
    }
}
