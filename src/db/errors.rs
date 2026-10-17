use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[allow(clippy::enum_variant_names)]
pub enum Error {
    #[error("failed to open database at '{path}': {source}")]
    DatabaseOpen {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("could not find a valid OpenCode database; checked: {}", format_candidates(.candidates))]
    DatabaseNotFound { candidates: Vec<PathBuf> },

    #[error("database query failed: {source}")]
    DatabaseQuery {
        #[source]
        source: rusqlite::Error,
    },
}

fn format_candidates(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Error {
    pub fn database_open(path: impl Into<PathBuf>, source: rusqlite::Error) -> Self {
        Self::DatabaseOpen {
            path: path.into(),
            source,
        }
    }

    pub fn database_not_found(candidates: Vec<PathBuf>) -> Self {
        Self::DatabaseNotFound { candidates }
    }

    pub fn database_query(source: rusqlite::Error) -> Self {
        Self::DatabaseQuery { source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
