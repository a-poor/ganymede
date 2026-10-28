//! Application-facing notebook documents, execution state, and controllers.
mod controller;
mod document;
mod state;

pub use controller::NotebookController;
pub use document::{Cell, CellId, CellKind, KnownOutput, Multiline, NotebookDocument, Output};
pub use state::{
    CellExecution, ConnectionState, ExecutionPhase, NotebookEvent, NotebookSnapshot, PendingInput,
    SaveState,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Jupyter(#[from] crate::jupyter::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid notebook: {0}")]
    Invalid(String),
    #[error("cell not found: {0}")]
    MissingCell(CellId),
    #[error("operation unavailable: {0}")]
    Unavailable(String),
}
pub type Result<T> = std::result::Result<T, Error>;
