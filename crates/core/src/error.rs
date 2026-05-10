use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("missing environment variable: {0}")]
    MissingEnv(&'static str),

    #[error("script error: {0}")]
    Script(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn script(msg: impl Into<String>) -> Self {
        Self::Script(msg.into())
    }
}
