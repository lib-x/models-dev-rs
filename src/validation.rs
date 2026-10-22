use std::{collections::BTreeSet, fmt};

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use url::Url;

use crate::{Catalog, Cost, Interleaved, ParseError, ValidationError};

pub const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Limits apply to known fields and unknown extensions alike. Custom limits are trusted policy.
#[derive(Clone, Debug)]
pub struct ParseLimits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_collection_items: usize,
    pub max_string_bytes: usize,
    pub max_number_bytes: usize,
    pub max_id_bytes: usize,
    pub max_providers: usize,
    pub max_models_per_provider: usize,
    pub max_models: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_depth: 32,
            max_nodes: 1_000_000,
            max_collection_items: 16_384,
            max_string_bytes: 65_536,
            max_number_bytes: 256,
            max_id_bytes: 512,
            max_providers: 4_096,
            max_models_per_provider: 8_192,
            max_models: 65_536,
        }
    }
}

impl ParseLimits {
    pub(crate) fn check(&self) -> Result<(), ParseError> {
        if self.max_bytes == 0
            || !(1..=64).contains(&self.max_depth)
            || self.max_nodes == 0
            || self.max_collection_items == 0
            || self.max_string_bytes == 0
            || self.max_number_bytes == 0
            || self.max_id_bytes == 0
            || self.max_providers == 0
            || self.max_models_per_provider == 0
            || self.max_models == 0
        {
            return Err(ParseError::InvalidLimits);
        }
        Ok(())
    }
}

struct Scan<'limits> {
    limits: &'limits ParseLimits,
    nodes: usize,
    failure: Option<ValidationError>,
}

impl Scan<'_> {
    fn fail<Error: de::Error>(&mut self, failure: ValidationError) -> Error {
        self.failure = Some(failure);
        Error::custom("catalog validation failed")
    }

    fn value(&mut self, raw: &RawValue, depth: usize) -> Result<(), serde_json::Error> {
        if depth > self.limits.max_depth {
            return Err(self.fail(ValidationError::Depth));
        }
        if self.nodes >= self.limits.max_nodes {
            return Err(self.fail(ValidationError::Nodes));
        }
        self.nodes += 1;
        let mut decoder = serde_json::Deserializer::from_str(raw.get());
        match raw.get().as_bytes().first() {
            Some(b'{') => decoder.deserialize_map(ObjectVisitor { scan: self, depth }),
            Some(b'[') => decoder.deserialize_seq(ArrayVisitor { scan: self, depth }),
            Some(b'"') => {
                let value: String = serde_json::from_str(raw.get())?;
                if value.len() > self.limits.max_string_bytes {
                    return Err(self.fail(ValidationError::StringLength));
                }
                if value.contains("://")
                    && let Ok(url) = Url::parse(&value)
                    && has_credentials(&url)
                {
                    return Err(self.fail(ValidationError::Credentials));
                }
                Ok(())
            }
            _ if raw.get().len() > self.limits.max_number_bytes => {
                Err(self.fail(ValidationError::StringLength))
            }
            _ => Ok(()),
        }
    }
}

struct ObjectVisitor<'scan, 'limits> {
    scan: &'scan mut Scan<'limits>,
    depth: usize,
}

impl<'de> Visitor<'de> for ObjectVisitor<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded object")
    }

    fn visit_map<Map: MapAccess<'de>>(self, mut map: Map) -> Result<(), Map::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if keys.len() >= self.scan.limits.max_collection_items {
                return Err(self.scan.fail(ValidationError::Collection));
            }
            if key.len() > self.scan.limits.max_string_bytes {
                return Err(self.scan.fail(ValidationError::StringLength));
            }
            if key.chars().any(char::is_control) || key.starts_with("$serde_json::") {
                return Err(self.scan.fail(ValidationError::ObjectKey));
            }
            if credential_key(&key) {
                return Err(self.scan.fail(ValidationError::Credentials));
            }
            if !keys.insert(key) {
                return Err(self.scan.fail(ValidationError::DuplicateKey));
            }
            let value = map.next_value::<&RawValue>()?;
            self.scan
                .value(value, self.depth + 1)
                .map_err(de::Error::custom)?;
        }
        Ok(())
    }
}

struct ArrayVisitor<'scan, 'limits> {
    scan: &'scan mut Scan<'limits>,
    depth: usize,
}

impl<'de> Visitor<'de> for ArrayVisitor<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded array")
    }

    fn visit_seq<Sequence: SeqAccess<'de>>(
        self,
        mut sequence: Sequence,
    ) -> Result<(), Sequence::Error> {
        let mut length = 0;
        while let Some(value) = sequence.next_element::<&RawValue>()? {
            if length >= self.scan.limits.max_collection_items {
                return Err(self.scan.fail(ValidationError::Collection));
            }
            length += 1;
            self.scan
                .value(value, self.depth + 1)
                .map_err(de::Error::custom)?;
        }
        Ok(())
    }
}

pub(crate) fn preflight(input: &[u8], limits: &ParseLimits) -> Result<(), ParseError> {
    limits.check()?;
    if input.len() > limits.max_bytes {
        return Err(ParseError::TooLarge {
            limit: limits.max_bytes,
        });
    }
    let raw: &RawValue = serde_json::from_slice(input).map_err(|error| ParseError::Json {
        line: error.line(),
        column: error.column(),
    })?;
    let mut scan = Scan {
        limits,
        nodes: 0,
        failure: None,
    };
    scan.value(raw, 0).map_err(|error| {
        scan.failure.map_or(
            ParseError::Json {
                line: error.line(),
                column: error.column(),
            },
            ParseError::Validation,
        )
    })
}

fn credential_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxyauthorization"
            | "cookie"
            | "setcookie"
            | "apikey"
            | "xapikey"
            | "password"
            | "passwd"
            | "secret"
            | "clientsecret"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "token"
            | "credentials"
    )
}

fn has_credentials(url: &Url) -> bool {
    !url.username().is_empty()
        || url.password().is_some()
        || url.query_pairs().any(|(key, _)| credential_key(&key))
}

fn declarative_url(value: &str, endpoint: bool) -> Result<(), ValidationError> {
    if value.chars().any(char::is_control) {
        return Err(ValidationError::Url);
    }
    let mut rendered = String::new();
    let mut remaining = value;
    while let Some((prefix, template)) = remaining.split_once("${") {
        if !endpoint {
            return Err(ValidationError::Url);
        }
        rendered.push_str(prefix);
        let (name, suffix) = template.split_once('}').ok_or(ValidationError::Url)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(ValidationError::Url);
        }
        rendered.push_str(if rendered.is_empty() {
            "https://example.invalid"
        } else {
            "placeholder"
        });
        remaining = suffix;
    }
    rendered.push_str(remaining);
    let url = Url::parse(&rendered).map_err(|_| ValidationError::Url)?;
    let allowed = matches!(url.scheme(), "https" | "http")
        || (endpoint && matches!(url.scheme(), "wss" | "ws"));
    if !allowed || url.host_str().is_none() {
        return Err(ValidationError::Url);
    }
    if has_credentials(&url) {
        return Err(ValidationError::Credentials);
    }
    Ok(())
}

fn identifier(value: &str, limits: &ParseLimits) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.len() > limits.max_id_bytes
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ValidationError::Identifier);
    }
    Ok(())
}

fn cost(value: &Cost) -> Result<(), ValidationError> {
    if let Some(legacy) = &value.context_over_200k {
        cost(legacy)?;
    }
    for tier in &value.tiers {
        if tier.tier.kind.is_empty() || tier.tier.kind.chars().any(char::is_control) {
            return Err(ValidationError::CostTier);
        }
        cost(&tier.cost)?;
    }
    Ok(())
}

pub(crate) fn catalog(catalog: &Catalog, limits: &ParseLimits) -> Result<(), ParseError> {
    if catalog.providers.len() > limits.max_providers {
        return Err(ValidationError::Providers.into());
    }
    let mut total_models = 0_usize;
    for (provider_id, provider) in &catalog.providers {
        identifier(provider_id, limits)?;
        if provider_id != &provider.id {
            return Err(ValidationError::IdentifierMismatch.into());
        }
        if provider.name.trim().is_empty() || provider.npm.trim().is_empty() {
            return Err(ParseError::Schema);
        }
        declarative_url(&provider.doc, false)?;
        if let Some(api) = &provider.api {
            declarative_url(api, true)?;
        }
        for variable in &provider.env {
            if variable.is_empty()
                || !variable
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(ValidationError::Environment.into());
            }
        }
        total_models = total_models.saturating_add(provider.models.len());
        if provider.models.len() > limits.max_models_per_provider
            || total_models > limits.max_models
        {
            return Err(ValidationError::Models.into());
        }
        for (model_id, model) in &provider.models {
            identifier(model_id, limits)?;
            if model_id != &model.id {
                return Err(ValidationError::IdentifierMismatch.into());
            }
            if let Some(canonical) = &model.canonical_model_id {
                identifier(canonical, limits)?;
            }
            if model.name.trim().is_empty() {
                return Err(ParseError::Schema);
            }
            match &model.interleaved {
                Some(Interleaved::Enabled(false)) => {
                    return Err(ValidationError::Interleaved.into());
                }
                Some(Interleaved::Field(field)) if field.field.trim().is_empty() => {
                    return Err(ValidationError::Interleaved.into());
                }
                _ => {}
            }
            if let Some(config) = &model.provider {
                if let Some(api) = &config.api {
                    declarative_url(api, true)?;
                }
                for (name, value) in &config.headers {
                    if name.is_empty()
                        || !name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                        || value.chars().any(char::is_control)
                    {
                        return Err(ParseError::Schema);
                    }
                }
            }
            if let Some(prices) = &model.cost {
                cost(prices)?;
            }
        }
    }
    Ok(())
}
