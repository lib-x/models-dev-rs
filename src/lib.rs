#![doc = include_str!("../README.md")]

mod catalog;
mod error;
mod schema;
mod validation;

#[cfg(feature = "client")]
mod client;

pub use catalog::{Capability, Catalog, ModelFilter, ModelRef};
pub use error::{ParseError, ValidationError};
pub use schema::{
    Capabilities, CatalogDate, ContextTier, Cost, CostTier, Extensions, Interleaved,
    InterleavedField, Limit, Modalities, Modality, Model, ModelType, Price, Provider,
    ProviderConfig, Status,
};
pub use validation::{DEFAULT_MAX_BYTES, ParseLimits};

#[cfg(feature = "client")]
pub use client::{
    CacheMetadata, Client, ClientBuilder, ClientError, Download, Refresh, TransportKind,
};
