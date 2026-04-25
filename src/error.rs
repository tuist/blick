use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlickError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Git(String),
    #[error("{0}")]
    Api(String),
    #[error("{0}")]
    MissingApiKey(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}
