//! Tipos de dados de duplicados, Marcações, Cobertura e espaço recuperável (A-24..A-31).
//!
//! São o formato das respostas do worker; o cálculo vive em `dag-store` (consultas) e em
//! [`crate::regras`] (regras puras). Tempos em segundos Unix; tamanhos em bytes.

use crate::{
    ArquivoId, Autor, DispositivoId, EstadoMarcacao, GrupoId, HashCompleto, MarcacaoId, PastaId,
    TipoMarcacao, VolumeId,
};
use serde::{Deserialize, Serialize};

/// Texto das limitações da estimativa de espaço (A-31), repetido em todo relatório de espaço.
pub const LIMITACOES_ESPACO: [&str; 4] = [
    "O espaço recuperável é uma estimativa por tamanho lógico: o DAG não sabe se duas cópias compartilham blocos (reflink/COW, Btrfs, XFS, APFS) e, nesse caso, apagar uma delas pode liberar bem menos.",
    "Compressão e deduplicação do sistema de arquivos não são conhecidas: o ganho real pode diferir do tamanho lógico.",
    "Mesmo Arquivo Físico (hard links) conta como um só e nunca gera espaço recuperável.",
    "Cópias em Volumes desconectados entram na conta com o último estado conhecido; o espaço só é liberado de fato quando o Volume estiver conectado.",
];

/// Ordenação da lista de Grupos de Duplicados.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrdemDuplicados {
    /// Maior espaço recuperável primeiro (padrão).
    #[default]
    Recuperavel,
    /// Maior tamanho do arquivo primeiro.
    Tamanho,
}

/// Modo da Cópia Principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoPrincipal {
    /// Escolhida pela Prioridade de Volume.
    Auto,
    /// Escolhida pelo usuário; persiste entre reconstruções enquanto a cópia seguir no grupo.
    Manual,
}

/// Escopo de uma Marcação.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscopoMarcacao {
    /// Regra: vale para todos os arquivos do Volume.
    RegraVolume,
    /// Regra: vale para todos os arquivos sob a pasta.
    RegraPasta,
    /// Manual: um arquivo.
    ManualArquivo,
    /// Manual: criada a partir de um Grupo de Duplicados (uma linha por cópia marcada).
    ManualGrupo,
}

impl EscopoMarcacao {
    #[must_use]
    pub fn como_str(self) -> &'static str {
        match self {
            Self::RegraVolume => "regra_volume",
            Self::RegraPasta => "regra_pasta",
            Self::ManualArquivo => "manual_arquivo",
            Self::ManualGrupo => "manual_grupo",
        }
    }

    #[must_use]
    pub fn de_str(s: &str) -> Option<Self> {
        Some(match s {
            "regra_volume" => Self::RegraVolume,
            "regra_pasta" => Self::RegraPasta,
            "manual_arquivo" => Self::ManualArquivo,
            "manual_grupo" => Self::ManualGrupo,
            _ => return None,
        })
    }

    /// Regra (Volume ou pasta) e não marcação de um arquivo.
    #[must_use]
    pub fn eh_regra(self) -> bool {
        matches!(self, Self::RegraVolume | Self::RegraPasta)
    }
}

/// Marcação como guardada no Catálogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarcacaoInfo {
    pub id: MarcacaoId,
    pub tipo: TipoMarcacao,
    pub estado: EstadoMarcacao,
    pub escopo: EscopoMarcacao,
    pub volume: Option<VolumeId>,
    pub pasta: Option<PastaId>,
    pub arquivo: Option<ArquivoId>,
    pub grupo: Option<GrupoId>,
    /// Caminho do arquivo, da pasta ou rótulo do Volume.
    pub alvo: String,
    /// Só marcações manuais.
    pub hash_na_marcacao: Option<HashCompleto>,
    pub autor: Autor,
    pub criada_em: i64,
    pub atualizada_em: i64,
}

/// Marcação que vale para uma cópia (a própria ou a herdada de uma regra).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarcacaoDaCopia {
    pub id: MarcacaoId,
    pub tipo: TipoMarcacao,
    pub estado: EstadoMarcacao,
    pub escopo: EscopoMarcacao,
}

/// Cobertura de uma cópia (A-28).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoberturaArquivo {
    pub coberto: bool,
    /// `Some(t)`: a Cobertura vem só de cópias em Volumes desconectados; vale como
    /// "não verificada desde `t`" (último contato com o Volume).
    pub nao_verificada_desde: Option<i64>,
}

/// Uma cópia (arquivo físico) dentro de um Grupo de Duplicados.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiaGrupo {
    pub arquivo: ArquivoId,
    pub volume: VolumeId,
    pub volume_rotulo: Option<String>,
    pub dispositivo: Option<DispositivoId>,
    pub caminho: String,
    /// `false` = Volume desconectado (membro mantido com o último hash conhecido).
    pub conectado: bool,
    /// Último contato com o Volume, quando desconectado.
    pub visto_em: Option<i64>,
    pub principal: bool,
    /// Outros caminhos do Mesmo Arquivo Físico (hard links) que esta cópia representa.
    pub outros_caminhos: u32,
    pub marcacao: Option<MarcacaoDaCopia>,
    pub cobertura: CoberturaArquivo,
    /// Espaço recuperável atribuído a esta cópia (marcada para Deleção ativa ou redundante).
    pub recuperavel: ClasseRecuperavel,
}

/// Por que uma cópia conta (ou não) como espaço recuperável.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClasseRecuperavel {
    /// Não conta.
    Nao,
    /// Marcação para Deleção ativa.
    Marcada,
    /// Cópia redundante sem marcação (estimativa).
    Estimada,
}

/// Grupo de Duplicados resumido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoDuplicados {
    pub id: GrupoId,
    pub hash: HashCompleto,
    /// Tamanho lógico de cada cópia.
    pub tamanho: u64,
    /// Arquivos físicos distintos (hard links contam como um).
    pub copias: u32,
    /// Quantas das cópias estão em Volumes desconectados.
    pub copias_offline: u32,
    pub copia_principal: Option<ArquivoId>,
    pub modo_principal: ModoPrincipal,
    /// Bytes das cópias com Marcação para Deleção ativa.
    pub bytes_marcados_delecao: u64,
    /// Estimativa das cópias redundantes sem marcação (descontadas as com Marcação de Backup).
    pub bytes_redundantes_estimados: u64,
    /// Soma dos dois.
    pub espaco_recuperavel_bytes: u64,
}

/// Grupo com todas as suas cópias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetalheGrupo {
    pub grupo: GrupoDuplicados,
    pub copias: Vec<CopiaGrupo>,
}

/// Arquivo marcado para Deleção sem nenhuma outra cópia no Catálogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertaUnicaCopia {
    pub arquivo: ArquivoId,
    pub caminho: String,
}

/// Resultado de marcar (ou confirmar).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoMarcacao {
    /// Marcações criadas ou confirmadas por esta operação.
    pub marcacoes: Vec<MarcacaoInfo>,
    /// Cópias que já tinham a mesma Marcação e foram deixadas como estavam.
    pub ja_marcadas: u64,
    /// Total de arquivos em alerta de Única Cópia (só Marcação para Deleção).
    pub unicas_copias_total: u64,
    /// Amostra (até 20) dos arquivos em alerta de Única Cópia.
    pub unicas_copias: Vec<AlertaUnicaCopia>,
    /// Avisos para humanos (ex.: o alvo era um hard link; representante usado).
    pub avisos: Vec<String>,
}

/// Tipo de relatório.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoRelatorio {
    /// Resumo de Cobertura por Volume e por pasta.
    Cobertura,
    /// Arquivos sem Cobertura.
    SemCobertura,
    /// Espaço recuperável.
    Espaco,
    /// Marcações existentes.
    Marcacoes,
}

/// Nível de agregação do relatório de espaço recuperável.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NivelEspaco {
    /// Todo o Catálogo (este computador).
    Computador,
    #[default]
    Volume,
    Pasta,
    Grupo,
}

/// Linha do resumo de Cobertura por Volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoberturaVolume {
    pub volume: VolumeId,
    pub rotulo: Option<String>,
    pub conectado: bool,
    /// Arquivos regulares não vazios do Volume.
    pub arquivos: u64,
    pub com_cobertura: u64,
    /// Parte de `com_cobertura` apoiada só em Volumes desconectados.
    pub cobertura_nao_verificada: u64,
    pub sem_cobertura: u64,
    pub bytes_sem_cobertura: u64,
}

/// Linha do resumo de Cobertura por pasta (arquivos diretamente na pasta).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoberturaPasta {
    pub pasta: PastaId,
    pub volume: VolumeId,
    pub caminho: String,
    pub arquivos: u64,
    pub com_cobertura: u64,
    pub sem_cobertura: u64,
    pub bytes_sem_cobertura: u64,
}

/// Arquivo sem Cobertura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArquivoSemCobertura {
    pub arquivo: ArquivoId,
    pub volume: VolumeId,
    pub volume_rotulo: Option<String>,
    pub caminho: String,
    pub tamanho: u64,
    /// `None` = ainda sem Hash Completo.
    pub hash: Option<HashCompleto>,
    /// Hash conhecido e nenhuma outra cópia física no Catálogo.
    pub unica_copia: bool,
}

/// Linha do relatório de espaço recuperável.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EspacoItem {
    pub nivel: NivelEspaco,
    /// `VolumeId`, `PastaId` ou `GrupoId` conforme o nível; `None` para o computador.
    pub id: Option<i64>,
    pub nome: String,
    /// Só para Volume.
    pub conectado: Option<bool>,
    pub bytes_marcados_delecao: u64,
    pub bytes_redundantes_estimados: u64,
    pub bytes_total: u64,
    /// Cópias que contribuem.
    pub copias: u64,
}

/// Linha do relatório de Marcações.
pub type LinhaMarcacao = MarcacaoInfo;

/// Dados de um relatório (uma página).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "relatorio", rename_all = "snake_case")]
pub enum DadosRelatorio {
    Cobertura {
        volumes: Vec<CoberturaVolume>,
        /// Pastas com ao menos um arquivo sem Cobertura, paginadas.
        pastas: Vec<CoberturaPasta>,
    },
    SemCobertura {
        arquivos: Vec<ArquivoSemCobertura>,
    },
    Espaco {
        /// Total do Catálogo (este computador).
        total: EspacoItem,
        itens: Vec<EspacoItem>,
        limitacoes: Vec<String>,
    },
    Marcacoes {
        marcacoes: Vec<LinhaMarcacao>,
    },
}

/// Formato de exportação de relatório.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatoExportacao {
    Csv,
    Json,
}

/// Resumo da etapa de Hash Completo de uma varredura.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoHash {
    pub desfecho: String,
    pub hasheados: u64,
    pub reaproveitados: u64,
    pub bytes_lidos: u64,
    pub erros: u64,
    pub duracao_ms: u64,
}

/// Resumo da reconstrução de duplicados e da reavaliação de Marcações.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoDuplicados {
    pub grupos: u64,
    pub copias: u64,
    pub grupos_criados: u64,
    pub grupos_removidos: u64,
    pub marcacoes_pendentes: u64,
    /// Soma de bytes marcados e estimados.
    pub espaco_recuperavel_bytes: u64,
}

/// Resultado da Verificação de Integridade.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoVerificacao {
    pub desfecho: String,
    /// Arquivos relidos e conferidos (ok + divergentes).
    pub verificados: u64,
    pub divergentes: u64,
    /// Arquivos divergentes (conteúdo diferente do hash guardado; o hash e o arquivo não são alterados).
    pub arquivos_divergentes: Vec<ArquivoId>,
    /// Arquivos que mudaram legitimamente desde o hash e foram pulados.
    pub alterados: u64,
    pub bytes_lidos: u64,
    pub erros: u64,
    pub duracao_ms: u64,
}
