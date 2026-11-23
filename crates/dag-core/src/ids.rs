//! Identificadores tipados. Cada um embrulha um `i64` (rowid do Catálogo).

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_newtype {
    ($(#[$meta:meta])* $nome:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $nome(pub i64);

        impl fmt::Display for $nome {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $nome {
            fn from(v: i64) -> Self {
                Self(v)
            }
        }
    };
}

id_newtype!(
    /// Identifica um Volume.
    VolumeId
);
id_newtype!(
    /// Identifica um Dispositivo Físico (dois Volumes no mesmo dispositivo falham juntos).
    DispositivoId
);
id_newtype!(
    /// Identifica uma pasta.
    PastaId
);
id_newtype!(
    /// Identifica um arquivo (uma cópia física no Catálogo).
    ArquivoId
);
id_newtype!(
    /// Identifica uma execução de varredura.
    ExecucaoId
);
id_newtype!(
    /// Identifica um Grupo de Duplicados.
    GrupoId
);
id_newtype!(
    /// Identifica uma Marcação.
    MarcacaoId
);
