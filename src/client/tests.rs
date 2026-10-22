use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use flate2::{Compression, write::GzEncoder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use super::*;

const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/catalog.json");
const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

struct Reply {
    bytes: Vec<u8>,
    delay: Duration,
    body_delay: Duration,
}

impl Reply {
    fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Self {
        let mut bytes = format!(
            "HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        )
        .into_bytes();
        for (name, value) in headers {
            bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(body);
        Self {
            bytes,
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        }
    }
}

struct Mock {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Mock {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api.json", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let size = stream.read(&mut buffer).await.unwrap();
                    if size == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..size]);
                    assert!(request.len() <= 8192);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                tokio::time::sleep(reply.delay).await;
                let boundary = reply
                    .bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let _ = stream.write_all(&reply.bytes[..boundary]).await;
                tokio::time::sleep(reply.body_delay).await;
                let _ = stream.write_all(&reply.bytes[boundary..]).await;
            }
        });
        Self {
            endpoint,
            requests,
            task,
        }
    }

    fn builder(&self) -> ClientBuilder {
        let mut builder = Client::builder().endpoint(&self.endpoint);
        builder.allow_http = true;
        builder
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn public_builder_is_https_only_and_credential_free() {
    for endpoint in [
        "http://127.0.0.1/api.json",
        "https://user:do-not-echo@example.com/api.json",
        "https://example.com/api.json?api_key=do-not-echo",
        "https://example.com/api.json#fragment",
        "file:///etc/passwd",
        "not-a-url",
    ] {
        let error = Client::builder().endpoint(endpoint).build().unwrap_err();
        assert_eq!(error, ClientError::InvalidEndpoint);
        assert!(!format!("{error:?} {error}").contains("do-not-echo"));
    }
    assert_eq!(
        Client::builder()
            .timeout(Duration::ZERO)
            .build()
            .unwrap_err(),
        ClientError::InvalidConfiguration
    );
    assert_eq!(
        Client::builder().max_download_bytes(0).build().unwrap_err(),
        ClientError::InvalidConfiguration
    );
    assert_eq!(
        Client::builder()
            .parse_limits(ParseLimits {
                max_depth: 65,
                ..ParseLimits::default()
            })
            .build()
            .unwrap_err(),
        ClientError::InvalidConfiguration
    );
    assert!(Client::new().is_ok());
}

#[tokio::test]
async fn fetches_validated_body_and_sends_conditional_validators() {
    let mock = Mock::start(vec![
        Reply::response(
            200,
            &[("ETag", "W/\"v1\""), ("Last-Modified", LAST_MODIFIED)],
            FIXTURE,
        ),
        Reply::response(304, &[("ETag", "\"v2\"")], b""),
    ])
    .await;
    let client = mock.builder().build().unwrap();
    let first = client.fetch().await.unwrap();
    assert_eq!(first.catalog.providers.len(), 1);
    assert_eq!(first.body, FIXTURE);
    assert_eq!(
        first.metadata.source.as_deref(),
        Some(mock.endpoint.as_str())
    );
    let Refresh::NotModified(metadata) = client.refresh(&first.metadata).await.unwrap() else {
        panic!("expected not modified")
    };
    assert_eq!(metadata.etag.as_deref(), Some("\"v2\""));
    assert_eq!(metadata.last_modified.as_deref(), Some(LAST_MODIFIED));
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    let conditional = requests[1].to_lowercase();
    assert!(conditional.contains("if-none-match: w/\"v1\""));
    assert!(conditional.contains("if-modified-since: wed, 21 oct 2015 07:28:00 gmt"));
    assert!(conditional.contains("accept-encoding: gzip"));
    assert!(!conditional.contains("authorization:"));
}

#[tokio::test]
async fn last_modified_only_refresh_retains_metadata_on_304() {
    let mock = Mock::start(vec![
        Reply::response(200, &[("Last-Modified", LAST_MODIFIED)], FIXTURE),
        Reply::response(304, &[], b""),
    ])
    .await;
    let client = mock.builder().build().unwrap();
    let first = client.fetch().await.unwrap();
    let Refresh::NotModified(metadata) = client.refresh(&first.metadata).await.unwrap() else {
        panic!("expected 304")
    };
    assert_eq!(metadata, first.metadata);
    assert!(!mock.requests()[1].to_lowercase().contains("if-none-match"));
}

#[tokio::test]
async fn a_new_document_replaces_instead_of_inheriting_old_validators() {
    let mock = Mock::start(vec![
        Reply::response(200, &[("ETag", "\"v1\"")], FIXTURE),
        Reply::response(200, &[], FIXTURE),
    ])
    .await;
    let client = mock.builder().build().unwrap();
    let first = client.fetch().await.unwrap();
    let Refresh::Modified(second) = client.refresh(&first.metadata).await.unwrap() else {
        panic!("expected 200")
    };
    assert_eq!(second.metadata.etag, None);
    assert_eq!(second.metadata.last_modified, None);
}

#[tokio::test]
async fn refuses_unscoped_wrong_source_or_injected_validators_before_io() {
    let mock = Mock::start(vec![]).await;
    let client = mock.builder().build().unwrap();
    let metadata = CacheMetadata {
        etag: Some("\"v1\"".into()),
        ..CacheMetadata::default()
    };
    assert_eq!(
        client.refresh(&metadata).await.unwrap_err(),
        ClientError::ValidatorSourceMismatch
    );
    let metadata = CacheMetadata {
        source: Some("https://other.example/api.json".into()),
        ..metadata
    };
    assert_eq!(
        client.refresh(&metadata).await.unwrap_err(),
        ClientError::ValidatorSourceMismatch
    );
    let metadata = CacheMetadata {
        source: Some(mock.endpoint.clone()),
        etag: Some("\"do-not-echo\"\r\nx-api-key: secret".into()),
        last_modified: None,
    };
    let error = client.refresh(&metadata).await.unwrap_err();
    assert_eq!(error, ClientError::InvalidValidator);
    assert!(!format!("{error:?} {error}").contains("do-not-echo"));
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn rejects_invalid_duplicate_and_oversized_response_validators() {
    for headers in [
        vec![("ETag", "not-quoted")],
        vec![("ETag", "\"one\""), ("ETag", "\"two\"")],
        vec![("Last-Modified", "do-not-echo")],
    ] {
        let mock = Mock::start(vec![Reply::response(200, &headers, FIXTURE)]).await;
        assert_eq!(
            mock.builder().build().unwrap().fetch().await.unwrap_err(),
            ClientError::InvalidValidator
        );
    }
    let large = format!("\"{}\"", "a".repeat(MAX_VALIDATOR_BYTES));
    let mock = Mock::start(vec![Reply::response(200, &[("ETag", &large)], FIXTURE)]).await;
    assert_eq!(
        mock.builder().build().unwrap().fetch().await.unwrap_err(),
        ClientError::InvalidValidator
    );
}

#[tokio::test]
async fn unsolicited_304_and_non_200_success_are_not_accepted() {
    for status in [201, 204, 206, 304, 404, 429, 500] {
        let mock = Mock::start(vec![Reply::response(status, &[], b"do-not-echo")]).await;
        let error = mock.builder().build().unwrap().fetch().await.unwrap_err();
        if status == 304 {
            assert_eq!(error, ClientError::NotModifiedWithoutCache);
        } else {
            assert_eq!(error, ClientError::UnexpectedStatus { status });
        }
        assert!(!format!("{error:?} {error}").contains("do-not-echo"));
    }
}

#[tokio::test]
async fn never_follows_redirects() {
    let mock = Mock::start(vec![Reply::response(
        302,
        &[("Location", "http://127.0.0.1:1/do-not-echo")],
        b"",
    )])
    .await;
    assert_eq!(
        mock.builder().build().unwrap().fetch().await.unwrap_err(),
        ClientError::UnexpectedStatus { status: 302 }
    );
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn caps_announced_and_chunked_wire_bodies() {
    let mock = Mock::start(vec![Reply::response(200, &[], FIXTURE)]).await;
    assert_eq!(
        mock.builder()
            .max_download_bytes(16)
            .build()
            .unwrap()
            .fetch()
            .await
            .unwrap_err(),
        ClientError::DownloadTooLarge { limit: 16 }
    );
    let reply = Reply { bytes: b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n12345678\r\n8\r\nabcdefgh\r\n0\r\n\r\n".to_vec(), delay: Duration::ZERO, body_delay: Duration::ZERO };
    let mock = Mock::start(vec![reply]).await;
    assert_eq!(
        mock.builder()
            .max_download_bytes(10)
            .build()
            .unwrap()
            .fetch()
            .await
            .unwrap_err(),
        ClientError::DownloadTooLarge { limit: 10 }
    );
}

#[tokio::test]
async fn gzip_and_concatenated_members_work_without_losing_exact_prices() {
    let mut concatenated = gzip(&FIXTURE[..100]);
    concatenated.extend_from_slice(&gzip(&FIXTURE[100..]));
    for compressed in [gzip(FIXTURE), concatenated] {
        let mock = Mock::start(vec![Reply::response(
            200,
            &[("Content-Encoding", "gzip")],
            &compressed,
        )])
        .await;
        let download = mock.builder().build().unwrap().fetch().await.unwrap();
        assert_eq!(download.body, FIXTURE);
        assert_eq!(
            download
                .catalog
                .model("example", "reasoner")
                .unwrap()
                .cost
                .as_ref()
                .unwrap()
                .input
                .as_str(),
            "0.12345678901234567890123456789"
        );
    }
}

#[tokio::test]
async fn decoded_limit_applies_to_identity_and_gzip_bombs() {
    for compressed in [false, true] {
        let body = if compressed {
            gzip(&vec![b' '; 65536])
        } else {
            vec![b' '; 65536]
        };
        let headers = if compressed {
            vec![("Content-Encoding", "gzip")]
        } else {
            vec![]
        };
        let mock = Mock::start(vec![Reply::response(200, &headers, &body)]).await;
        let limits = ParseLimits {
            max_bytes: 1024,
            ..ParseLimits::default()
        };
        assert_eq!(
            mock.builder()
                .parse_limits(limits)
                .build()
                .unwrap()
                .fetch()
                .await
                .unwrap_err(),
            ClientError::DecodedTooLarge { limit: 1024 }
        );
    }
}

#[tokio::test]
async fn rejects_unsupported_stacked_or_corrupt_encoding() {
    for encoding in ["br", "deflate", "gzip, gzip"] {
        let mock = Mock::start(vec![Reply::response(
            200,
            &[("Content-Encoding", encoding)],
            FIXTURE,
        )])
        .await;
        assert_eq!(
            mock.builder().build().unwrap().fetch().await.unwrap_err(),
            ClientError::UnsupportedEncoding
        );
    }
    let mut truncated = gzip(FIXTURE);
    truncated.truncate(truncated.len() - 5);
    for compressed in [b"not gzip".to_vec(), truncated] {
        let mock = Mock::start(vec![Reply::response(
            200,
            &[("Content-Encoding", "gzip")],
            &compressed,
        )])
        .await;
        assert_eq!(
            mock.builder().build().unwrap().fetch().await.unwrap_err(),
            ClientError::InvalidCompression
        );
    }
}

#[tokio::test]
async fn malformed_or_failed_refresh_leaves_caller_owned_previous_data_intact() {
    let mock = Mock::start(vec![
        Reply::response(200, &[("ETag", "\"v1\"")], FIXTURE),
        Reply::response(200, &[("ETag", "\"bad\"")], b"{\"do-not-echo\": false}"),
        Reply::response(503, &[], b"do-not-echo"),
    ])
    .await;
    let client = mock.builder().build().unwrap();
    let first = client.fetch().await.unwrap();
    let original = first.metadata.clone();
    let error = client.refresh(&first.metadata).await.unwrap_err();
    assert!(matches!(error, ClientError::Parse(_)));
    assert!(!format!("{error:?} {error}").contains("do-not-echo"));
    assert_eq!(
        client.refresh(&first.metadata).await.unwrap_err(),
        ClientError::UnexpectedStatus { status: 503 }
    );
    assert_eq!(first.metadata, original);
    assert_eq!(first.body, FIXTURE);
}

#[tokio::test]
async fn total_timeout_covers_waiting_for_headers_and_body() {
    for delay_body in [false, true] {
        let mut reply = Reply::response(200, &[], FIXTURE);
        if delay_body {
            reply.body_delay = Duration::from_millis(300);
        } else {
            reply.delay = Duration::from_millis(300);
        }
        let mock = Mock::start(vec![reply]).await;
        let client = mock
            .builder()
            .timeout(Duration::from_millis(30))
            .build()
            .unwrap();
        assert_eq!(
            client.fetch().await.unwrap_err(),
            ClientError::Transport {
                kind: TransportKind::Timeout
            }
        );
    }
}

#[tokio::test]
async fn connection_errors_are_sanitized_and_categorized() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/do-not-echo", listener.local_addr().unwrap());
    drop(listener);
    let mut builder = Client::builder().endpoint(endpoint);
    builder.allow_http = true;
    let error = builder.build().unwrap().fetch().await.unwrap_err();
    assert_eq!(
        error,
        ClientError::Transport {
            kind: TransportKind::Connect
        }
    );
    assert!(!format!("{error:?} {error}").contains("do-not-echo"));
    assert!(std::error::Error::source(&error).is_none());
}
