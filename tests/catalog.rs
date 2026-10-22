use models_dev_rs::{
    Capability, Catalog, CatalogDate, DEFAULT_MAX_BYTES, Modality, ModelFilter, ParseError,
    ParseLimits, Status, ValidationError,
};
use serde_json::{Value, json};

const FIXTURE: &[u8] = include_bytes!("fixtures/catalog.json");

fn modified(change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    change(&mut value);
    serde_json::to_vec(&value).unwrap()
}

#[test]
fn parses_lookup_capabilities_and_exact_prices() {
    let catalog = Catalog::from_slice(FIXTURE).unwrap();
    assert_eq!(catalog.providers.len(), 1);
    assert_eq!(
        catalog.provider("example").unwrap().env,
        ["EXAMPLE_API_KEY"]
    );
    let model = catalog.model("example", "reasoner").unwrap();
    let cost = model.cost.as_ref().unwrap();
    assert_eq!(cost.input.as_str(), "0.12345678901234567890123456789");
    assert_eq!(cost.cache_read.as_ref().unwrap().to_string(), "0");
    assert_eq!(cost.tiers[0].tier.size, 200001);
    assert_eq!(cost.tiers[0].cost.output.to_string(), "2.5");
    assert_eq!(
        cost.context_over_200k.as_ref().unwrap().input.to_string(),
        "0.25"
    );
    assert_eq!(model.limit.input, Some(180000));
    assert_eq!(model.capabilities().reasoning, Some(true));
    assert_eq!(model.capabilities().temperature, Some(false));
    assert_eq!(model.release_date.as_ref().unwrap().as_str(), "2026-02-28");
    assert_eq!(model.status, Some(Status::Beta));
    assert!(catalog.model("missing", "reasoner").is_none());
    assert!(catalog.model("example", "missing").is_none());
}

#[test]
fn absent_prices_and_capabilities_stay_unknown() {
    let catalog = Catalog::from_slice(FIXTURE).unwrap();
    let model = catalog.model("example", "subscription").unwrap();
    assert!(model.cost.is_none());
    assert_eq!(model.structured_output, None);
    assert_eq!(model.status, None);
    assert_eq!(model.limit.context, 0);
}

#[test]
fn missing_and_null_capabilities_remain_unknown_not_false() {
    let bytes = modified(|value| {
        let model = value["example"]["models"]["reasoner"]
            .as_object_mut()
            .unwrap();
        model.remove("attachment");
        model.remove("reasoning");
        model.remove("release_date");
        model.insert("tool_call".into(), Value::Null);
        model.insert("open_weights".into(), Value::Null);
    });
    let catalog = Catalog::from_slice(&bytes).unwrap();
    let model = catalog.model("example", "reasoner").unwrap();
    assert_eq!(model.attachment, None);
    assert_eq!(model.reasoning, None);
    assert_eq!(model.tool_call, None);
    assert_eq!(model.open_weights, None);
    assert_eq!(model.release_date, None);
}

#[test]
fn vendor_qualified_identifiers_are_preserved_verbatim() {
    for identifier in [
        "@cf/meta/model",
        "~anthropic/claude",
        "vendor/model:version",
    ] {
        let bytes = modified(|value| {
            let models = value["example"]["models"].as_object_mut().unwrap();
            let mut model = models.remove("reasoner").unwrap();
            model["id"] = json!(identifier);
            models.insert(identifier.to_owned(), model);
        });
        let catalog = Catalog::from_slice(&bytes).unwrap();
        assert_eq!(catalog.model("example", identifier).unwrap().id, identifier);
    }
}

#[test]
fn every_extension_and_unknown_enum_survives_round_trip() {
    let bytes = modified(|value| {
        value["example"]["models"]["reasoner"]["status"] = json!("preview-next");
        value["example"]["models"]["reasoner"]["modalities"]["input"]
            .as_array_mut()
            .unwrap()
            .push(json!("sensor"));
    });
    let catalog = Catalog::from_slice(&bytes).unwrap();
    let model = catalog.model("example", "reasoner").unwrap();
    assert_eq!(model.status, Some(Status::Other("preview-next".into())));
    assert_eq!(
        model.modalities.input.last(),
        Some(&Modality::Other("sensor".into()))
    );
    assert_eq!(
        model.extensions["future_model"]["nested"][2]["label"],
        "preserved"
    );
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    let output = serde_json::to_value(&catalog).unwrap();
    assert_eq!(output, original);
    assert_eq!(
        Catalog::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap(),
        catalog
    );
}

#[test]
fn search_is_deterministic_and_combines_filters() {
    let catalog = Catalog::from_slice(FIXTURE).unwrap();
    let default_filter = ModelFilter::default();
    let names: Vec<_> = catalog
        .search(&default_filter)
        .map(|entry| entry.model.id.as_str())
        .collect();
    assert_eq!(names, ["reasoner", "subscription"]);
    let filter = ModelFilter {
        provider: Some("example".into()),
        query: Some("REASONER".into()),
        family: Some("example-family".into()),
        status: Some(Status::Beta),
        min_context: Some(100000),
        input: vec![Modality::Image],
        capabilities: vec![
            Capability::Reasoning,
            Capability::ToolCall,
            Capability::StructuredOutput,
        ],
        open_weights: Some(false),
        ..ModelFilter::default()
    };
    assert_eq!(catalog.search(&filter).count(), 1);
    let impossible = ModelFilter {
        capabilities: vec![Capability::Temperature],
        ..filter
    };
    assert_eq!(catalog.search(&impossible).count(), 0);
}

#[test]
fn rejects_invalid_known_price_and_token_shapes() {
    for replacement in [json!(-0.1), json!("1.25"), json!(null), json!(true)] {
        let bytes =
            modified(|value| value["example"]["models"]["reasoner"]["cost"]["input"] = replacement);
        assert_eq!(Catalog::from_slice(&bytes).unwrap_err(), ParseError::Schema);
    }
    for number in ["1e999", "-0", "NaN", "Infinity"] {
        let input = String::from_utf8(FIXTURE.to_vec())
            .unwrap()
            .replace("0.12345678901234567890123456789", number);
        assert!(Catalog::from_slice(input.as_bytes()).is_err(), "{number}");
    }
    for replacement in [
        json!(-1),
        json!(1.5),
        json!("100"),
        json!(18446744073709551616_u128),
    ] {
        let bytes = modified(|value| {
            value["example"]["models"]["reasoner"]["limit"]["context"] = replacement
        });
        assert!(Catalog::from_slice(&bytes).is_err());
    }
}

#[test]
fn dates_are_calendar_valid_not_just_regex_valid() {
    for valid in ["2024-02-29", "2000-02-29", "2026-10", "2026-12-31"] {
        assert_eq!(valid.parse::<CatalogDate>().unwrap().as_str(), valid);
    }
    for invalid in [
        "2026-02-29",
        "1900-02-29",
        "2026-00",
        "2026-13",
        "2026-04-31",
        "2026-1",
        "é026-01",
        "2026-01-00",
    ] {
        assert!(invalid.parse::<CatalogDate>().is_err(), "{invalid}");
    }
    let bytes = modified(|value| {
        value["example"]["models"]["reasoner"]["release_date"] = json!("2026-02-30")
    });
    assert_eq!(Catalog::from_slice(&bytes).unwrap_err(), ParseError::Schema);
}

#[test]
fn rejects_duplicate_keys_in_all_locations_and_reserved_serde_keys() {
    for bytes in [
        br#"{"p":{},"p":{}}"#.as_slice(),
        br#"{"p":{"id":"p","id":"other"}}"#.as_slice(),
        br#"{"p":{"future":[{"key":1,"key":2}]}}"#.as_slice(),
    ] {
        assert_eq!(
            Catalog::from_slice(bytes).unwrap_err(),
            ParseError::Validation(ValidationError::DuplicateKey)
        );
    }
    assert_eq!(
        Catalog::from_slice(br#"{"$serde_json::private::Number":"42"}"#).unwrap_err(),
        ParseError::Validation(ValidationError::ObjectKey)
    );
}

#[test]
fn rejects_identifier_mismatch_control_and_whitespace() {
    let bytes = modified(|value| value["example"]["id"] = json!("other"));
    assert_eq!(
        Catalog::from_slice(&bytes).unwrap_err(),
        ParseError::Validation(ValidationError::IdentifierMismatch)
    );
    let bytes = modified(|value| {
        value["example"]["models"]["reasoner"]["id"] = json!("other");
    });
    assert_eq!(
        Catalog::from_slice(&bytes).unwrap_err(),
        ParseError::Validation(ValidationError::IdentifierMismatch)
    );
    for identifier in ["bad id", "bad\n", "bad\u{007f}", ""] {
        let bytes = modified(|value| {
            let mut provider = value["example"].take();
            provider["id"] = json!(identifier);
            *value = json!({ identifier: provider });
        });
        assert!(Catalog::from_slice(&bytes).is_err());
    }
}

#[test]
fn rejects_labeled_credentials_even_in_future_fields() {
    for key in [
        "Authorization",
        "cookie",
        "api_key",
        "x-api-key",
        "password",
        "clientSecret",
        "access_token",
    ] {
        let bytes = modified(|value| {
            value["example"]["future_provider"] = json!({ key: "sensitive-do-not-echo" })
        });
        let error = Catalog::from_slice(&bytes).unwrap_err();
        assert_eq!(error, ParseError::Validation(ValidationError::Credentials));
        assert!(!format!("{error:?} {error}").contains("sensitive-do-not-echo"));
    }
    for url in [
        "https://user:sensitive-do-not-echo@example.com",
        "https://example.com/?api_key=sensitive-do-not-echo",
    ] {
        let bytes = modified(|value| value["example"]["api"] = json!(url));
        assert_eq!(
            Catalog::from_slice(&bytes).unwrap_err(),
            ParseError::Validation(ValidationError::Credentials)
        );
    }
}

#[test]
fn malformed_payloads_never_leak_schema_data() {
    for input in [
        br#"{"sensitive-do-not-echo":"secret"}"#.as_slice(),
        br#"{"secret-value":invalid}"#.as_slice(),
    ] {
        let error = Catalog::from_slice(input).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("sensitive"));
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
}

#[test]
fn enforces_byte_depth_node_collection_and_string_limits() {
    let limit = ParseLimits {
        max_bytes: FIXTURE.len() - 1,
        ..ParseLimits::default()
    };
    assert_eq!(
        Catalog::from_slice_with_limits(FIXTURE, &limit).unwrap_err(),
        ParseError::TooLarge {
            limit: FIXTURE.len() - 1
        }
    );
    for (limits, expected) in [
        (
            ParseLimits {
                max_depth: 2,
                ..ParseLimits::default()
            },
            ValidationError::Depth,
        ),
        (
            ParseLimits {
                max_nodes: 2,
                ..ParseLimits::default()
            },
            ValidationError::Nodes,
        ),
        (
            ParseLimits {
                max_collection_items: 2,
                ..ParseLimits::default()
            },
            ValidationError::Collection,
        ),
        (
            ParseLimits {
                max_string_bytes: 5,
                ..ParseLimits::default()
            },
            ValidationError::StringLength,
        ),
        (
            ParseLimits {
                max_models: 1,
                ..ParseLimits::default()
            },
            ValidationError::Models,
        ),
        (
            ParseLimits {
                max_models_per_provider: 1,
                ..ParseLimits::default()
            },
            ValidationError::Models,
        ),
        (
            ParseLimits {
                max_id_bytes: 2,
                ..ParseLimits::default()
            },
            ValidationError::Identifier,
        ),
    ] {
        assert_eq!(
            Catalog::from_slice_with_limits(FIXTURE, &limits).unwrap_err(),
            ParseError::Validation(expected)
        );
    }
    let limits = ParseLimits {
        max_depth: 65,
        ..ParseLimits::default()
    };
    assert_eq!(
        Catalog::from_slice_with_limits(FIXTURE, &limits).unwrap_err(),
        ParseError::InvalidLimits
    );
}

#[test]
fn validates_declarative_urls_environment_and_interleaving() {
    for value in ["javascript:alert(1)", "file:///etc/passwd", "not a URL"] {
        let bytes = modified(|catalog| catalog["example"]["doc"] = json!(value));
        assert_eq!(
            Catalog::from_slice(&bytes).unwrap_err(),
            ParseError::Validation(ValidationError::Url)
        );
    }
    let bytes = modified(|value| value["example"]["env"] = json!(["API_KEY=plaintext"]));
    assert_eq!(
        Catalog::from_slice(&bytes).unwrap_err(),
        ParseError::Validation(ValidationError::Environment)
    );
    let bytes =
        modified(|value| value["example"]["models"]["reasoner"]["interleaved"] = json!(false));
    assert_eq!(
        Catalog::from_slice(&bytes).unwrap_err(),
        ParseError::Validation(ValidationError::Interleaved)
    );
    assert!(Catalog::from_slice(b"{}").unwrap().providers.is_empty());
}

#[test]
fn accepts_published_environment_labels_beginning_with_digits() {
    let bytes = modified(|value| value["example"]["env"] = json!(["302AI_API_KEY"]));
    let catalog = Catalog::from_slice(&bytes).unwrap();
    assert_eq!(catalog.provider("example").unwrap().env, ["302AI_API_KEY"]);
}

#[test]
fn default_byte_cap_is_exactly_sixteen_mebibytes() {
    assert_eq!(DEFAULT_MAX_BYTES, 16 * 1024 * 1024);
    assert_eq!(ParseLimits::default().max_bytes, DEFAULT_MAX_BYTES);
    let too_large = vec![b' '; DEFAULT_MAX_BYTES + 1];
    assert_eq!(
        Catalog::from_slice(&too_large).unwrap_err(),
        ParseError::TooLarge {
            limit: DEFAULT_MAX_BYTES
        }
    );
}

#[test]
fn preserves_declarative_websocket_and_environment_template_endpoints() {
    for endpoint in [
        "wss://example.com/api-ws/v1/realtime",
        "https://${AZURE_RESOURCE_NAME}.services.ai.azure.com/models",
        "${NEON_AI_GATEWAY_BASE_URL}/v1",
        "https://${GOOGLE_VERTEX_ENDPOINT}/v1/projects/${GOOGLE_VERTEX_PROJECT}",
    ] {
        let bytes = modified(|value| {
            value["example"]["api"] = json!(endpoint);
            value["example"]["models"]["reasoner"]["provider"]["api"] = json!(endpoint);
        });
        let catalog = Catalog::from_slice(&bytes).unwrap();
        assert_eq!(
            catalog.provider("example").unwrap().api.as_deref(),
            Some(endpoint)
        );
        assert_eq!(
            catalog
                .model("example", "reasoner")
                .unwrap()
                .provider
                .as_ref()
                .unwrap()
                .api
                .as_deref(),
            Some(endpoint)
        );
    }
}
