//! Opaque public error boundary retaining the typed internal source chain.

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ApplicationError(#[from] crate::BlobrayError);

impl ApplicationError {
    pub(crate) fn into_inner(self) -> crate::BlobrayError {
        self.0
    }
}

pub type ApplicationResult<T> = std::result::Result<T, ApplicationError>;
