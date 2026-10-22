use thiserror::Error;

/// A sanitized parse failure. No variant contains data from the document.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ParseError {
    #[error("catalog exceeds the byte limit of {limit}")]
    TooLarge { limit: usize },
    #[error("invalid parser limits")]
    InvalidLimits,
    #[error("invalid JSON at line {line}, column {column}")]
    Json { line: usize, column: usize },
    #[error("catalog does not match the supported schema")]
    Schema,
    #[error("catalog validation failed: {0}")]
    Validation(#[from] ValidationError),
}

/// Validation categories deliberately omit offending keys and values.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    #[error("maximum nesting depth exceeded")]
    Depth,
    #[error("maximum node count exceeded")]
    Nodes,
    #[error("maximum collection size exceeded")]
    Collection,
    #[error("maximum string length exceeded")]
    StringLength,
    #[error("duplicate JSON key")]
    DuplicateKey,
    #[error("invalid or reserved object key")]
    ObjectKey,
    #[error("credential-bearing data is not accepted")]
    Credentials,
    #[error("invalid identifier")]
    Identifier,
    #[error("map key and identifier disagree")]
    IdentifierMismatch,
    #[error("provider count limit exceeded")]
    Providers,
    #[error("model count limit exceeded")]
    Models,
    #[error("invalid declarative URL")]
    Url,
    #[error("invalid environment variable name")]
    Environment,
    #[error("invalid interleaved capability")]
    Interleaved,
    #[error("invalid context pricing tier")]
    CostTier,
}
