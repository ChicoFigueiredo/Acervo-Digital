//! Domínio do DAG, independente de plataforma e sem I/O.
//!
//! Termos seguem o glossário em `CONTEXT.md`. O módulo [`comando`] é o contrato
//! compartilhado entre GUI, CLI e MCP (clientes) e o worker (único escritor).

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod classificacao;
pub mod comando;
pub mod descoberta;
pub mod duplicados;
pub mod formato;
pub mod hash;
pub mod ids;
pub mod regras;
pub mod tipos;

pub use classificacao::{
    BYTES_RECOMENDADOS, CategoriaFormato, ClassificacaoFinalidade, ConsolidacaoPasta, Divergencia,
    EvidenciaFormato, IdentificacaoFormato, ProporcaoFinalidade, TipoFormato,
    classificar_finalidade_caminho, consolidar_finalidade_pasta, identificar_formato,
    identificar_por_assinatura, identificar_por_extensao, verificar_divergencia,
};
pub use comando::{
    AcaoServico, Alvo, Comando, EntradaExploracao, Pedido, Resposta, ResumoCatalogo,
    StatusExecucao, TipoEntradaExploracao, VolumeInfo,
};
pub use descoberta::{DispositivoDescoberto, TipoDispositivo, VolumeDescoberto};
pub use duplicados::{
    AlertaUnicaCopia, ArquivoSemCobertura, ClasseRecuperavel, CoberturaArquivo, CoberturaPasta,
    CoberturaVolume, CopiaGrupo, DadosRelatorio, DetalheGrupo, EscopoMarcacao, EspacoItem,
    FormatoExportacao, GrupoDuplicados, LIMITACOES_ESPACO, LinhaMarcacao, MarcacaoDaCopia,
    MarcacaoInfo, ModoPrincipal, NivelEspaco, OrdemDuplicados, ResultadoMarcacao, ResumoDuplicados,
    ResumoHash, ResumoVerificacao, TipoRelatorio,
};
pub use hash::{HashCompleto, HashInvalido};
pub use ids::{ArquivoId, DispositivoId, ExecucaoId, GrupoId, MarcacaoId, PastaId, VolumeId};
pub use tipos::{
    Autor, EstadoExecucao, EstadoMarcacao, EtapaVarredura, Finalidade, ModoVarredura,
    OrigemMarcacao, TipoMarcacao,
};
