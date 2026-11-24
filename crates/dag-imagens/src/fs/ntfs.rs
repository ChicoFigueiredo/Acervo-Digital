use super::Ctx;
use crate::disco::Vista;
use crate::erro::{Result, sem_suporte};

pub(crate) fn percorrer(_v: Vista, _ctx: &mut Ctx<'_>) -> Result<()> {
    sem_suporte("ainda não implementado")
}
