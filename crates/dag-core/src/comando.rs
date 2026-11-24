//! Contrato entre clientes (GUI, CLI, MCP) e o worker, único escritor do Catálogo.
//!
//! Todo cliente envia um [`Pedido`] (autor + [`Comando`]) e recebe uma [`Resposta`].
//! O transporte (socket local, etc.) não faz parte deste crate; só o formato.

use crate::ClasseVolume;
use crate::{
    ArquivoId, Autor, DadosRelatorio, DetalheGrupo, DispositivoId, EstadoExecucao, EtapaVarredura,
    EventoInfo, ExecucaoId, Finalidade, FormatoExportacao, GrupoDuplicados, GrupoId, MarcacaoId,
    MarcacaoInfo, ModoVarredura, NivelEspaco, OrdemDuplicados, ResultadoMarcacao, ResumoDuplicados,
    ResumoHash, ResumoMonitorInfo, ResumoReconciliacao, ResumoVerificacao, SaudeMonitorInfo,
    TipoDispositivo, TipoRelatorio, VolumeId,
};
use serde::{Deserialize, Serialize};

/// Embrulho de um [`Comando`] com quem o originou (gravado nas Marcações).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pedido {
    pub autor: Autor,
    pub comando: Comando,
}

/// Alvo de uma Marcação (ou de uma escolha de Cópia Principal).
///
/// * `Caminho` / `Arquivo`: um arquivo. Se o caminho for de um hard link, vale o arquivo físico.
/// * `Grupo`: Marcação de Backup = todas as cópias do grupo; Marcação para Deleção = todas as
///   cópias **menos a Cópia Principal**.
/// * `Volume` / `Pasta`: regra; vale para todos os arquivos sob o Volume ou a pasta.
///   `Volume` aceita id numérico, rótulo, ponto de montagem ou `platform_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "valor", rename_all = "snake_case")]
pub enum Alvo {
    Caminho(String),
    Arquivo(ArquivoId),
    Grupo(GrupoId),
    Volume(String),
    Pasta(String),
}

/// Ação sobre o serviço em segundo plano (opt-in, A-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcaoServico {
    /// Só escreve a unidade e recarrega o systemd (não habilita nem inicia).
    Instalar,
    /// Instala (ou atualiza) a unidade, entrega o Catálogo ao serviço sem interromper abruptamente
    /// um agente já ativo (encerramento limpo: a execução fica `interrompida` e o serviço a
    /// retoma) e executa `enable --now`. `boot`: liga também o linger (sobe no boot).
    InstalarEAtivar {
        boot: bool,
    },
    Remover,
    Iniciar,
    Parar,
    /// Liga ou desliga a inicialização automática.
    Autostart {
        ativo: bool,
    },
    Reiniciar,
    /// Estado do serviço (instalado, autostart, ativo, PID, linger).
    Estado,
    /// Liga ou desliga o *linger* do usuário (`loginctl enable-linger`): o serviço de usuário
    /// passa a subir no boot e a sobreviver ao último logout. Ação explícita e separada.
    PersistirSemSessao {
        ativo: bool,
    },
}

/// Toda operação que o worker aceita.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "comando", rename_all = "snake_case")]
pub enum Comando {
    /// Lista Volumes conhecidos (conectados ou não).
    ListarVolumes,
    /// Inicia Full Scan ou Fast Scan sobre as Raízes (caminhos) dadas. No Fast Scan, sem
    /// Raízes, vale todas as Raízes já catalogadas cujo Volume está conectado.
    ///
    /// Ambos seguem com a etapa de Hash Completo (o Fast Scan só relê o que mudou), a
    /// reconstrução dos Grupos de Duplicados e a reavaliação das Marcações (que ficam
    /// Pendentes se o conteúdo mudou); `sem_hash` pula essas três etapas (inventário rápido).
    IniciarScan {
        modo: ModoVarredura,
        raizes: Vec<String>,
        #[serde(default)]
        sem_hash: bool,
        /// Lê pastas que negam acesso por um helper privilegiado (Polkit; opt-in, `--elevar`).
        #[serde(default)]
        elevar: bool,
        /// Só no Full Scan com hash, e só quando pedido: lê o conteúdo de **todos** os arquivos
        /// de novo, em vez de reaproveitar o hash dos que não mudaram (`--reler-tudo`).
        #[serde(default)]
        releitura_total: bool,
        /// Leitores simultâneos por disco no Hash Completo (1..=8); vence a flag do agente e o
        /// valor persistido.
        #[serde(default)]
        threads_por_disco: Option<u32>,
        /// Discos lendo ao mesmo tempo no Hash Completo (1..=16).
        #[serde(default)]
        max_discos: Option<u32>,
    },
    /// Pausa a execução indicada (ou a única em andamento).
    Pausar { execucao: Option<ExecucaoId> },
    /// Retoma uma execução pausada.
    ///
    /// `threads_por_disco`/`max_discos` valem só para a retomada de execução gravada no
    /// Catálogo (a pausada em memória continua com os valores com que começou).
    Retomar {
        execucao: Option<ExecucaoId>,
        #[serde(default)]
        threads_por_disco: Option<u32>,
        #[serde(default)]
        max_discos: Option<u32>,
    },
    /// Cancela uma execução.
    Cancelar { execucao: Option<ExecucaoId> },
    /// Lista os trabalhos do agente (em andamento e na fila), seja quem for que os iniciou.
    ListarJobs,
    /// Pausa o trabalho `job` (id do agente, como em `ListarJobs`).
    PausarJob { job: u64 },
    /// Retoma o trabalho pausado `job`.
    RetomarJob { job: u64 },
    /// Cancela o trabalho `job`, em andamento ou ainda na fila.
    CancelarJob { job: u64 },
    /// Pede ao agente em segundo plano que se reexecute no lugar (mesmo processo, binário atual
    /// do disco), com checkpoint do que estiver rodando. Só existe com agente.
    ReiniciarAgente,
    /// Consulta progresso das execuções.
    Status { execucao: Option<ExecucaoId> },
    /// Lista Grupos de Duplicados, paginado por cursor (keyset). `volume` filtra grupos com
    /// ao menos uma cópia no Volume (id, rótulo, ponto de montagem); `tamanho_minimo` em bytes;
    /// `ordem` padrão = maior espaço recuperável primeiro.
    ListarDuplicados {
        volume: Option<String>,
        tamanho_minimo: Option<u64>,
        ordem: Option<OrdemDuplicados>,
        limite: Option<u32>,
        cursor: Option<String>,
    },
    /// Reconstrói os Grupos de Duplicados a partir dos hashes atuais e reavalia as Marcações
    /// (trabalho longo em Catálogos grandes; acompanha-se pelo progresso "Agrupando duplicados").
    ReconstruirDuplicados,
    /// Um Grupo de Duplicados com todas as cópias, Marcações e Cobertura.
    DetalharGrupo { grupo: GrupoId },
    /// Troca manual da Cópia Principal do grupo; `arquivo: None` volta à escolha automática
    /// pela Prioridade de Volume. A escolha manual persiste entre reconstruções.
    DefinirCopiaPrincipal {
        grupo: GrupoId,
        arquivo: Option<Alvo>,
    },
    /// Cria Marcação de Backup (somente no Catálogo).
    MarcarBackup { alvo: Alvo },
    /// Cria Marcação para Deleção (não apaga nem move nada).
    MarcarDelecao { alvo: Alvo },
    /// Remove a Marcação do alvo (Marcação de arquivo, de grupo ou regra de Volume/pasta).
    Desmarcar { alvo: Alvo },
    /// Confirma de novo uma Marcação Pendente, adotando o Hash Completo atual do arquivo.
    ConfirmarMarcacao { marcacao: MarcacaoId },
    /// Define a Prioridade de Volume: Volumes ou pastas do mais ao menos prioritário. Cada item
    /// é id/rótulo/ponto de montagem de um Volume ou o caminho de uma pasta catalogada.
    DefinirPrioridadeVolume { ordem: Vec<String> },
    /// Relatório de Cobertura, arquivos sem Cobertura, espaço recuperável ou Marcações.
    /// Com `saida` (`.csv` ou `.json`), grava o relatório inteiro nesse caminho (sem
    /// sobrescrever arquivo existente); sem `saida`, devolve uma página (`limite`/`cursor`).
    /// `nivel` só vale para `Espaco`.
    Relatorio {
        tipo: TipoRelatorio,
        nivel: Option<NivelEspaco>,
        limite: Option<u32>,
        cursor: Option<String>,
        saida: Option<String>,
    },
    /// Monitora as Raízes (todas as catalogadas, se vazio) com inotify, em primeiro plano, até o
    /// cancelamento (Ctrl-C). `reconciliar_a_cada_s`: Fast Scan periódico (padrão 1800 s;
    /// 0 desliga). Os eventos são dicas: cada lote dispara reconciliação pelo escritor único.
    Monitorar {
        raizes: Vec<String>,
        #[serde(default)]
        reconciliar_a_cada_s: Option<u64>,
        /// Não faz Hash Completo das mudanças (só o inventário).
        #[serde(default)]
        sem_hash: bool,
    },
    /// Últimos eventos persistidos (criado, alterado, removido, renomeado, lacunas...).
    Eventos { limite: Option<u32> },
    /// Saúde do monitor e pendências de reconciliação.
    SaudeMonitor,
    /// Verificação de Integridade sobre as Raízes (todas, se vazio).
    VerificarIntegridade {
        raizes: Vec<String>,
        #[serde(default)]
        threads_por_disco: Option<u32>,
        #[serde(default)]
        max_discos: Option<u32>,
    },
    /// Paralelismo padrão do Hash Completo, persistido no Catálogo. Sem campos só consulta;
    /// com campos grava (1..=8 threads por disco, 1..=16 discos) para a próxima execução.
    ConfigHash {
        #[serde(default)]
        threads_por_disco: Option<u32>,
        #[serde(default)]
        max_discos: Option<u32>,
    },
    /// Gerencia o serviço em segundo plano.
    Servico { acao: AcaoServico },
    /// Backup consistente do Catálogo (SQLite) para `destino`.
    BackupCatalogo { destino: String },
    /// Move para a Lixeira do Volume (`<ponto de montagem>/DAG-Lixeira`) as cópias com Marcação
    /// para Deleção dos `alvos` (arquivo, caminho, pasta ou grupo), por `rename(2)` no mesmo
    /// Volume (ADR 0005). Nunca apaga nem sobrescreve.
    ///
    /// * Única Cópia só vai com `confirmacao_unica_copia`.
    /// * Pasta só vai se **todo** arquivo catalogado dentro dela tem Marcação para Deleção;
    ///   `marcar_tudo` é o passo explícito "marcar tudo e mover".
    /// * `simular`: só planeja (contagens, bytes, destino), sem tocar no disco nem no Catálogo.
    MoverParaLixeira {
        alvos: Vec<Alvo>,
        #[serde(default)]
        confirmacao_unica_copia: bool,
        #[serde(default)]
        marcar_tudo: bool,
        #[serde(default)]
        simular: bool,
    },
    /// Devolve Na Lixeira para o caminho de origem (nome novo com sufixo se estiver ocupado).
    RestaurarDaLixeira { alvos: Vec<Alvo> },
    /// Lista o que está Na Lixeira (e quantos registros Descartados existem).
    ListarLixeira {
        limite: Option<u32>,
        cursor: Option<String>,
    },
    /// Filhos diretos de `pasta` (ou as Raízes catalogadas, se `None`), do maior para o menor.
    /// `cursor` é o `proximo_cursor` opaco da página anterior.
    Explorar {
        pasta: Option<String>,
        limite: Option<u32>,
        cursor: Option<String>,
    },
}

/// Resumo de um Volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub id: VolumeId,
    /// `None` quando o sistema não informa o Dispositivo Físico (ex.: overlay, tmpfs).
    pub dispositivo: Option<DispositivoId>,
    pub tipo_dispositivo: Option<TipoDispositivo>,
    pub fs_type: Option<String>,
    pub rotulo: Option<String>,
    pub ponto_montagem: Option<String>,
    pub conectado: bool,
    pub capacidade_bytes: Option<u64>,
    pub livre_bytes: Option<u64>,
    /// Natureza do volume; volumes antigos (sem o campo) contam como físicos.
    #[serde(default)]
    pub classe: ClasseVolume,
    /// Partição de sistema (`/boot/efi`...) de um disco real.
    #[serde(default)]
    pub sistema: bool,
}

/// Progresso de uma execução de varredura.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusExecucao {
    pub id: ExecucaoId,
    pub modo: ModoVarredura,
    pub estado: EstadoExecucao,
    pub etapa: Option<EtapaVarredura>,
    pub itens_processados: u64,
    pub erros: u64,
    /// Estado exato gravado no Catálogo (`concluida_com_erros`, `interrompida`...).
    pub estado_detalhe: String,
    pub raizes: Vec<String>,
    /// Segundos Unix.
    pub iniciada_em: i64,
    pub encerrada_em: Option<i64>,
    pub pastas: u64,
    pub arquivos: u64,
    pub especiais: u64,
    pub bytes_logicos: u64,
    pub ignorados: u64,
    /// Andamento do Hash Completo, quando a execução está (ou parou) nessa etapa.
    #[serde(default)]
    pub hash: Option<StatusHash>,
    /// Flag "continuar de onde parou": execução interrompida (queda, falta de energia ou
    /// encerramento do agente) que a retomada automática vai continuar ou já está continuando.
    #[serde(default)]
    pub retomando: bool,
}

/// Situação do Ponto de Confiança de uma Raiz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusConfianca {
    pub raiz: String,
    pub volume: VolumeId,
    /// Segundos Unix; `Some` = só o monitoramento a mantém em dia.
    pub confiavel_em: Option<i64>,
    pub perdida_em: Option<i64>,
    pub motivo_perda: Option<String>,
}

/// Andamento da etapa de Hash Completo lido da fila persistente: contagens de leituras físicas
/// (um hard link com vários caminhos conta uma vez) e bytes, por disco e no total.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusHash {
    /// Há leitura de bytes recente (batimento de arquivo grande) ou arquivo concluído recente na
    /// fila; com o agente respondendo, vale o trabalho em memória. Falso: pausada, cancelada ou
    /// processo morto; `dag scan --retomar <id>` continua de onde parou.
    pub em_andamento: bool,
    pub arquivos_hasheados: u64,
    pub arquivos_pendentes: u64,
    pub arquivos_com_erro: u64,
    pub bytes_hasheados: u64,
    pub bytes_pendentes: u64,
    /// Vazão recente (média do último minuto), em bytes por segundo.
    pub bytes_por_seg: Option<u64>,
    pub discos: Vec<StatusDiscoHash>,
}

/// Andamento do hash em um Dispositivo Físico (volumes sem dispositivo conhecido aparecem um por
/// linha).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDiscoHash {
    pub disco: String,
    pub volumes: Vec<i64>,
    pub arquivos_hasheados: u64,
    pub arquivos_pendentes: u64,
    pub arquivos_com_erro: u64,
    pub bytes_hasheados: u64,
    pub bytes_pendentes: u64,
    /// Arquivos grandes em leitura agora (batimento gravado na fila ou, com o agente respondendo,
    /// o progresso em memória).
    #[serde(default)]
    pub lendo: Vec<crate::LeituraEmCurso>,
}

/// Totais do Catálogo (soma dos agregados por Volume da última varredura de cada um).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoCatalogo {
    pub volumes: u64,
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes_logicos: u64,
    /// Arquivos do Catálogo que já têm hash (contagem de `hashes`).
    #[serde(default)]
    pub arquivos_com_hash: u64,
}

/// Pasta ou arquivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoEntradaExploracao {
    Pasta,
    Arquivo,
}

/// Filho direto de uma pasta, para exploração.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntradaExploracao {
    pub tipo: TipoEntradaExploracao,
    pub nome: String,
    /// Pasta: soma recursiva; arquivo: tamanho lógico.
    pub bytes_logicos: u64,
    /// `None` = indisponível (ou pasta sem agregado).
    pub bytes_alocados: Option<u64>,
    /// Pasta: arquivos regulares na subárvore.
    pub arquivos: Option<u64>,
    /// Pasta: pastas na subárvore.
    pub pastas: Option<u64>,
    /// Arquivo: `arquivo`, `symlink`, `fifo`...
    pub kind: Option<String>,
    /// Arquivo: tipo MIME identificado (só por extensão neste incremento).
    pub mime: Option<String>,
    /// Arquivo: categoria do formato (`documento`, `imagem`...).
    pub categoria: Option<String>,
    pub finalidade: Option<Finalidade>,
    /// Pasta: `pendente`/`parcial`/`completo`/`desatualizado`.
    pub estado_agregado: Option<String>,
    /// Nanossegundos Unix.
    pub mtime_ns: Option<i64>,
}

/// Resposta do worker a um [`Comando`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resposta", rename_all = "snake_case")]
pub enum Resposta {
    /// Comando executado, sem dados a devolver.
    Ok {
        mensagem: Option<String>,
    },
    /// `volumes`: reais (físicos, removíveis, rede), registrados no Catálogo.
    /// `ocultos`: imagem/virtuais (snap, loop, zram...), fora da visão padrão e do Catálogo
    /// (a menos que tenham sido varridos explicitamente); `id` 0 = não registrado.
    Volumes {
        volumes: Vec<VolumeInfo>,
        #[serde(default)]
        ocultos: Vec<VolumeInfo>,
    },
    ExecucaoIniciada {
        execucao: ExecucaoId,
    },
    Status {
        execucoes: Vec<StatusExecucao>,
        catalogo: ResumoCatalogo,
        /// Saúde do monitor e reconciliação pendente.
        #[serde(default)]
        monitor: Option<SaudeMonitorInfo>,
        /// Ponto de Confiança por Raiz.
        #[serde(default)]
        confianca: Vec<StatusConfianca>,
    },
    /// Últimos eventos persistidos.
    Eventos {
        eventos: Vec<EventoInfo>,
    },
    SaudeMonitor {
        saude: SaudeMonitorInfo,
    },
    /// Monitoramento encerrado (Ctrl-C).
    MonitorEncerrado {
        resumo: ResumoMonitorInfo,
    },
    /// Varredura síncrona terminada (ou cancelada: retomável).
    ScanConcluido {
        execucao: ExecucaoId,
        /// `concluida`, `concluida_com_erros` ou `cancelada`.
        desfecho: String,
        pastas: u64,
        arquivos: u64,
        especiais: u64,
        bytes_logicos: u64,
        ignorados: u64,
        erros: u64,
        duracao_ms: u64,
        /// Etapa de Hash Completo (ausente com `sem_hash`).
        #[serde(default)]
        hash: Option<ResumoHash>,
        /// Reconstrução de duplicados e reavaliação de Marcações (ausente com `sem_hash`).
        #[serde(default)]
        duplicados: Option<ResumoDuplicados>,
        /// Só no Fast Scan: novos, alterados, reaproveitados, renomeados e removidos.
        #[serde(default)]
        reconciliacao: Option<ResumoReconciliacao>,
    },
    Explorar {
        /// Caminho explorado; `None` = Raízes.
        pasta: Option<String>,
        entradas: Vec<EntradaExploracao>,
        proximo_cursor: Option<String>,
    },
    Duplicados {
        grupos: Vec<GrupoDuplicados>,
        /// Grupos que casam com o filtro (todas as páginas).
        total: u64,
        proximo_cursor: Option<String>,
    },
    Grupo {
        detalhe: DetalheGrupo,
    },
    /// Marcação criada ou confirmada.
    Marcacao {
        resultado: ResultadoMarcacao,
    },
    /// Marcações removidas por `Desmarcar`.
    Desmarcado {
        removidas: Vec<MarcacaoInfo>,
    },
    /// Uma página de relatório.
    Relatorio {
        dados: DadosRelatorio,
        proximo_cursor: Option<String>,
    },
    /// Relatório gravado em `caminho`.
    RelatorioExportado {
        tipo: TipoRelatorio,
        caminho: String,
        formato: FormatoExportacao,
        linhas: u64,
    },
    Verificacao {
        resumo: ResumoVerificacao,
    },
    /// Trabalhos do agente (em andamento e na fila).
    Jobs {
        jobs: Vec<crate::JobInfo>,
    },
    /// Paralelismo do Hash Completo persistido (já com os padrões aplicados).
    ConfigHash {
        threads_por_disco: u32,
        max_discos: u32,
    },
    /// Resultado de mover ou restaurar (ou da simulação).
    Lixeira {
        resultado: crate::ResultadoLixeira,
    },
    /// O que está Na Lixeira.
    ListaLixeira {
        itens: Vec<crate::ItemLixeira>,
        /// Itens Na Lixeira (todas as páginas) e a soma dos bytes.
        total: u64,
        bytes: u64,
        /// Registros Descartados no histórico.
        descartados: u64,
        /// Últimos Descartados (amostra).
        #[serde(default)]
        ultimos_descartados: Vec<crate::ItemDescartado>,
        proximo_cursor: Option<String>,
    },
    /// Falha; `codigo` é estável para máquinas, `mensagem` é para humanos.
    Erro {
        codigo: String,
        mensagem: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ida_e_volta(c: &Comando) {
        let j = serde_json::to_string(c).unwrap();
        assert_eq!(&serde_json::from_str::<Comando>(&j).unwrap(), c);
    }

    #[test]
    fn comandos_serializam_e_voltam() {
        for c in [
            Comando::ListarVolumes,
            Comando::MoverParaLixeira {
                alvos: vec![Alvo::Pasta("/x".into()), Alvo::Arquivo(ArquivoId(3))],
                confirmacao_unica_copia: true,
                marcar_tudo: false,
                simular: true,
            },
            Comando::RestaurarDaLixeira {
                alvos: vec![Alvo::Arquivo(ArquivoId(3))],
            },
            Comando::ListarLixeira {
                limite: Some(5),
                cursor: None,
            },
            Comando::IniciarScan {
                modo: ModoVarredura::FastScan,
                raizes: vec!["/tmp".into()],
                sem_hash: true,
                elevar: false,
                releitura_total: false,
                threads_por_disco: Some(2),
                max_discos: None,
            },
            Comando::ConfigHash {
                threads_por_disco: Some(3),
                max_discos: Some(4),
            },
            Comando::Retomar {
                execucao: None,
                threads_por_disco: None,
                max_discos: Some(1),
            },
            Comando::ListarDuplicados {
                volume: Some("SSD".into()),
                tamanho_minimo: Some(10),
                ordem: Some(OrdemDuplicados::Tamanho),
                limite: Some(5),
                cursor: None,
            },
            Comando::DefinirCopiaPrincipal {
                grupo: GrupoId(2),
                arquivo: Some(Alvo::Arquivo(ArquivoId(4))),
            },
            Comando::MarcarDelecao {
                alvo: Alvo::Grupo(GrupoId(1)),
            },
            Comando::MarcarBackup {
                alvo: Alvo::Volume("HD-Externo".into()),
            },
            Comando::Desmarcar {
                alvo: Alvo::Pasta("/a/b".into()),
            },
            Comando::ConfirmarMarcacao {
                marcacao: MarcacaoId(7),
            },
            Comando::Relatorio {
                tipo: TipoRelatorio::Espaco,
                nivel: Some(NivelEspaco::Pasta),
                limite: None,
                cursor: None,
                saida: Some("/tmp/r.csv".into()),
            },
            Comando::Pausar { execucao: None },
            Comando::MarcarBackup {
                alvo: Alvo::Caminho("/a".into()),
            },
            Comando::Desmarcar {
                alvo: Alvo::Arquivo(ArquivoId(3)),
            },
            Comando::Servico {
                acao: AcaoServico::Autostart { ativo: true },
            },
            Comando::Servico {
                acao: AcaoServico::PersistirSemSessao { ativo: false },
            },
            Comando::Servico {
                acao: AcaoServico::Estado,
            },
            Comando::Servico {
                acao: AcaoServico::InstalarEAtivar { boot: true },
            },
            Comando::Servico {
                acao: AcaoServico::Reiniciar,
            },
            Comando::BackupCatalogo {
                destino: "/b".into(),
            },
            Comando::Explorar {
                pasta: Some("/a".into()),
                limite: Some(10),
                cursor: None,
            },
        ] {
            ida_e_volta(&c);
        }
    }

    #[test]
    fn formato_estavel() {
        let c = Comando::IniciarScan {
            modo: ModoVarredura::FullScan,
            raizes: vec!["/x".into()],
            sem_hash: false,
            elevar: false,
            releitura_total: false,
            threads_por_disco: None,
            max_discos: None,
        };
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        assert_eq!(v["comando"], "iniciar_scan");
        assert_eq!(v["modo"], "full_scan");
        // `sem_hash` é opcional no JSON (compatível com clientes antigos).
        let antigo: Comando = serde_json::from_str(
            r#"{"comando":"iniciar_scan","modo":"full_scan","raizes":["/x"]}"#,
        )
        .unwrap();
        assert_eq!(antigo, c);
        let m = Comando::MarcarBackup {
            alvo: Alvo::Volume("SSD".into()),
        };
        let v: serde_json::Value = serde_json::to_value(&m).unwrap();
        assert_eq!(v["comando"], "marcar_backup");
        assert_eq!(v["alvo"]["tipo"], "volume");
    }

    #[test]
    fn resposta_e_pedido_serializam() {
        let r = Resposta::Erro {
            codigo: "x".into(),
            mensagem: "y".into(),
        };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Resposta>(&j).unwrap(), r);
        let p = Pedido {
            autor: Autor::Mcp,
            comando: Comando::Relatorio {
                tipo: TipoRelatorio::Marcacoes,
                nivel: None,
                limite: None,
                cursor: None,
                saida: None,
            },
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Pedido>(&j).unwrap(), p);
    }
}
