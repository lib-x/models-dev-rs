# models-dev-rs Implementation Plan

> **For agentic workers:** Execute inline using the executing-plans workflow. No commits, remotes or publication are authorized in this implementation task.

**Goal:** Build an independent Rust 0.1.0 client suitable for Pangolin's catalog ingestion.

**Architecture:** Separate bounded `api.json` parsing from an optional reqwest transport. Keep unknown fields as declarative JSON and leave cache activation, signatures and DNS pinning to the consumer.

**Tech Stack:** Rust 2024, serde/serde_json arbitrary precision, reqwest 0.13 rustls, pure-Rust flate2, Tokio mock tests.

**Spec:** The task request and Pangolin's `specs/SPEC-modelsdev-and-gateway-usability.md`; the official models.dev SDK types and live `api.json`, inspected against upstream commit `f4b7584ae9667165005722ac29c1590ca16463dd` on 2026-10-23.

## Global Constraints

- Version 0.1.0; MIT; no network at build time; no checked-in upstream snapshot.
- No copying from AGPL projects; use the official SDK as a behavior/schema reference.
- Preserve unknown fields and enum strings without executing provider instructions.
- Download and decompression default to separate 16 MiB caps; HTTPS without redirects or credentials.
- Errors never include payloads, URLs, response headers or underlying transport causes.
- No git initialization, commits, remote creation or publication in this task.

## Review Focus

- Unknown nested extensions and exact decimals must survive serialization.
- Missing pricing must remain absent rather than become an invented zero.
- Duplicate keys, oversized dimensions and embedded credentials must be rejected.
- Conditional refresh must distinguish a 304 from a newly validated document.
- Redirects, gzip bombs, timeouts and malformed responses must fail safely offline.

## Tasks

- [x] Add the typed schema, validation limits, lookups/search and focused parser tests.
- [x] Add the optional client, source-scoped validators and offline HTTP mock contracts.
- [x] Document the exact API, ship examples and CI, and verify fmt/clippy/test/package with and without the client feature.

## Compatibility Findings

- Capability flags and dates are nullable rather than invented false/empty values.
- Published environment labels can start with digits (`302AI_API_KEY`); a focused test failed before allowing this declarative label format.
- Published endpoints include WebSocket schemes and `${ENVIRONMENT_VARIABLE}` templates; a focused test failed before accepting these as non-executable metadata.
- The separately downloaded live response parses as 232 providers and 9,149 models. The full response is not checked in.
- Minimum supported Rust is 1.94, verified locally in both feature configurations, alongside Rust 1.99.1.
- Independent final review and every publication/release action remain with the parent delivery task.
