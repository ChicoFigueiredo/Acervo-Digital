//! Adaptador do [`Classificador`] do scanner para a classificação por extensão do `dag-core`.
//! Só olha o nome: nunca abre o conteúdo (incremento 2).

use dag_core::identificar_por_extensao;
use dag_scanner::{Classificador, FonteEvidencia, TipoDetectado};

/// Extensão → MIME/descrição/categoria. Extensão ausente ou desconhecida: sem tipo (`None`),
/// em vez de inventar `application/octet-stream`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClassificadorPorExtensao;

impl Classificador for ClassificadorPorExtensao {
    fn tipo_do_arquivo(&self, nome_raw: &[u8]) -> Option<TipoDetectado> {
        let id = identificar_por_extensao(nome_raw);
        if id.desconhecido {
            return None;
        }
        let categoria = serde_json::to_value(id.tipo.categoria)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        Some(TipoDetectado {
            extensao: id.extensao_normalizada,
            mime: Some(id.tipo.mime.to_string()),
            descricao: Some(id.tipo.descricao.to_string()),
            categoria,
            fonte: FonteEvidencia::Extensao,
            confianca: None,
        })
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn pdf_e_desconhecido() {
        let t = ClassificadorPorExtensao.tipo_do_arquivo(b"a/Relatorio.PDF");
        let t = t.unwrap();
        assert_eq!(t.mime.as_deref(), Some("application/pdf"));
        assert_eq!(t.categoria.as_deref(), Some("documento"));
        assert_eq!(t.extensao.as_deref(), Some("pdf"));
        assert!(
            ClassificadorPorExtensao
                .tipo_do_arquivo(b"semextensao")
                .is_none()
        );
        assert!(
            ClassificadorPorExtensao
                .tipo_do_arquivo(b"x.zzzqq")
                .is_none()
        );
    }
}
