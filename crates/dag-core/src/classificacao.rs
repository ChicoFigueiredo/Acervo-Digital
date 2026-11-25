//! Classificação de formato (A-07) e de finalidade (A-08) de arquivos e pastas.
//!
//! Módulo puro: nenhuma operação de I/O. A verificação por assinatura recebe um
//! trecho de bytes já lido pelo chamador.

mod assinatura;
mod finalidade;
mod formato;

pub use assinatura::{
    BYTES_RECOMENDADOS, Divergencia, EvidenciaFormato, TIPO_OLE, identificar_por_assinatura,
    verificar_divergencia,
};
pub use finalidade::{
    ClassificacaoFinalidade, ConsolidacaoPasta, LIMIAR_DOMINANCIA, ProporcaoFinalidade,
    classificar_finalidade_caminho, consolidar_finalidade_pasta,
};
pub use formato::{
    CategoriaFormato, IdentificacaoExtensao, TIPO_DESCONHECIDO, TipoFormato, extrair_extensao,
    identificar_por_extensao, tipo_por_extensao, total_extensoes_conhecidas,
};

use serde::Serialize;

/// Identificação completa de um arquivo: extensão, assinatura (opcional) e divergência.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentificacaoFormato {
    pub extensao: IdentificacaoExtensao,
    /// Tipo validado pela assinatura, quando o conteúdo foi fornecido e reconhecido.
    pub assinatura: Option<TipoFormato>,
    pub divergencia: Divergencia,
    pub evidencias: Vec<EvidenciaFormato>,
}

impl IdentificacaoFormato {
    /// Melhor tipo conhecido: a assinatura prevalece quando a extensão é
    /// desconhecida ou enganosa; do contrário vale o tipo da extensão.
    #[must_use]
    pub fn tipo_efetivo(&self) -> TipoFormato {
        match self.assinatura {
            Some(a) if self.extensao.desconhecido || self.divergencia.eh_divergente() => a,
            _ => self.extensao.tipo,
        }
    }
}

/// Identifica o formato por extensão e, se `conteudo` for dado, valida por assinatura.
#[must_use]
pub fn identificar_formato(nome: &[u8], conteudo: Option<&[u8]>) -> IdentificacaoFormato {
    let extensao = identificar_por_extensao(nome);
    let assinatura = conteudo.and_then(identificar_por_assinatura);
    let ext_tipo = (!extensao.desconhecido).then_some(extensao.tipo);
    let divergencia = verificar_divergencia(ext_tipo, assinatura);
    let mut evidencias = Vec::new();
    if !extensao.desconhecido {
        evidencias.push(EvidenciaFormato::Extensao);
    }
    if assinatura.is_some() {
        evidencias.push(EvidenciaFormato::Assinatura);
    }
    IdentificacaoFormato {
        extensao,
        assinatura,
        divergencia,
        evidencias,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn extensao_enganosa_e_detectada() {
        let r = identificar_formato(b"foto.jpg", Some(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(r.divergencia, Divergencia::Divergente);
        assert_eq!(r.tipo_efetivo().mime, "image/png");
        assert_eq!(
            r.evidencias,
            [EvidenciaFormato::Extensao, EvidenciaFormato::Assinatura]
        );
    }

    #[test]
    fn docx_zip_nao_diverge_e_mantem_tipo_da_extensao() {
        let r = identificar_formato(b"a.DOCX", Some(b"PK\x03\x04\x14\0"));
        assert_eq!(r.divergencia, Divergencia::Compativel);
        assert_eq!(r.tipo_efetivo().extensao, "docx");
    }

    #[test]
    fn sem_conteudo_so_extensao_e_desconhecida_usa_assinatura() {
        let r = identificar_formato(b"a.pdf", None);
        assert_eq!(r.divergencia, Divergencia::Indeterminado);
        assert_eq!(r.evidencias, [EvidenciaFormato::Extensao]);
        let r = identificar_formato(b"sem_extensao", Some(b"%PDF-1.7"));
        assert!(r.extensao.desconhecido);
        assert_eq!(r.tipo_efetivo().mime, "application/pdf");
        assert_eq!(r.evidencias, [EvidenciaFormato::Assinatura]);
    }
}
