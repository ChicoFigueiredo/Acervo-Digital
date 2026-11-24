//! DAG — Acervo Digital (GUI). Abre a casca Bancada com o worker de sessão e o Catálogo padrão.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let casca = dag_app::app::montar(dag_app::app::ConfigApp::do_ambiente()?)?;
    dag_app::janela::executar(casca, dag_app::TITULO)?;
    Ok(())
}
