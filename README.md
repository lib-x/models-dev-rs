# models-dev-rs

An independent Rust counterpart to the provider-catalog portion of
[`@opencode-ai/models`](https://github.com/anomalyco/models.dev/tree/dev/packages/sdk).
Parses [`https://models.dev/api.json`](https://models.dev/api.json) without I/O and
optionally retrieves it with a bounded, asynchronous rustls-only HTTP client.

This crate is not an official models.dev project and does **not** claim complete
SDK parity. The separate `models.json` and combined `catalog.json` endpoints,
bundled snapshots and Effect integration are outside version 0.1's scope.
The `Catalog` type here represents the **provider map from `api.json`**.
The minimum supported Rust version is 1.94 (edition 2024).

```toml
[dependencies]
models-dev-rs = "0.1"
```

## Parse without network access

For a caller-owned HTTPS/DNS-pinned transport, turn the optional client off:

```toml
models-dev-rs = { version = "0.1", default-features = false }
```

```rust
use models_dev_rs::{Capability, Catalog, ModelFilter};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let bytes = br#"{}"#;
let catalog = Catalog::from_slice(bytes)?;
let provider = catalog.provider("anthropic");
let model = catalog.model("anthropic", "claude-opus-4-6");
if let Some(price) = model.and_then(|model| model.cost.as_ref()) {
    println!("Exact USD per million input tokens: {}", price.input.as_str());
}
let filter = ModelFilter {
    capabilities: vec![Capability::ToolCall],
    min_context: Some(100_000),
    ..ModelFilter::default()
};
for entry in catalog.search(&filter) {
    println!("{}/{}", entry.provider.id, entry.model.id);
}
# let _ = provider;
# Ok(())
# }
```

`catalog.providers` and `provider.models` are public `BTreeMap`s, with stable
ID ordering. Search combines all specified filters using AND. Input/output
modality and capability lists require all listed values; the text query is a
Unicode-lowercased substring of model ID, name, description or provider name.

## Optional asynchronous client

Requires a Tokio runtime. See `examples/fetch.rs` for an executable example.

```rust,no_run
# #[cfg(feature = "client")]
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
use models_dev_rs::{Client, Refresh};

let client = Client::new()?;
let first = client.fetch().await?;
match client.refresh(&first.metadata).await? {
    Refresh::Modified(next) => {
        // The consumer atomically persists/activates next.catalog and next.metadata.
        println!("{} providers", next.catalog.providers.len());
    }
    Refresh::NotModified(metadata) => {
        // Keep the previous catalog and persist any updated validators.
        let _ = metadata;
    }
}
# Ok(())
# }
```

`Download` contains `catalog`, original **decoded** `body` bytes and `metadata`.
`CacheMetadata` is serializable and contains `source`, `etag`, `last_modified`.
Validators are checked and scoped to the exact endpoint; do not reuse them for
another URL. A 200 replaces validators, while a 304 retains missing validators
and can update supplied ones. An unsolicited 304 is an error. Refresh never
modifies a previous catalog, and no disk cache or automatic retry is provided.

`Client::builder()` configures `endpoint`, `timeout`, `connect_timeout`,
`max_download_bytes`, and `parse_limits`. Defaults are a 30-second overall
request timeout, 10-second connection timeout, 16 MiB wire limit and independent
16 MiB decoded/parser limit. Only identity and gzip content encodings are
accepted; concatenated gzip members are checked together and decoded into a
bounded buffer. All redirects, HTTP endpoints, credential-bearing URLs, URL
queries/fragments and environment proxies are disabled. No arbitrary transport
injection is exposed; loopback HTTP is enabled only inside the crate's test build.

This is not a general SSRF-safe fetcher: it does not pin DNS or reject private
addresses. Consumers requiring those policies must use their own transport and
`Catalog::from_slice`. No endpoint from catalog data is ever contacted.

## Data and validation contract

- `Price` retains a `serde_json::Number` using `arbitrary_precision`; `as_str`,
  `as_number` and `Display` never round through binary floating point. Known
  prices must be nonnegative and finite in the f64 numeric range, but are stored
  exactly. Source decimal digits are preserved; JSON exponent spelling may be
  normalized by serde_json. Prices are reference USD per million tokens, **not**
  a billing ledger. Missing pricing stays `None`, distinct from a free model.
- Costs expose input/output, reasoning, cache read/write, audio input/output,
  legacy `context_over_200k` and exact `tiers`. Token limits are unsigned integers;
  zero is retained as published, not treated as measured usage.
- Models expose typed modalities, capabilities, status and calendar-validated
  `YYYY-MM`/`YYYY-MM-DD` dates. Unknown modality/type/status strings use `Other`.
  All capability booleans and dates are optional: missing/null is `None`, never
  silently converted to `false`. `capabilities()` returns these same nullable facts.
  `reasoning_options` and `experimental` remain bounded declarative JSON.
- Unknown fields at provider, model, cost, tier, limit, modality and provider
  configuration boundaries survive in `extensions`. They cannot enable an
  adapter, execute code, change routing or authorize a request. Public models
  can be constructed/mutated by the caller; only `Catalog::from_slice` and client
  downloads establish the validation contract, not bare serde deserialization.
- Defaults additionally bound depth (32, configurable up to 64), nodes
  (1,000,000), each collection (16,384), decoded string bytes (65,536), numeric
  literal bytes (256), identifier bytes (512), providers (4,096), models per
  provider (8,192), and total models (65,536). `ParseLimits` may be tightened by
  the consumer; increasing limits is an explicit trusted policy decision.
- Duplicate keys are rejected recursively before typed parsing. Provider/model
  map keys must equal their IDs. IDs allow vendor-qualified forms such as
  `@cf/...`, `~anthropic/...`, slashes and colons, but not whitespace/controls.
  Declarative provider/model endpoints can contain WebSocket schemes and named
  `${ENVIRONMENT_VARIABLE}` placeholders; they are preserved verbatim, never
  expanded or contacted. Environment labels such as `302AI_API_KEY` are accepted.
- Authentication environment-variable **names** are metadata, not credentials.
  Common credential-bearing keys (authorization/cookie/API key/password/token/
  secret variants), URL userinfo, credential URL query parameters, control-bearing
  keys and reserved serde_json keys are rejected, including inside extensions.
  This cannot detect arbitrary unlabeled secrets; never submit secrets as metadata.
- Parse/client errors carry only bounded categories, counts/locations or HTTP
  status, never payloads, URLs, header values or raw transport causes.

Parsing and bounded gzip decoding are synchronous CPU work within the async
fetch operation. Applications with latency-sensitive executors can parse their
own downloaded bytes on a blocking worker. No build step accesses the network;
tests use only synthetic fixtures and loopback mock servers.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo test --locked --no-default-features
cargo doc --locked --all-features --no-deps
cargo package --locked
```

`examples/parse.rs` can smoke-test a separately downloaded live API file without
checking that payload into the repository. Schema/behavior references were the
official MIT SDK and core schema at upstream commit
`f4b7584ae9667165005722ac29c1590ca16463dd` (2026-10-23). Fixtures are independently
authored, not copied snapshots. Licensed under MIT; catalog data and provider
trademarks remain subject to their respective terms.
