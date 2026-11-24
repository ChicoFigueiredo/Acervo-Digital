use crate::disco::Disco;
use crate::erro::{Result, sem_suporte};

pub(crate) fn abrir(_d: Box<dyn Disco>) -> Result<Box<dyn Disco>> {
    sem_suporte("ainda não implementado")
}
