//! Catálogo SQLite do DAG: abertura, migrações versionadas, cópia segura e verificação de
//! integridade. A mecânica de SQLite vem de `bancada-armazem`; este crate só define o esquema
//! (veja `ESQUEMA.md`) e o tipo [`Catalogo`].
//!
//! O worker é o único escritor; a GUI e a CLI leem pela conexão de leitura (WAL).

pub mod confianca;
pub mod configuracao;
pub mod consulta_gui;
pub mod duplicados;
pub mod hashes;
pub mod inventario;
pub mod reconciliacao;

use std::path::Path;
use std::time::Duration;

pub use bancada_armazem::{Armazem, Erro, Resultado};
use bancada_armazem::{Escada, Migracao, Pragmas, RelatorioMigracao};

/// Degraus do esquema. Migração já publicada nunca é editada: só se acrescenta uma nova.
pub const MIGRACOES: &[Migracao] = &[
    Migracao::sql(
        "esquema inicial do Catálogo (Fase A rev. 2)",
        include_str!("../migrations/0001_inicial.sql"),
    ),
    Migracao::sql(
        "inventário do Full Scan: tipo de entrada, totais por volume, índices",
        include_str!("../migrations/0002_inventario.sql"),
    ),
    Migracao::sql(
        "Fast Scan, monitor de eventos e volumes offline: first_run_id, eventos e saúde",
        include_str!("../migrations/0003_reconciliacao.sql"),
    ),
    Migracao::sql(
        "retomada automática de execuções interrompidas e Ponto de Confiança por Raiz",
        include_str!("../migrations/0004_retomada_e_confianca.sql"),
    ),
    Migracao::sql(
        "configurações do usuário (chave/valor), p. ex. paralelismo do Hash Completo",
        include_str!("../migrations/0005_configuracoes.sql"),
    ),
    Migracao::sql(
        "fila de hash com volume_id denormalizado e índice por volume",
        include_str!("../migrations/0006_fila_hash_volume.sql"),
    ),
    Migracao::sql(
        "contador mantido de arquivos com hash (evita COUNT(*) em dezenas de milhões de linhas)",
        include_str!("../migrations/0007_contador_hashes.sql"),
    ),
];

/// Versão do esquema desta build (`PRAGMA user_version` esperado).
#[must_use]
pub const fn versao_do_esquema() -> u32 {
    MIGRACOES.len() as u32
}

fn pragmas() -> Pragmas {
    Pragmas {
        espera_ocupado: Duration::from_secs(30),
        ..Pragmas::default()
    }
}

/// O Catálogo aberto e migrado até a última versão.
#[derive(Debug)]
pub struct Catalogo {
    armazem: Armazem,
}

impl Catalogo {
    /// Abre (criando, se preciso) o Catálogo em `caminho` e aplica as migrações pendentes.
    ///
    /// # Errors
    /// Falha do SQLite, ou [`Erro::BancoMaisNovo`] se o arquivo vem de uma versão futura.
    pub fn abrir(caminho: impl AsRef<Path>) -> Resultado<Self> {
        Self::migrado(Armazem::abrir(caminho, &pragmas())?)
    }

    /// Catálogo em memória (testes).
    ///
    /// # Errors
    /// Falha do SQLite.
    pub fn em_memoria() -> Resultado<Self> {
        Self::migrado(Armazem::em_memoria(&pragmas())?)
    }

    fn migrado(mut armazem: Armazem) -> Resultado<Self> {
        let _: RelatorioMigracao = armazem.migrar(&Escada::nova(MIGRACOES))?;
        Ok(Self { armazem })
    }

    /// `PRAGMA user_version` do banco.
    ///
    /// # Errors
    /// Falha do SQLite.
    pub fn versao(&self) -> Resultado<u32> {
        self.armazem.versao()
    }

    /// Copia o Catálogo para `destino` pela API de backup do SQLite (consistente em WAL).
    /// Nunca sobrescreve.
    ///
    /// # Errors
    /// [`Erro::DestinoJaExiste`], SQLite ou E/S.
    pub fn copiar_para(&self, destino: impl AsRef<Path>) -> Resultado<()> {
        self.armazem.copiar_para(destino)
    }

    /// `PRAGMA integrity_check`.
    ///
    /// # Errors
    /// [`Erro::IntegridadeComFalhas`] ou falha do SQLite.
    pub fn verificar_integridade(&self) -> Resultado<()> {
        self.armazem.verificar_integridade()
    }

    /// O armazém subjacente (conexões, transações curtas).
    #[must_use]
    pub fn armazem(&self) -> &Armazem {
        &self.armazem
    }

    /// O armazém subjacente, mutável (para `transacao_imediata`).
    pub fn armazem_mut(&mut self) -> &mut Armazem {
        &mut self.armazem
    }
}
