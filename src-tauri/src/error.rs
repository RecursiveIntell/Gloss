use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum GlossError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Provider error ({provider}): {source}")]
    Provider {
        provider: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Ingestion error for source {source_id}: {message}")]
    Ingestion { source_id: String, message: String },

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Search error: {0}")]
    Search(String),

    #[error("Studio error ({output_type}): {message}")]
    Studio {
        output_type: String,
        message: String,
    },

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("External tool not found: {tool}")]
    ExternalToolMissing { tool: String },

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

impl Serialize for GlossError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<anyhow::Error> for GlossError {
    fn from(err: anyhow::Error) -> Self {
        GlossError::Other(err.to_string())
    }
}
