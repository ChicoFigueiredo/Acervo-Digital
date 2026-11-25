//! DAG — Acervo Digital (GUI). Abre a casca Bancada com o worker de sessão e o Catálogo padrão.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    via_xwayland();
    let casca = dag_app::app::montar(dag_app::app::ConfigApp::do_ambiente()?)?;
    dag_app::janela::executar(casca, dag_app::TITULO)?;
    Ok(())
}

/// No Wayland, o winit 0.30 derruba a janela ao perder os dados do ponteiro (mouse passando de um
/// monitor para outro: "failed to get pointer data"). Com XWayland disponível, a GUI se reabre
/// pelo X11, que não tem esse problema. `DAG_APP_BACKEND=wayland` mantém o Wayland nativo.
#[cfg(target_os = "linux")]
fn via_xwayland() {
    use std::os::unix::process::CommandExt;
    let tem = |v: &str| std::env::var_os(v).is_some_and(|s| !s.is_empty());
    if !tem("WAYLAND_DISPLAY") || !tem("DISPLAY") || tem("DAG_APP_BACKEND") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // `exec` só volta em caso de erro; aí segue no Wayland mesmo.
    let erro = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env_remove("WAYLAND_DISPLAY")
        .env("DAG_APP_BACKEND", "x11")
        .exec();
    eprintln!("dag-app: não consegui reabrir pelo XWayland ({erro}); seguindo no Wayland");
}
