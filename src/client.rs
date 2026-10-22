use std::{fmt, io::Read, time::Duration};

use flate2::read::MultiGzDecoder;
use reqwest::{
    StatusCode, Url,
    header::{self, HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Catalog, DEFAULT_MAX_BYTES, ParseError, ParseLimits};

const DEFAULT_ENDPOINT: &str = "https://models.dev/api.json";
const MAX_VALIDATOR_BYTES: usize = 1024;

/// Validators belong to one exact endpoint. Persist this alongside your last-known-good catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub source: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub struct Download {
    pub catalog: Catalog,
    /// Validated, decompressed original JSON, suitable for consumer-managed persistence.
    pub body: Vec<u8>,
    pub metadata: CacheMetadata,
}

impl fmt::Debug for Download {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Download")
            .field("providers", &self.catalog.providers.len())
            .field("body_bytes", &self.body.len())
            .field("metadata", &self.metadata)
            .finish()
    }
}

#[derive(Debug)]
pub enum Refresh {
    Modified(Download),
    NotModified(CacheMetadata),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransportKind {
    Timeout,
    Connect,
    Body,
    Other,
}

/// Network failures intentionally do not retain reqwest's URL-bearing error or response data.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClientError {
    #[error("invalid client configuration")]
    InvalidConfiguration,
    #[error("endpoint must be HTTPS without credentials, query or fragment")]
    InvalidEndpoint,
    #[error("could not initialize HTTP transport")]
    Build,
    #[error("catalog transport failed ({kind:?})")]
    Transport { kind: TransportKind },
    #[error("unexpected HTTP status {status}")]
    UnexpectedStatus { status: u16 },
    #[error("304 response without a conditional request")]
    NotModifiedWithoutCache,
    #[error("cache validators belong to a different endpoint")]
    ValidatorSourceMismatch,
    #[error("invalid cache validator")]
    InvalidValidator,
    #[error("download exceeds the byte limit of {limit}")]
    DownloadTooLarge { limit: usize },
    #[error("decoded body exceeds the byte limit of {limit}")]
    DecodedTooLarge { limit: usize },
    #[error("unsupported content encoding")]
    UnsupportedEncoding,
    #[error("invalid gzip body")]
    InvalidCompression,
    #[error(transparent)]
    Parse(#[from] ParseError),
}

impl ClientError {
    fn transport(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            TransportKind::Timeout
        } else if error.is_connect() {
            TransportKind::Connect
        } else if error.is_body() {
            TransportKind::Body
        } else {
            TransportKind::Other
        };
        Self::Transport { kind }
    }
}

#[derive(Clone)]
pub struct Client {
    transport: reqwest::Client,
    endpoint: Url,
    max_download_bytes: usize,
    parse_limits: ParseLimits,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("max_download_bytes", &self.max_download_bytes)
            .field("parse_limits", &self.parse_limits)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct ClientBuilder {
    endpoint: String,
    timeout: Duration,
    connect_timeout: Duration,
    max_download_bytes: usize,
    parse_limits: ParseLimits,
    #[cfg(test)]
    allow_http: bool,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.into(),
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            max_download_bytes: DEFAULT_MAX_BYTES,
            parse_limits: ParseLimits::default(),
            #[cfg(test)]
            allow_http: false,
        }
    }
}

impl ClientBuilder {
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Overall request deadline, including receiving the body.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The wire-byte limit, before any gzip decoding.
    pub fn max_download_bytes(mut self, limit: usize) -> Self {
        self.max_download_bytes = limit;
        self
    }

    /// `max_bytes` bounds the decoded document as well as the parser.
    pub fn parse_limits(mut self, limits: ParseLimits) -> Self {
        self.parse_limits = limits;
        self
    }

    pub fn build(self) -> Result<Client, ClientError> {
        self.parse_limits
            .check()
            .map_err(|_| ClientError::InvalidConfiguration)?;
        if self.timeout.is_zero()
            || self.connect_timeout.is_zero()
            || self.max_download_bytes == 0
            || self.parse_limits.max_bytes.checked_add(1).is_none()
        {
            return Err(ClientError::InvalidConfiguration);
        }
        #[cfg(test)]
        let allow_http = self.allow_http;
        #[cfg(not(test))]
        let allow_http = false;
        let endpoint = Url::parse(&self.endpoint).map_err(|_| ClientError::InvalidEndpoint)?;
        if (endpoint.scheme() != "https" && !(allow_http && endpoint.scheme() == "http"))
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ClientError::InvalidEndpoint);
        }
        let transport = reqwest::Client::builder()
            .https_only(!allow_http)
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(self.timeout)
            .connect_timeout(self.connect_timeout)
            .user_agent(concat!("models-dev-rs/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| ClientError::Build)?;
        Ok(Client {
            transport,
            endpoint,
            max_download_bytes: self.max_download_bytes,
            parse_limits: self.parse_limits,
        })
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    pub fn new() -> Result<Self, ClientError> {
        Self::builder().build()
    }

    pub async fn fetch(&self) -> Result<Download, ClientError> {
        match self.refresh(&CacheMetadata::default()).await? {
            Refresh::Modified(download) => Ok(download),
            Refresh::NotModified(_) => Err(ClientError::NotModifiedWithoutCache),
        }
    }

    /// Never mutates a previous catalog. The caller decides when to activate a modified result.
    pub async fn refresh(&self, previous: &CacheMetadata) -> Result<Refresh, ClientError> {
        let conditional = previous.etag.is_some() || previous.last_modified.is_some();
        if (conditional || previous.source.is_some())
            && previous.source.as_deref() != Some(self.endpoint.as_str())
        {
            return Err(ClientError::ValidatorSourceMismatch);
        }
        validate_etag(previous.etag.as_deref())?;
        validate_last_modified(previous.last_modified.as_deref())?;
        let mut request = self
            .transport
            .get(self.endpoint.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::ACCEPT_ENCODING, "gzip");
        if let Some(etag) = &previous.etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &previous.last_modified {
            request = request.header(header::IF_MODIFIED_SINCE, modified);
        }
        let mut response = request.send().await.map_err(ClientError::transport)?;
        let status = response.status();
        if status != StatusCode::OK && status != StatusCode::NOT_MODIFIED {
            return Err(ClientError::UnexpectedStatus {
                status: status.as_u16(),
            });
        }
        if status == StatusCode::NOT_MODIFIED && !conditional {
            return Err(ClientError::NotModifiedWithoutCache);
        }
        let mut metadata = CacheMetadata {
            source: Some(self.endpoint.as_str().to_owned()),
            etag: validator(response.headers(), header::ETAG)?,
            last_modified: validator(response.headers(), header::LAST_MODIFIED)?,
        };
        validate_etag(metadata.etag.as_deref())?;
        validate_last_modified(metadata.last_modified.as_deref())?;
        if status == StatusCode::NOT_MODIFIED {
            metadata.etag = metadata.etag.or_else(|| previous.etag.clone());
            metadata.last_modified = metadata
                .last_modified
                .or_else(|| previous.last_modified.clone());
            return Ok(Refresh::NotModified(metadata));
        }
        let gzip = encoding(response.headers())?;
        if response
            .content_length()
            .is_some_and(|size| size > self.max_download_bytes as u64)
        {
            return Err(ClientError::DownloadTooLarge {
                limit: self.max_download_bytes,
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ClientError::transport)? {
            if chunk.len() > self.max_download_bytes.saturating_sub(body.len()) {
                return Err(ClientError::DownloadTooLarge {
                    limit: self.max_download_bytes,
                });
            }
            body.extend_from_slice(&chunk);
        }
        let decoded_limit = self.parse_limits.max_bytes;
        if gzip {
            let mut decoded = Vec::new();
            MultiGzDecoder::new(body.as_slice())
                .take(decoded_limit as u64 + 1)
                .read_to_end(&mut decoded)
                .map_err(|_| ClientError::InvalidCompression)?;
            body = decoded;
        }
        if body.len() > decoded_limit {
            return Err(ClientError::DecodedTooLarge {
                limit: decoded_limit,
            });
        }
        let catalog = Catalog::from_slice_with_limits(&body, &self.parse_limits)?;
        Ok(Refresh::Modified(Download {
            catalog,
            body,
            metadata,
        }))
    }
}

fn validator(headers: &HeaderMap, name: HeaderName) -> Result<Option<String>, ClientError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() || value.as_bytes().len() > MAX_VALIDATOR_BYTES {
        return Err(ClientError::InvalidValidator);
    }
    let text = value.to_str().map_err(|_| ClientError::InvalidValidator)?;
    Ok(Some(text.to_owned()))
}

fn validate_etag(value: Option<&str>) -> Result<(), ClientError> {
    let Some(value) = value else { return Ok(()) };
    let tag = value.strip_prefix("W/").unwrap_or(value);
    if value.len() > MAX_VALIDATOR_BYTES
        || tag.len() < 2
        || !tag.starts_with('"')
        || !tag.ends_with('"')
        || !tag.as_bytes()[1..tag.len() - 1]
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte))
        || HeaderValue::from_str(value).is_err()
    {
        return Err(ClientError::InvalidValidator);
    }
    Ok(())
}

fn validate_last_modified(value: Option<&str>) -> Result<(), ClientError> {
    let Some(value) = value else { return Ok(()) };
    if value.len() > MAX_VALIDATOR_BYTES
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || !byte.is_ascii())
        || httpdate::parse_http_date(value).is_err()
    {
        return Err(ClientError::InvalidValidator);
    }
    Ok(())
}

fn encoding(headers: &HeaderMap) -> Result<bool, ClientError> {
    let mut values = headers.get_all(header::CONTENT_ENCODING).iter();
    let Some(value) = values.next() else {
        return Ok(false);
    };
    if values.next().is_some() {
        return Err(ClientError::UnsupportedEncoding);
    }
    let text = value
        .to_str()
        .map_err(|_| ClientError::UnsupportedEncoding)?;
    if text.eq_ignore_ascii_case("gzip") {
        Ok(true)
    } else if text.eq_ignore_ascii_case("identity") {
        Ok(false)
    } else {
        Err(ClientError::UnsupportedEncoding)
    }
}

#[cfg(test)]
mod tests;
