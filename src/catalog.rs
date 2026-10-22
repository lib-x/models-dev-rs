use std::collections::BTreeMap;

use serde::Serialize;

use crate::{Modality, Model, ModelType, ParseError, ParseLimits, Provider, Status, validation};

/// The provider map returned by `api.json`, not the SDK's combined `catalog.json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Catalog {
    pub providers: BTreeMap<String, Provider>,
}

impl Catalog {
    pub fn from_slice(input: &[u8]) -> Result<Self, ParseError> {
        Self::from_slice_with_limits(input, &ParseLimits::default())
    }

    pub fn from_slice_with_limits(input: &[u8], limits: &ParseLimits) -> Result<Self, ParseError> {
        validation::preflight(input, limits)?;
        let providers = serde_json::from_slice(input).map_err(|_| ParseError::Schema)?;
        let catalog = Self { providers };
        validation::catalog(&catalog, limits)?;
        Ok(catalog)
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.get(id)
    }

    pub fn model(&self, provider: &str, id: &str) -> Option<&Model> {
        self.provider(provider)?.models.get(id)
    }

    /// Results are ordered by provider ID, then model ID. All filters are ANDed.
    pub fn search<'catalog>(
        &'catalog self,
        filter: &'catalog ModelFilter,
    ) -> impl Iterator<Item = ModelRef<'catalog>> + 'catalog {
        let query = filter.query.as_ref().map(|value| value.to_lowercase());
        self.providers
            .values()
            .filter(move |provider| filter.provider.as_ref().is_none_or(|id| id == &provider.id))
            .flat_map(|provider| {
                provider
                    .models
                    .values()
                    .map(move |model| ModelRef { provider, model })
            })
            .filter(move |entry| {
                let model = entry.model;
                query.as_ref().is_none_or(|query| {
                    [
                        &model.id,
                        &model.name,
                        &model.description,
                        &entry.provider.name,
                    ]
                    .iter()
                    .any(|value| value.to_lowercase().contains(query))
                }) && filter
                    .family
                    .as_ref()
                    .is_none_or(|family| model.family.as_ref() == Some(family))
                    && filter
                        .kind
                        .as_ref()
                        .is_none_or(|kind| model.kind.as_ref() == Some(kind))
                    && filter
                        .status
                        .as_ref()
                        .is_none_or(|status| model.status.as_ref() == Some(status))
                    && filter
                        .min_context
                        .is_none_or(|minimum| model.limit.context >= minimum)
                    && filter
                        .open_weights
                        .is_none_or(|required| model.open_weights == Some(required))
                    && filter
                        .input
                        .iter()
                        .all(|modality| model.modalities.input.contains(modality))
                    && filter
                        .output
                        .iter()
                        .all(|modality| model.modalities.output.contains(modality))
                    && filter
                        .capabilities
                        .iter()
                        .all(|capability| capability.supported_by(model))
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ModelRef<'catalog> {
    pub provider: &'catalog Provider,
    pub model: &'catalog Model,
}

#[derive(Clone, Debug, Default)]
pub struct ModelFilter {
    pub provider: Option<String>,
    pub query: Option<String>,
    pub family: Option<String>,
    pub kind: Option<ModelType>,
    pub status: Option<Status>,
    pub min_context: Option<u64>,
    pub open_weights: Option<bool>,
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Capability {
    Attachment,
    Reasoning,
    ToolCall,
    StructuredOutput,
    Temperature,
}

impl Capability {
    fn supported_by(self, model: &Model) -> bool {
        match self {
            Self::Attachment => model.attachment == Some(true),
            Self::Reasoning => model.reasoning == Some(true),
            Self::ToolCall => model.tool_call == Some(true),
            Self::StructuredOutput => model.structured_output == Some(true),
            Self::Temperature => model.temperature == Some(true),
        }
    }
}
