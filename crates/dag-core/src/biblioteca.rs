//! Contrato da Curadoria Bibliográfica (Fase B) entre clientes (GUI, CLI, MCP) e o agente.
//!
//! Os comandos vivem em [`Comando`](crate::Comando) (variantes `Biblioteca*`, aditivas, com serde
//! estável); toda resposta bibliográfica é `Resposta::Biblioteca { dados }` com um
//! [`DadosBiblioteca`] etiquetado por `tipo`. Os nomes seguem `CONTEXT.md`: Biblioteca
//! Homologada (só leitura), A Classificar, Obra > Edição > Formato > Arquivo, Cobertura, Lixeira
//! do Volume. Nada aqui move, apaga ou altera arquivos catalogados: o descarte continua sendo o
//! fluxo da Fase A (Marcação para Deleção + Lixeira do Volume, ADR 0005) com dupla confirmação, e
//! IA ou MCP nunca o iniciam.

use serde::{Deserialize, Serialize};

/// Papel de uma biblioteca Calibre registrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelRegistro {
    Homologada,
    AClassificar,
}

/// Estado de um item da fila bibliográfica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoFilaInfo {
    Pendente,
    EmAndamento,
    Concluido,
    Falhou,
    Excecao,
}

/// Por que um livro entra na fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotivoFila {
    Novo,
    Alterado,
    Reprocessar,
}

/// Ação humana sobre uma sugestão.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcaoRevisao {
    Aceitar,
    Corrigir,
    Rejeitar,
}

/// Alvo de uma decisão: `arquivo` (id do Catálogo), `edicao` ou `obra`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "id", rename_all = "snake_case")]
pub enum AlvoRevisao {
    Arquivo(i64),
    Edicao(i64),
    Obra(i64),
}

/// Uma decisão do lote. `valor` só vale (e é obrigatório) em `corrigir`; listas (autores, tags)
/// vão separadas por `|`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDecisao {
    pub alvo: AlvoRevisao,
    /// `titulo`, `autores`, `tags`, `estante`... (os nomes de `dag_biblio::Campo`, snake_case).
    pub campo: String,
    pub acao: AcaoRevisao,
    #[serde(default)]
    pub valor: Option<String>,
}

/// Aprovação humana explícita de uma homologação (B-24). O MCP não pode fornecê-la.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AprovacaoHumana {
    /// Quem aprova (nome da pessoa).
    pub quem: String,
    /// Quando (segundos Unix); `None` = agora.
    #[serde(default)]
    pub quando: Option<i64>,
    #[serde(default)]
    pub motivo: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Configuração
// ---------------------------------------------------------------------------------------------

/// Modo da homologação: `calibredb` local ou via `calibre-server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModoHomologacao {
    Local,
    Servidor,
}

/// Política da trava de escrita local do Calibre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoliticaTravaCalibre {
    Estrita,
    ApenasServidor,
}

/// IA local (Ollama ou llama-server em loopback).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigIaBiblioteca {
    pub ativa: bool,
    pub endpoint: String,
    pub modelo_primario: String,
    pub modelo_secundario: Option<String>,
    pub usar_visao: bool,
    pub timeout_s: u64,
    /// Livros por lote: o primário roda para o lote todo e o segundo parecer só depois, para
    /// trocar o modelo na GPU uma vez por lote. 1 = um livro por vez (sem agrupar).
    pub lote: usize,
}

impl Default for ConfigIaBiblioteca {
    fn default() -> Self {
        Self {
            ativa: false,
            endpoint: "http://127.0.0.1:11434".into(),
            modelo_primario: "gemma3:12b".into(),
            modelo_secundario: None,
            usar_visao: true,
            timeout_s: 300,
            lote: 8,
        }
    }
}

/// OCR dos PDFs digitalizados.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigOcrBiblioteca {
    pub ativo: bool,
    pub idiomas: Vec<String>,
    pub dpi: u32,
    pub max_paginas: u32,
    pub paginas_iniciais: u32,
    /// Páginas em OCR ao mesmo tempo dentro de um livro; 0 = automático (metade dos núcleos,
    /// dividida pelas extrações simultâneas).
    pub paralelo: u32,
}

impl Default for ConfigOcrBiblioteca {
    fn default() -> Self {
        Self {
            ativo: true,
            idiomas: vec!["por".into(), "eng".into()],
            dpi: 300,
            max_paginas: 6,
            paginas_iniciais: 20,
            paralelo: 0,
        }
    }
}

/// Homologação controlada (B-24).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigHomologacao {
    pub modo: ModoHomologacao,
    /// `http://127.0.0.1:17790` (modo servidor).
    pub url: Option<String>,
    pub id_biblioteca: Option<String>,
    pub usuario: Option<String>,
    /// Legado: nome de uma variável de ambiente com a senha do servidor. Só vale se a senha não
    /// estiver no Cofre (que é o lugar certo: `homologacao.senha` em `BibliotecaConfigDefinir`).
    pub senha_env: Option<String>,
    /// Informativo (preenchido pelo agente na resposta): há senha do servidor no Cofre. A senha
    /// em si NUNCA volta por nenhuma resposta.
    #[serde(default)]
    pub senha_no_cofre: bool,
}

impl Default for ConfigHomologacao {
    fn default() -> Self {
        Self {
            modo: ModoHomologacao::Local,
            url: None,
            id_biblioteca: None,
            usuario: None,
            senha_env: None,
            senha_no_cofre: false,
        }
    }
}

/// Configuração da curadoria bibliográfica (persistida no Catálogo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigBiblioteca {
    /// Onde criar a A Classificar quando `BibliotecaRegistrar` aponta para um caminho que não é
    /// uma biblioteca (padrão sugerido para o computador do Chico).
    pub destino_a_classificar: String,
    pub ocr: ConfigOcrBiblioteca,
    pub ia: ConfigIaBiblioteca,
    /// Extrações simultâneas por Dispositivo Físico (padrão 2: o OCR é CPU, não disco).
    pub extracoes_por_dispositivo: u32,
    pub max_bytes_arquivo: u64,
    pub tentativas_max: u32,
    /// Prefixo do `calibredb` (`unshare`) para isolar o lock; desligado por padrão.
    pub usar_unshare: bool,
    pub politica_trava: PoliticaTravaCalibre,
    pub homologacao: ConfigHomologacao,
    /// Segundos de arquivo estável antes de enfileirar (debounce do watcher).
    pub debounce_s: u64,
    /// Extensões de download incompleto, ignoradas pelo watcher.
    pub extensoes_ignoradas: Vec<String>,
    /// Pontuação de aging por hora de espera (B-06), na mesma escala de
    /// `dag_biblio::prioridade` (0 a ~2): com 0,05 um arquivo que espera 20 h ganha 1 ponto,
    /// o que o põe à frente de qualquer chegada nova. Valores `<= 0` são tratados como 0,01
    /// (a fila nunca deixa um item morrer de fome).
    pub aging_por_hora: f64,
    /// Quantos Dispositivos Físicos extraem ao mesmo tempo (um arquivo por dispositivo).
    #[serde(default = "um_por_padrao_discos")]
    pub max_discos_extracao: u32,
    /// Opção (desligada): tag de controle (p. ex. `!Catalogar`) aplicada aos livros cadastrados
    /// na A Classificar. Nunca é aplicada na Homologada.
    #[serde(default)]
    pub tag_controle_livro_novo: Option<String>,
    /// Tamanho mínimo (bytes) para duplicata exata (ignora vazios e quebrados).
    pub tamanho_minimo_duplicata: u64,
    /// Trechos de caminho que identificam pastas de download / a classificar.
    pub pastas_a_classificar: Vec<String>,
    /// Versão das regras de extração/classificação: mudar reprocessa (CA-B11).
    pub versao_regras: String,
    /// O descarte só é sugerido se houve verificação de Cobertura aprovada nos últimos N dias
    /// (B-26). `0` desliga a exigência.
    #[serde(default = "validade_cobertura_padrao")]
    pub cobertura_validade_dias: u32,
    /// Percentual mínimo de Formatos com Cobertura para a verificação ser aprovada.
    #[serde(default = "cobertura_minima_padrao")]
    pub cobertura_minima_pct: u32,
}

fn validade_cobertura_padrao() -> u32 {
    30
}

fn cobertura_minima_padrao() -> u32 {
    100
}

fn um_por_padrao_discos() -> u32 {
    2
}

impl Default for ConfigBiblioteca {
    fn default() -> Self {
        Self {
            destino_a_classificar: "/mnt/e2/Livros_A_Classificar".into(),
            ocr: ConfigOcrBiblioteca::default(),
            ia: ConfigIaBiblioteca::default(),
            extracoes_por_dispositivo: 2,
            max_bytes_arquivo: 4 * 1024 * 1024 * 1024,
            tentativas_max: 3,
            usar_unshare: false,
            politica_trava: PoliticaTravaCalibre::Estrita,
            homologacao: ConfigHomologacao::default(),
            debounce_s: 30,
            extensoes_ignoradas: [
                ".part",
                ".crdownload",
                ".tmp",
                ".download",
                ".partial",
                ".opdownload",
            ]
            .map(String::from)
            .to_vec(),
            aging_por_hora: 0.05,
            max_discos_extracao: um_por_padrao_discos(),
            tag_controle_livro_novo: None,
            tamanho_minimo_duplicata: 1024,
            pastas_a_classificar: ["classific", "download", "_novos"]
                .map(String::from)
                .to_vec(),
            versao_regras: "b1".into(),
            cobertura_validade_dias: validade_cobertura_padrao(),
            cobertura_minima_pct: cobertura_minima_padrao(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Respostas
// ---------------------------------------------------------------------------------------------

/// Uma biblioteca Calibre registrada.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BibliotecaInfo {
    pub id: i64,
    pub caminho: String,
    pub papel: PapelRegistro,
    pub nome: String,
    pub estado: String,
    pub versao_calibre: Option<String>,
    pub library_uuid: Option<String>,
    /// Livros e formatos no espelho (Homologada) ou cadastrados (A Classificar).
    pub livros: i64,
    pub formatos: i64,
    pub ultima_sincronizacao: Option<i64>,
    pub ultimo_resultado: Option<String>,
}

/// Campo (coluna) do Calibre e se o DAG o clona.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampoInfo {
    pub rotulo: String,
    pub nome: String,
    pub datatype: String,
    pub multiplo: bool,
    pub clonavel: bool,
    pub motivo: Option<String>,
}

/// Resumo do perfil da Biblioteca Homologada (B-01).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerfilResumo {
    pub versao_esquema: Option<i64>,
    pub total_livros: u64,
    pub total_autores: u64,
    pub total_tags: u64,
    pub total_series: u64,
    pub total_editoras: u64,
    pub total_arquivos_formato: u64,
    pub formatos: Vec<(String, u64)>,
    /// Tags mais usadas.
    pub tags_principais: Vec<(String, u64)>,
    /// Candidatos a "estante" (nome, pontuação, razões).
    pub candidatos_estante: Vec<(String, u32, Vec<String>)>,
    /// Itens que o DAG não clona (nada se perde em silêncio).
    pub nao_clonaveis: Vec<String>,
    pub perfil_bruto: serde_json::Value,
}

/// Resultado de sincronizar a Homologada (somente leitura; provado por instantâneo).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumoSincronizacao {
    pub biblioteca: i64,
    pub livros: u64,
    pub removidos: u64,
    pub formatos: u64,
    pub formatos_ligados_ao_catalogo: u64,
    pub hashes_importados: u64,
    pub hashes_calculados: u64,
    pub formatos_sem_arquivo_no_catalogo: u64,
    pub formatos_sem_hash: u64,
    /// `primeiro`, `igual`, `alterado` ou `incompativel` (comparação do metadata.db).
    pub comparacao_metadata_db: String,
    /// O instantâneo antes e depois é idêntico: nada na biblioteca mudou (CA-B01).
    pub biblioteca_intacta: bool,
    pub diferencas: Vec<String>,
    pub duracao_ms: u64,
}

/// Papel de uma cópia fora da Homologada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PapelCopiaInfo {
    /// Espelho de biblioteca Calibre (é Cobertura, não candidata).
    EspelhoDeBiblioteca,
    /// Pasta de download ou "a classificar".
    DownloadOuAClassificar,
    Outro,
}

/// Decisão de `dag_biblio::descarte::avaliar`, só para exibir. Nunca executa nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisaoDescarteInfo {
    /// Pode ser SUGERIDA à Lixeira do Volume (com dupla confirmação, no fluxo da Fase A).
    pub sugerir: bool,
    pub copia_preservada: Option<i64>,
    /// Razões do bloqueio (vazio quando sugere).
    pub razoes: Vec<String>,
    pub passos_obrigatorios: Vec<String>,
}

/// Uma cópia, fora da Homologada, de um Formato homologado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiaDuplicata {
    pub file_id: i64,
    pub caminho: String,
    pub volume: String,
    pub volume_id: i64,
    pub dispositivo_id: Option<i64>,
    pub tamanho: u64,
    pub formato: String,
    pub hash: String,
    pub papel: PapelCopiaInfo,
    pub coberta: bool,
    pub decisao: DecisaoDescarteInfo,
}

/// Formato homologado que a cópia repete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoHomologadoRef {
    pub formato_id: i64,
    pub formato: String,
    pub caminho: String,
    pub tamanho: u64,
    pub hash: String,
    pub file_id: Option<i64>,
}

/// Livro homologado (referência para exibir).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LivroRef {
    pub id: i64,
    pub calibre_id: i64,
    pub titulo: String,
    pub autores: Vec<String>,
    pub isbn13: Option<String>,
    pub editora: Option<String>,
    pub idioma: Option<String>,
    pub estante: Option<String>,
    pub tags: Vec<String>,
    pub caminho: String,
}

/// Duplicatas exatas de um livro homologado (WOW-01, WOW-02).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrupoDuplicata {
    pub livro: LivroRef,
    pub formatos_homologados: Vec<FormatoHomologadoRef>,
    pub copias: Vec<CopiaDuplicata>,
    /// Soma dos bytes das cópias (potencial; só vira economia depois do fluxo da Lixeira).
    pub bytes_totais: u64,
    /// Soma das cópias cuja decisão é `sugerir`.
    pub bytes_sugeridos: u64,
}

/// Resultado de enfileirar livros.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoEnfileiramento {
    pub examinados: u64,
    pub enfileirados: u64,
    pub ja_na_fila: u64,
    pub ignorados: u64,
    pub interrompido: bool,
}

/// Contagem da fila.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContagemFila {
    pub pendentes: i64,
    pub em_andamento: i64,
    pub concluidos: i64,
    pub falhos: i64,
    pub excecoes: i64,
}

/// Um item da fila.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFilaInfo {
    pub id: i64,
    pub file_id: i64,
    pub caminho: Option<String>,
    pub motivo: MotivoFila,
    pub prioridade: i64,
    pub estado: EstadoFilaInfo,
    pub etapa: Option<String>,
    pub tentativas: i64,
    pub erro: Option<String>,
    pub enfileirado_em: i64,
}

/// Andamento (e o texto que vai ao painel) do job Biblioteca.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressoBiblioteca {
    pub itens_feitos: u64,
    pub itens_total: u64,
    pub bytes: u64,
    pub paginas: u64,
    pub paginas_ocr: u64,
    pub falhas: u64,
    pub excecoes: u64,
    pub duplicatas_exatas: u64,
    pub espaco_potencial_bytes: u64,
    pub item_atual: Option<String>,
}

impl ProgressoBiblioteca {
    /// Linha para o painel de trabalhos (vai como `Progresso::Mensagem`).
    #[must_use]
    pub fn texto(&self) -> String {
        format!(
            "biblioteca: {}/{} itens, {} B, {} páginas ({} com OCR), {} falha(s), {} exceção(ões), {} duplicata(s) exata(s), {} B potenciais{}",
            self.itens_feitos,
            self.itens_total,
            self.bytes,
            self.paginas,
            self.paginas_ocr,
            self.falhas,
            self.excecoes,
            self.duplicatas_exatas,
            self.espaco_potencial_bytes,
            self.item_atual
                .as_deref()
                .map(|i| format!(" — {i}"))
                .unwrap_or_default()
        )
    }
}

/// Resultado de `BibliotecaProcessar`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumoProcessamento {
    pub processados: u64,
    pub duplicatas_exatas: u64,
    pub extraidos: u64,
    pub reaproveitados: u64,
    pub revisao_pendente: u64,
    pub excecoes: u64,
    pub falhas: u64,
    pub consultas_ia: u64,
    pub bytes: u64,
    pub paginas: u64,
    pub paginas_ocr: u64,
    pub espaco_potencial_bytes: u64,
    /// Parou por pausa/cancelamento antes de acabar.
    pub interrompido: bool,
    pub pendentes_restantes: i64,
    /// Itens que estavam `em_andamento` de um processo morto e foram retomados.
    pub orfaos_retomados: u64,
    /// Itens de Contêiner (um por livro dentro de compactado) levados à revisão.
    #[serde(default)]
    pub membros_de_conteiner: u64,
    /// Livros da Homologada (`!Catalogar` ou sem tag) com proposta de tags/estante.
    #[serde(default)]
    pub propostas_homologada: u64,
}

/// Valor de um campo, lado a lado (B-12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CampoLadoALado {
    pub campo: String,
    /// Metadado interno do arquivo.
    pub extraido: Option<String>,
    pub sugerido: Option<String>,
    pub sugerido_lista: Vec<String>,
    pub confianca: Option<f32>,
    /// `alta`, `media` ou `baixa`.
    pub faixa: Option<String>,
    pub origem: Option<String>,
    pub modelo: Option<String>,
    pub evidencias: Vec<String>,
    pub precisa_revisao: bool,
    pub motivos: Vec<String>,
    pub homologado: Option<String>,
    /// Estado da sugestão: pendente, aceita, corrigida, rejeitada.
    pub estado: String,
    pub sugestao_id: Option<i64>,
}

/// Livro homologado parecido com o item em revisão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParecidoHomologado {
    pub livro: LivroRef,
    /// `isbn`, `titulo`...
    pub motivo: String,
    pub relacao: Option<String>,
}

/// Um item pendente de revisão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRevisaoInfo {
    pub file_id: i64,
    pub caminho: Option<String>,
    pub tamanho: Option<u64>,
    pub hash: Option<String>,
    pub formato: Option<String>,
    pub pendentes: i64,
    pub campos: Vec<CampoLadoALado>,
    pub homologados_parecidos: Vec<ParecidoHomologado>,
    pub obra_id: Option<i64>,
    pub edicao_id: Option<i64>,
    pub avisos: Vec<String>,
    /// Item de um Contêiner: caminho interno do livro (o `file_id` é o do Contêiner). Um
    /// compactado com vários livros gera um item por livro. Para decidir, use
    /// `AlvoRevisao::Edicao(edicao_id)`.
    #[serde(default)]
    pub membro: Option<String>,
    /// Sugestão para um livro que já está na Homologada (`!Catalogar` ou sem tag): só proposta,
    /// nada é escrito na Homologada sem uma homologação aprovada.
    #[serde(default)]
    pub proposta_homologada: Option<LivroRef>,
    /// Páginas extraídas, resumidas (amostra do conteúdo lido).
    #[serde(default)]
    pub paginas: Vec<PaginaResumo>,
    /// Quantas páginas precisaram de OCR (`> 0`: PDF de imagem ou parte dele).
    #[serde(default)]
    pub paginas_ocr: u32,
}

/// Página extraída, resumida para a revisão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaginaResumo {
    /// 1-based.
    pub numero: u32,
    pub via_ocr: bool,
    /// 0..=1.
    pub qualidade: f32,
    /// Começo do texto (até ~160 caracteres).
    pub trecho: String,
}

/// Estado de um cadastro na A Classificar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoCadastroInfo {
    Pendente,
    Cadastrando,
    Cadastrado,
    Falhou,
    Removido,
}

/// Um livro da A Classificar (ou pronto para ir a ela), para as telas de cadastro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CadastroInfo {
    /// `None` enquanto o livro ainda não tem cadastro (só está pronto para cadastrar).
    pub cadastro_id: Option<i64>,
    pub file_id: i64,
    pub titulo: Option<String>,
    pub calibre_id: Option<i64>,
    /// `None` = ainda sem cadastro.
    pub estado: Option<EstadoCadastroInfo>,
    /// Concluído na fila, sem revisão pendente e ainda não cadastrado.
    pub pronto_para_cadastrar: bool,
    pub erro: Option<String>,
}

/// Resultado do cadastro de um arquivo na A Classificar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoCadastro {
    /// Id do cadastro (para `BibliotecaHomologar`), quando já existe.
    #[serde(default)]
    pub cadastro_id: Option<i64>,
    pub file_id: i64,
    /// `cadastrado`, `ja_cadastrado`, `recusado`, `pendente` ou `falhou`.
    pub estado: String,
    pub calibre_id: Option<i64>,
    pub erro: Option<String>,
}

/// Uma homologação (proposta, aprovada ou aplicada).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomologacaoInfo {
    pub id: i64,
    pub cadastro_id: i64,
    /// `proposta`, `aprovada`, `aplicada`, `falhou`, `revertida`.
    pub estado: String,
    pub aprovado_por: Option<String>,
    pub aprovado_em: Option<i64>,
    pub calibre_id: Option<i64>,
    pub erro: Option<String>,
    /// O que vai entrar na Homologada (título, autores e formatos), para mostrar o plano antes
    /// da aprovação.
    #[serde(default)]
    pub resumo: Option<ResumoHomologacao>,
}

/// Resumo do livro de uma homologação.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoHomologacao {
    pub titulo: Option<String>,
    #[serde(default)]
    pub autores: Vec<String>,
    /// Formatos (EPUB, PDF...) que o livro tem na A Classificar.
    #[serde(default)]
    pub formatos: Vec<String>,
}

/// Um alerta bibliográfico.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertaInfo {
    pub id: i64,
    pub chave: Option<String>,
    /// `ja_tem_este_livro`, `duplicata_exata`, `extracao_falhou`...
    pub tipo: String,
    pub file_id: Option<i64>,
    pub dados: serde_json::Value,
    pub criado_em: i64,
    pub visto_em: Option<i64>,
    pub silenciado: bool,
}

/// Resumo diário (WOW-10).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumoBiblioteca {
    pub de: i64,
    pub ate: i64,
    pub alertas_por_tipo: Vec<(String, i64, i64)>,
    pub livros_novos_na_fila: i64,
    pub fila: ContagemFila,
    pub revisoes_pendentes: i64,
    pub candidatos_duplicidade_abertos: i64,
    pub bytes_recuperaveis_exatos: u64,
    pub alertas_nao_vistos: i64,
    /// Jobs concluídos no período.
    #[serde(default)]
    pub jobs_concluidos_periodo: i64,
    /// Itens que ganharam sugestões de classificação no período (arquivos distintos).
    #[serde(default)]
    pub classificados_periodo: i64,
    /// Decisões humanas (aceitar, corrigir, rejeitar) no período.
    #[serde(default)]
    pub decisoes_periodo: i64,
    /// Candidatos de duplicidade surgidos no período (exatas e bibliográficas).
    #[serde(default)]
    pub duplicatas_periodo: i64,
    #[serde(default)]
    pub duplicatas_exatas_periodo: i64,
}

/// Espaço por Volume (B-15, CA-B19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EspacoVolumeInfo {
    pub volume_id: i64,
    pub caminhos: u64,
    pub arquivos_fisicos: u64,
    pub bytes_logicos: u64,
    pub bytes_alocados: u64,
    pub estimativa_recuperavel: u64,
    pub economia_incerta: u64,
    pub efetivamente_recuperado: u64,
    pub caminhos_hardlink_ignorados: u64,
    pub membros_ignorados: u64,
}

/// Grupo de cópias idênticas com a mantida e as redundantes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoEspacoInfo {
    pub hash: String,
    pub tamanho_logico: u64,
    pub mantida: (i64, i64),
    pub redundantes: Vec<(i64, i64, u64, bool, Option<String>)>,
    pub bytes_estimados: u64,
    pub bytes_incertos: u64,
}

/// Relatório de espaço recuperável das duplicatas de livros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatorioEspacoInfo {
    pub volumes: Vec<EspacoVolumeInfo>,
    pub grupos: Vec<GrupoEspacoInfo>,
    /// Aviso fixo: estimativa, não promessa.
    pub observacao: String,
}

/// Formato da Homologada sem cópia com Cobertura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatoSemCoberturaInfo {
    pub formato_id: i64,
    pub caminho: String,
    pub motivo: String,
}

/// Verificação de Cobertura da Biblioteca Homologada (B-26/CA-B24): backup verificado = Cobertura
/// da Fase A (cópia com Marcação de Backup em outro Dispositivo Físico) mais a "restauração
/// amostral" feita como releitura integral e conferência de BLAKE3 (nada é copiado).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumoCobertura {
    pub biblioteca: i64,
    /// Linha gravada em `backups_biblioteca`.
    pub backup_id: i64,
    /// Segundos Unix da verificação.
    pub verificado_em: i64,
    pub formatos_total: u64,
    pub formatos_cobertos: u64,
    pub formatos_sem_cobertura: u64,
    /// Até 50 exemplos, com o motivo.
    pub sem_cobertura: Vec<FormatoSemCoberturaInfo>,
    /// Arquivos no manifesto (metadata.db, formatos, capas, OPF).
    pub arquivos_no_manifesto: u64,
    pub hash_metadata_db: String,
    pub amostra_pedida: u32,
    pub amostra_verificada: u32,
    pub amostra_ok: u32,
    pub amostra_falha: u32,
    pub falhas: Vec<String>,
    /// Cobertura mínima atingida e amostra sem falha (e com ao menos uma cópia verificada).
    pub aprovada: bool,
    /// Validade (dias) da verificação para o fluxo de descarte.
    pub validade_dias: u32,
}

/// Dados de toda resposta bibliográfica.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum DadosBiblioteca {
    Registrada {
        biblioteca: BibliotecaInfo,
        criada: bool,
        #[serde(default)]
        avisos: Vec<String>,
    },
    Bibliotecas {
        itens: Vec<BibliotecaInfo>,
    },
    Perfil {
        biblioteca: BibliotecaInfo,
        perfil: PerfilResumo,
        campos: Vec<CampoInfo>,
    },
    Sincronizada {
        resumo: ResumoSincronizacao,
    },
    Duplicatas {
        grupos: Vec<GrupoDuplicata>,
        bytes_totais_pagina: u64,
        bytes_sugeridos_pagina: u64,
        alertas_emitidos: u64,
        proximo_cursor: Option<String>,
    },
    Enfileirada {
        resumo: ResumoEnfileiramento,
    },
    Fila {
        contagem: ContagemFila,
        itens: Vec<ItemFilaInfo>,
        proximo_cursor: Option<String>,
    },
    Processada {
        resumo: ResumoProcessamento,
    },
    Revisao {
        itens: Vec<ItemRevisaoInfo>,
        total_pendentes: i64,
        proximo_cursor: Option<String>,
    },
    Decidida {
        lote: String,
        aplicadas: u64,
        ignoradas: Vec<String>,
    },
    Desfeita {
        lote: String,
        desfeitas: u64,
    },
    Cadastros {
        itens: Vec<CadastroInfo>,
        proximo_cursor: Option<String>,
    },
    Cadastrada {
        resultados: Vec<ResultadoCadastro>,
        /// Calibre/servidor ocupado: o que sobrou fica pendente (idempotente).
        bloqueado: Option<String>,
    },
    Homologacao {
        homologacao: HomologacaoInfo,
    },
    Alertas {
        itens: Vec<AlertaInfo>,
        nao_vistos_total: i64,
        proximo_cursor: Option<String>,
    },
    Vistos {
        marcados: u64,
    },
    Resumo {
        resumo: ResumoBiblioteca,
    },
    Espaco {
        relatorio: RelatorioEspacoInfo,
    },
    /// Resultado de `BibliotecaVerificarCobertura`.
    Cobertura {
        resumo: ResumoCobertura,
    },
    Config {
        config: ConfigBiblioteca,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_padrao_e_parcial_voltam() {
        let c = ConfigBiblioteca::default();
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<ConfigBiblioteca>(&j).unwrap(), c);
        let parcial: ConfigBiblioteca =
            serde_json::from_str(r#"{"ia":{"ativa":true},"usar_unshare":true}"#).unwrap();
        assert!(parcial.ia.ativa && parcial.usar_unshare);
        assert_eq!(parcial.ia.modelo_primario, "gemma3:12b");
        assert!(!c.usar_unshare && !c.ia.ativa);
    }

    #[test]
    fn dados_tem_etiqueta_estavel() {
        let d = DadosBiblioteca::Vistos { marcados: 2 };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["tipo"], "vistos");
        let a = AlvoRevisao::Arquivo(3);
        let v = serde_json::to_value(a).unwrap();
        assert_eq!(v["tipo"], "arquivo");
        assert_eq!(v["id"], 3);
    }

    #[test]
    fn progresso_vira_texto() {
        let p = ProgressoBiblioteca {
            itens_feitos: 2,
            itens_total: 5,
            ..Default::default()
        };
        assert!(p.texto().starts_with("biblioteca: 2/5 itens"));
    }
}
