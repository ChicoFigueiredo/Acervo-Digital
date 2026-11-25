//! Janela nativa: ícone do aplicativo e `app_id` (Wayland) / `WM_CLASS` (X11).
//!
//! O compositor casa a janela com o `dag-app.desktop` pelo `app_id`; sem isso o KDE e o GNOME
//! mostram um ícone genérico (o "W" do Wayland) e agrupam mal na barra de tarefas.

use bancada_casca::eframe::{self, egui};
use bancada_casca::{AppCasca, Casca};
use dag_platform_linux::atalho::{ID_APP, PNG_256};

/// Decodifica um PNG RGBA de 8 bits em dados de ícone da janela.
///
/// # Errors
/// PNG inválido ou em formato de cor inesperado.
pub fn icone_de_png(bytes: &[u8]) -> Result<egui::IconData, png::DecodingError> {
    let mut decodificador = png::Decoder::new(std::io::Cursor::new(bytes));
    decodificador.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut leitor = decodificador.read_info()?;
    let mut buf = vec![0; leitor.output_buffer_size().unwrap_or(0)];
    let info = leitor.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    Ok(egui::IconData {
        rgba: buf,
        width: info.width,
        height: info.height,
    })
}

/// Opções da janela: título, tamanho, ícone embutido e `app_id` `dag-app`.
#[must_use]
pub fn opcoes(titulo: &str) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(titulo)
        .with_app_id(ID_APP)
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([720.0, 480.0]);
    if let Ok(icone) = icone_de_png(PNG_256) {
        viewport = viewport.with_icon(icone);
    }
    eframe::NativeOptions {
        viewport,
        ..Default::default()
    }
}

/// Abre a janela e roda até fechar (no lugar de `Casca::executar`, que não expõe o `app_id`).
///
/// # Errors
/// Falha do eframe ao criar a janela ou o contexto gráfico.
pub fn executar<A: AppCasca + 'static>(casca: Casca<A>, titulo: &str) -> eframe::Result {
    eframe::run_native(
        titulo,
        opcoes(titulo),
        Box::new(move |_cc| Ok(Box::new(casca))),
    )
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn icone_embutido_decodifica_em_256() {
        let i = icone_de_png(PNG_256).unwrap();
        assert_eq!((i.width, i.height), (256, 256));
        assert_eq!(i.rgba.len(), 256 * 256 * 4);
        // Cantos transparentes (rounded square), centro opaco.
        assert_eq!(i.rgba[3], 0);
    }

    #[test]
    fn app_id_casa_com_o_desktop() {
        let o = opcoes("t");
        assert_eq!(o.viewport.app_id.as_deref(), Some("dag-app"));
        assert!(o.viewport.icon.is_some());
    }
}
