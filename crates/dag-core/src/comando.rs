//! Contrato entre clientes (GUI, CLI, MCP) e o worker, único escritor do Catálogo.
//!
//! Todo cliente envia um [`Pedido`] (autor + [`Comando`]) e recebe uma [`Resposta`].
//! O transporte (socket local, etc.) não faz parte deste crate; só o formato.

use crate::{
    ArquivoId, Autor, DispositivoId, EstadoExecucao, EtapaVarredura, ExecucaoId, Finalidade,
    GrupoId, HashCompleto, ModoVarredura, TipoDispositivo, VolumeId,
};
use serde::{Deserialize, Serialize};

/// Embrulho de um [`Comando`] com quem o originou (gravado nas Marcações).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pedido {
    pub autor: Autor,
    pub comando: Comando,
}

/// Alvo de uma Marcação: caminho absoluto ou arquivo já conhecido do Catálogo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", content = "valor", rename_all = "snake_case")]
pub enum Alvo {
    Caminho(String),
    Arquivo(ArquivoId),
}

/// Ação sobre o serviço em segundo plano (opt-in, A-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcaoServico {
    Instalar,
    Remover,
    Iniciar,
    Parar,
    /// Liga ou desliga a inicialização automática.
    Autostart {
        ativo: bool,
    },
}

/// Toda operação que o worker aceita.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "comando", rename_all = "snake_case")]
pub enum Comando {
    /// Lista Volumes conhecidos (conectados ou não).
    ListarVolumes,
    /// Inicia Full Scan ou Fast Scan sobre as Raízes (caminhos) dadas.
    IniciarScan {
        modo: ModoVarredura,
        raizes: Vec<String>,
    },
    /// Pausa a execução indicada (ou a única em andamento).
    Pausar { execucao: Option<ExecucaoId> },
    /// Retoma uma execução pausada.
    Retomar { execucao: Option<ExecucaoId> },
    /// Cancela uma execução.
    Cancelar { execucao: Option<ExecucaoId> },
    /// Consulta progresso das execuções.
    Status { execucao: Option<ExecucaoId> },
    /// Lista Grupos de Duplicados, paginado.
    ListarDuplicados {
        limite: Option<u32>,
        deslocamento: Option<u32>,
    },
    /// Cria Marcação de Backup.
    MarcarBackup { alvo: Alvo },
    /// Cria Marcação para Deleção (não apaga nem move nada).
    MarcarDelecao { alvo: Alvo },
    /// Remove a Marcação do alvo.
    Desmarcar { alvo: Alvo },
    /// Define a Prioridade de Volume: Volumes/pastas (caminhos) do mais ao menos prioritário.
    DefinirPrioridadeVolume { ordem: Vec<String> },
    /// Gera relatório de espaço, duplicados e Cobertura.
    Relatorio,
    /// Verificação de Integridade sobre as Raízes (todas, se vazio).
    VerificarIntegridade { raizes: Vec<String> },
    /// Gerencia o serviço em segundo plano.
    Servico { acao: AcaoServico },
    /// Backup consistente do Catálogo (SQLite) para `destino`.
    BackupCatalogo { destino: String },
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
}

/// Totais do Catálogo (soma dos agregados por Volume da última varredura de cada um).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoCatalogo {
    pub volumes: u64,
    pub pastas: u64,
    pub arquivos: u64,
    pub bytes_logicos: u64,
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

/// Grupo de Duplicados resumido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrupoDuplicados {
    pub id: GrupoId,
    pub hash: HashCompleto,
    pub tamanho: u64,
    pub copias: Vec<ArquivoId>,
    pub copia_principal: Option<ArquivoId>,
    pub espaco_recuperavel_bytes: u64,
}

/// Relatório agregado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relatorio {
    pub arquivos: u64,
    pub bytes_logicos: u64,
    pub grupos_duplicados: u64,
    pub espaco_recuperavel_bytes: u64,
    pub arquivos_com_cobertura: u64,
    pub unicas_copias_marcadas_delecao: u64,
}

/// Resposta do worker a um [`Comando`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resposta", rename_all = "snake_case")]
pub enum Resposta {
    /// Comando executado, sem dados a devolver.
    Ok {
        mensagem: Option<String>,
    },
    Volumes {
        volumes: Vec<VolumeInfo>,
    },
    ExecucaoIniciada {
        execucao: ExecucaoId,
    },
    Status {
        execucoes: Vec<StatusExecucao>,
        catalogo: ResumoCatalogo,
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
    },
    Explorar {
        /// Caminho explorado; `None` = Raízes.
        pasta: Option<String>,
        entradas: Vec<EntradaExploracao>,
        proximo_cursor: Option<String>,
    },
    Duplicados {
        grupos: Vec<GrupoDuplicados>,
        total: u64,
    },
    Relatorio {
        relatorio: Relatorio,
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
            Comando::IniciarScan {
                modo: ModoVarredura::FastScan,
                raizes: vec!["/tmp".into()],
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
        };
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        assert_eq!(v["comando"], "iniciar_scan");
        assert_eq!(v["modo"], "full_scan");
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
            comando: Comando::Relatorio,
        };
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Pedido>(&j).unwrap(), p);
    }
}
