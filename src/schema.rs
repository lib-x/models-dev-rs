use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Number, Value};

/// Unknown fields are retained as data, never interpreted as executable configuration.
pub type Extensions = BTreeMap<String, Value>;

/// A nonnegative finite JSON number, retained without an intermediate binary float.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Price(Number);

impl Price {
    pub fn as_number(&self) -> &Number {
        &self.0
    }

    /// The decimal JSON representation, suitable for exact downstream decimal parsing.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for Price {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for Price {
    fn deserialize<Decoder: Deserializer<'de>>(decoder: Decoder) -> Result<Self, Decoder::Error> {
        let number = Number::deserialize(decoder)?;
        if number.as_str().starts_with('-') || !number.as_f64().is_some_and(f64::is_finite) {
            return Err(de::Error::custom("invalid price"));
        }
        Ok(Self(number))
    }
}

/// A validated calendar date in YYYY-MM or YYYY-MM-DD form.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct CatalogDate(String);

impl CatalogDate {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CatalogDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for CatalogDate {
    type Err = crate::ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        if !matches!(bytes.len(), 7 | 10)
            || bytes[4] != b'-'
            || (bytes.len() == 10 && bytes[7] != b'-')
            || !bytes
                .iter()
                .enumerate()
                .all(|(position, byte)| matches!(position, 4 | 7) || byte.is_ascii_digit())
        {
            return Err(crate::ParseError::Schema);
        }
        let year: u32 = value[..4].parse().map_err(|_| crate::ParseError::Schema)?;
        let month: usize = value[5..7].parse().map_err(|_| crate::ParseError::Schema)?;
        if !(1..=12).contains(&month) {
            return Err(crate::ParseError::Schema);
        }
        if bytes.len() == 10 {
            let day: u32 = value[8..].parse().map_err(|_| crate::ParseError::Schema)?;
            let leap_year =
                year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
            let days = [
                31,
                if leap_year { 29 } else { 28 },
                31,
                30,
                31,
                30,
                31,
                31,
                30,
                31,
                30,
                31,
            ];
            if day == 0 || day > days[month - 1] {
                return Err(crate::ParseError::Schema);
            }
        }
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for CatalogDate {
    fn deserialize<Decoder: Deserializer<'de>>(decoder: Decoder) -> Result<Self, Decoder::Error> {
        String::deserialize(decoder)?
            .parse()
            .map_err(|_| de::Error::custom("invalid calendar date"))
    }
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $($variant,)+
            Other(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Other(value) => value,
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $($value => Self::$variant,)+
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<Encoder: Serializer>(&self, encoder: Encoder) -> Result<Encoder::Ok, Encoder::Error> {
                encoder.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<Decoder: Deserializer<'de>>(decoder: Decoder) -> Result<Self, Decoder::Error> {
                Ok(Self::from(String::deserialize(decoder)?.as_str()))
            }
        }
    };
}

string_enum!(Modality {
    Text => "text", Audio => "audio", Image => "image", Video => "video", Pdf => "pdf",
});

string_enum!(Status {
    Alpha => "alpha", Beta => "beta", Deprecated => "deprecated",
});

string_enum!(ModelType {
    Chat => "chat", Image => "image", Video => "video", Embedding => "embedding",
    Reranking => "reranking", Decision => "decision", Transcription => "transcription",
    Speech => "speech", Realtime => "realtime",
});

/// USD per million tokens. Absence and zero are distinct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub input: Price,
    pub output: Price,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Price>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<Price>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<Price>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_audio: Option<Price>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_audio: Option<Price>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_over_200k: Option<Box<Cost>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<CostTier>,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostTier {
    pub tier: ContextTier,
    #[serde(flatten)]
    pub cost: Cost,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextTier {
    #[serde(rename = "type")]
    pub kind: String,
    pub size: u64,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub context: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    pub output: u64,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Modalities {
    pub input: Vec<Modality>,
    pub output: Vec<Modality>,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Interleaved {
    Enabled(bool),
    Field(InterleavedField),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterleavedField {
    pub field: String,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub body: Extensions,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extensions: Extensions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub attachment: Option<bool>,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub structured_output: Option<bool>,
    pub temperature: Option<bool>,
    pub open_weights: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model_id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ModelType>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_options: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interleaved: Option<Interleaved>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<CatalogDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<CatalogDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<CatalogDate>,
    pub modalities: Modalities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_weights: Option<bool>,
    pub limit: Limit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(flatten)]
    pub extensions: Extensions,
}

impl Model {
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            attachment: self.attachment,
            reasoning: self.reasoning,
            tool_call: self.tool_call,
            structured_output: self.structured_output,
            temperature: self.temperature,
            open_weights: self.open_weights,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub env: Vec<String>,
    pub npm: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    pub name: String,
    pub doc: String,
    pub models: BTreeMap<String, Model>,
    #[serde(flatten)]
    pub extensions: Extensions,
}
