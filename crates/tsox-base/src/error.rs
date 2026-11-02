use thiserror::Error;

#[derive(Clone, Debug, Error)]
pub enum CheckerError {
    #[error("failed to parse source: {0}")]
    Parse(String),
}
