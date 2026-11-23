use std::time::Duration;

use dag_scanner::{ConfigScan, Exclusao};

/// Parâmetros do monitor (A-21: padrões conservadores).
#[derive(Debug, Clone)]
pub struct ConfigMonitor {
    /// Teto de watches deste monitor. `None` = metade de `fs.inotify.max_user_watches`
    /// (o resto fica para outros programas do usuário).
    pub max_watches: Option<usize>,
    /// Silêncio exigido num diretório antes de reconciliá-lo (coalescência).
    pub debounce: Duration,
    /// Espera máxima de um diretório sob escrita contínua.
    pub debounce_maximo: Duration,
    /// Teto de diretórios sujos na fila. Acima dele não se acumula mais detalhe: a raiz é
    /// marcada como lacuna e reconciliada por Fast Scan (backpressure).
    pub max_dirs_pendentes: usize,
    /// Diretórios por lote de reconciliação.
    pub lote_max: usize,
    /// Fast Scan periódico de todas as raízes (rede de segurança). `None` desliga.
    pub reconciliar_a_cada: Option<Duration>,
    /// Fast Scan das raízes ao iniciar (pega o que mudou com o monitor parado).
    pub reconciliar_ao_iniciar: bool,
    /// Arquivo modificado há menos que isto ainda está sendo gravado: reconcilia de novo e só
    /// depois faz hash.
    pub estabilidade: Duration,
    /// Intervalo mínimo para gravar a saúde em `monitor_saude`.
    pub heartbeat: Duration,
    /// Espera antes de tentar de novo uma raiz indisponível.
    pub nova_tentativa: Duration,
    pub exclusoes: Vec<Exclusao>,
    /// Configuração dos Fast Scans disparados pelo monitor.
    pub scan: ConfigScan,
}

impl Default for ConfigMonitor {
    fn default() -> Self {
        Self {
            max_watches: None,
            debounce: Duration::from_millis(750),
            debounce_maximo: Duration::from_secs(5),
            max_dirs_pendentes: 50_000,
            lote_max: 2_000,
            reconciliar_a_cada: Some(Duration::from_secs(30 * 60)),
            reconciliar_ao_iniciar: true,
            estabilidade: Duration::from_secs(2),
            heartbeat: Duration::from_secs(1),
            nova_tentativa: Duration::from_secs(10),
            exclusoes: Vec::new(),
            scan: ConfigScan::default(),
        }
    }
}
