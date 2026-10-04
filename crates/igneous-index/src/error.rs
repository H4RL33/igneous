#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("the index database couldn't be rebuilt")]
    Unusable,
}
