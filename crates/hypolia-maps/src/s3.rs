use std::fmt::{self, Write as _};

use chrono::{DateTime, Utc};
use reqwest::Client;
use reqwest::header::{CONTENT_TYPE, HOST, HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::domain::{Bundle, Game, MapId, MapName, complete_maps};
use crate::store::{MapStore, StoreError};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Error)]
pub enum S3Error {
    #[error("URL endpoint S3 invalide ({0}) : {1}")]
    InvalidEndpoint(String, #[source] url::ParseError),
    #[error("requête S3 échouée : {0}")]
    Http(#[from] reqwest::Error),
    #[error("erreur S3 ({status}) sur {action} : {body}")]
    Remote {
        action: String,
        status: u16,
        body: String,
    },
    #[error("valeur d'en-tête HTTP invalide : {0}")]
    InvalidHeader(String),
}

impl From<S3Error> for StoreError {
    fn from(err: S3Error) -> Self {
        StoreError::Backend(Box::new(err))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub path_style: bool,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"***")
            .field("path_style", &self.path_style)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct S3MapStore {
    config: S3Config,
    base_url: Url,
    http: Client,
}

impl S3MapStore {
    pub fn new(mut config: S3Config) -> Result<Self, S3Error> {
        config.endpoint = config.endpoint.trim_end_matches('/').to_owned();
        let base_url = Url::parse(&config.endpoint)
            .map_err(|err| S3Error::InvalidEndpoint(config.endpoint.clone(), err))?;
        let http = Client::builder().build()?;
        Ok(Self {
            config,
            base_url,
            http,
        })
    }

    pub fn config(&self) -> &S3Config {
        &self.config
    }

    pub async fn ensure_bucket(&self) -> Result<(), S3Error> {
        let resp = self.send_signed("PUT", "", &[], Vec::new(), None).await?;
        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        if body.contains("BucketAlreadyOwnedByYou") || body.contains("BucketAlreadyExists") {
            return Ok(());
        }
        Err(S3Error::Remote {
            action: format!("CreateBucket({})", self.config.bucket),
            status: status.as_u16(),
            body,
        })
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, S3Error> {
        let mut keys = Vec::new();
        let mut continuation_token: Option<String> = None;
        loop {
            let mut query = vec![
                ("list-type".to_owned(), "2".to_owned()),
                ("prefix".to_owned(), prefix.to_owned()),
            ];
            if let Some(token) = &continuation_token {
                query.push(("continuation-token".to_owned(), token.clone()));
            }
            let resp = self
                .send_signed("GET", "", &query, Vec::new(), None)
                .await?;
            let status = resp.status();
            let body = resp.text().await?;
            if status.as_u16() == 404 && body.contains("NoSuchBucket") {
                return Ok(Vec::new());
            }
            if !status.is_success() {
                return Err(S3Error::Remote {
                    action: "ListObjectsV2".to_owned(),
                    status: status.as_u16(),
                    body,
                });
            }
            let page = parse_list_objects_v2(&body);
            keys.extend(page.keys);
            continuation_token = page.next_continuation_token.filter(|_| page.is_truncated);
            if continuation_token.is_none() {
                return Ok(keys);
            }
        }
    }

    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>, S3Error> {
        let resp = self.send_signed("GET", key, &[], Vec::new(), None).await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(S3Error::Remote {
                action: format!("GetObject({key})"),
                status: status.as_u16(),
                body,
            });
        }
        Ok(Some(resp.bytes().await?.to_vec()))
    }

    async fn put_object(&self, key: &str, body: &[u8], content_type: &str) -> Result<(), S3Error> {
        let resp = self
            .send_signed("PUT", key, &[], body.to_vec(), Some(content_type))
            .await?;
        expect_success(resp, format!("PutObject({key})")).await
    }

    async fn delete_object(&self, key: &str) -> Result<(), S3Error> {
        let resp = self
            .send_signed("DELETE", key, &[], Vec::new(), None)
            .await?;
        expect_success(resp, format!("DeleteObject({key})")).await
    }

    async fn send_signed(
        &self,
        method: &str,
        key: &str,
        query: &[(String, String)],
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, S3Error> {
        let signed = build_signed_request(
            &self.config,
            &self.base_url,
            method,
            key,
            query,
            &body,
            Utc::now(),
        )?;
        let mut headers = HeaderMap::new();
        for (name, value) in &signed.headers {
            let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|err| S3Error::InvalidHeader(err.to_string()))?;
            let header_val = HeaderValue::from_str(value)
                .map_err(|err| S3Error::InvalidHeader(err.to_string()))?;
            headers.insert(header_name, header_val);
        }
        if let Some(ct) = content_type {
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_str(ct).map_err(|err| S3Error::InvalidHeader(err.to_string()))?,
            );
        }
        let req_method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|err| S3Error::InvalidHeader(err.to_string()))?;
        let mut req = self.http.request(req_method, signed.url).headers(headers);
        if !body.is_empty() {
            req = req.body(body);
        }
        Ok(req.send().await?)
    }
}

async fn expect_success(resp: reqwest::Response, action: String) -> Result<(), S3Error> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    Err(S3Error::Remote {
        action,
        status: status.as_u16(),
        body,
    })
}

impl MapStore for S3MapStore {
    async fn list(&self, game: Game) -> Result<Vec<MapName>, StoreError> {
        let keys = self.list_keys(&game.prefix()).await?;
        Ok(complete_maps(keys.iter().map(String::as_str), game))
    }

    async fn put(&self, id: &MapId, bundle: &Bundle) -> Result<(), StoreError> {
        self.ensure_bucket().await?;
        self.put_object(&id.world_key(), bundle.world_zip(), "application/zip")
            .await?;
        self.put_object(&id.arena_key(), bundle.arena_json(), "application/json")
            .await?;
        Ok(())
    }

    async fn get(&self, id: &MapId) -> Result<Bundle, StoreError> {
        let (Some(world), Some(arena)) = (
            self.get_object(&id.world_key()).await?,
            self.get_object(&id.arena_key()).await?,
        ) else {
            return Err(StoreError::NotFound(id.clone()));
        };
        Bundle::new(world, arena).map_err(|source| StoreError::Corrupt {
            id: id.clone(),
            source,
        })
    }

    async fn delete(&self, id: &MapId) -> Result<(), StoreError> {
        self.delete_object(&id.arena_key()).await?;
        self.delete_object(&id.world_key()).await?;
        Ok(())
    }
}

#[derive(Debug)]
struct SignedRequest {
    url: Url,
    headers: Vec<(String, String)>,
}

fn build_signed_request(
    config: &S3Config,
    base_url: &Url,
    method: &str,
    key: &str,
    query: &[(String, String)],
    body: &[u8],
    now: DateTime<Utc>,
) -> Result<SignedRequest, S3Error> {
    let mut url = base_url.clone();
    let base_host = url
        .host_str()
        .ok_or_else(|| S3Error::InvalidHeader("endpoint sans hôte".to_owned()))?
        .to_owned();

    let clean_key = key.trim_start_matches('/');
    let base_path = url.path().trim_end_matches('/').to_owned();

    let (host_header, canonical_uri) = if config.path_style {
        let host = match url.port() {
            Some(port) => format!("{base_host}:{port}"),
            None => base_host,
        };
        let raw_path = if clean_key.is_empty() {
            format!("{base_path}/{}", config.bucket)
        } else {
            format!("{base_path}/{}/{}", config.bucket, clean_key)
        };
        (host, aws_uri_encode(&raw_path, false))
    } else {
        let virtual_host = format!("{}.{}", config.bucket, base_host);
        url.set_host(Some(&virtual_host))
            .map_err(|err| S3Error::InvalidHeader(err.to_string()))?;
        let host = match url.port() {
            Some(port) => format!("{virtual_host}:{port}"),
            None => virtual_host,
        };
        let raw_path = if clean_key.is_empty() {
            if base_path.is_empty() {
                "/".to_owned()
            } else {
                format!("{base_path}/")
            }
        } else {
            format!("{base_path}/{clean_key}")
        };
        (host, aws_uri_encode(&raw_path, false))
    };

    url.set_path(&canonical_uri);

    let mut encoded_query: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (aws_uri_encode(k, true), aws_uri_encode(v, true)))
        .collect();
    encoded_query.sort();
    let canonical_query = encoded_query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    if canonical_query.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&canonical_query));
    }

    let payload_hash = if body.is_empty() {
        EMPTY_SHA256.to_owned()
    } else {
        sha256_hex(body)
    };
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = now.format("%Y%m%d").to_string();

    let canonical_headers =
        format!("host:{host_header}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let credential_scope = format!("{date_stamp}/{}/s3/aws4_request", config.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac_sha256(
        format!("AWS4{}", config.secret_access_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, config.region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex_lower(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        config.access_key_id
    );

    Ok(SignedRequest {
        url,
        headers: vec![
            (HOST.as_str().to_owned(), host_header),
            ("x-amz-content-sha256".to_owned(), payload_hash),
            ("x-amz-date".to_owned(), amz_date),
            ("authorization".to_owned(), authorization),
        ],
    })
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex_lower(&digest)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK_LEN: usize = 64;
    let mut k_pad = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        let hashed = Sha256::digest(key);
        k_pad[..32].copy_from_slice(&hashed);
    } else {
        k_pad[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_LEN];
    let mut opad = [0x5cu8; BLOCK_LEN];
    for i in 0..BLOCK_LEN {
        ipad[i] ^= k_pad[i];
        opad[i] ^= k_pad[i];
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn aws_uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let is_unreserved = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (!encode_slash && byte == b'/');
        if is_unreserved {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct ListObjectsPage {
    keys: Vec<String>,
    is_truncated: bool,
    next_continuation_token: Option<String>,
}

fn parse_list_objects_v2(xml: &str) -> ListObjectsPage {
    let mut keys = Vec::new();
    let mut cursor = xml;
    while let Some(start) = cursor.find("<Contents>") {
        let after_start = &cursor[start + "<Contents>".len()..];
        let Some(end) = after_start.find("</Contents>") else {
            break;
        };
        let block = &after_start[..end];
        if let Some(key) = extract_xml_tag(block, "Key") {
            keys.push(unescape_xml(&key));
        }
        cursor = &after_start[end + "</Contents>".len()..];
    }

    let is_truncated =
        extract_xml_tag(xml, "IsTruncated").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    let next_continuation_token =
        extract_xml_tag(xml, "NextContinuationToken").map(|v| unescape_xml(&v));

    ListObjectsPage {
        keys,
        is_truncated,
        next_continuation_token,
    }
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)?;
    Some(xml[start..start + end].to_owned())
}

fn unescape_xml(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::{Path, Query, State};
    use axum::http::{Method, StatusCode};
    use axum::routing::any;
    use chrono::TimeZone;

    use super::*;
    use crate::domain::tests::valid_bundle;

    #[test]
    fn parses_list_objects_v2_xml_with_pagination_and_entities() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>tok&amp;1</NextContinuationToken>
  <Contents><Key>towerwars/a&amp;b/world.zip</Key></Contents>
  <Contents><Key>towerwars/steampunk/arena.json</Key></Contents>
</ListBucketResult>"#;
        let page = parse_list_objects_v2(xml);
        assert!(page.is_truncated);
        assert_eq!(page.next_continuation_token.as_deref(), Some("tok&1"));
        assert_eq!(
            page.keys,
            ["towerwars/a&b/world.zip", "towerwars/steampunk/arena.json"]
        );
    }

    #[test]
    fn sigv4_produces_deterministic_path_style_signature() {
        let cfg = S3Config {
            endpoint: "http://hypolia-rustfs:9000".to_owned(),
            bucket: "hypolia-maps".to_owned(),
            region: "us-east-1".to_owned(),
            access_key_id: "AKIAEXAMPLE".to_owned(),
            secret_access_key: "secret-key".to_owned(),
            path_style: true,
        };
        let base = Url::parse(&cfg.endpoint).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        let signed = build_signed_request(
            &cfg,
            &base,
            "GET",
            "towerwars/imperial/arena.json",
            &[],
            &[],
            now,
        )
        .unwrap();
        assert_eq!(
            signed.url.as_str(),
            "http://hypolia-rustfs:9000/hypolia-maps/towerwars/imperial/arena.json"
        );
        let auth = signed
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert!(auth.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20261120/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="
        ));
    }

    #[test]
    fn config_debug_hides_secret() {
        let cfg = S3Config {
            endpoint: "http://x".to_owned(),
            bucket: "b".to_owned(),
            region: "r".to_owned(),
            access_key_id: "id".to_owned(),
            secret_access_key: "super-secret".to_owned(),
            path_style: true,
        };
        assert!(!format!("{cfg:?}").contains("super-secret"));
    }

    #[derive(Default)]
    struct MockS3 {
        bucket_exists: bool,
        objects: BTreeMap<String, Vec<u8>>,
        log: Vec<String>,
    }

    type Shared = Arc<Mutex<MockS3>>;

    async fn bucket_handler(
        State(s3): State<Shared>,
        method: Method,
        Query(q): Query<HashMap<String, String>>,
    ) -> (StatusCode, String) {
        let mut s3 = s3.lock().unwrap();
        if method == Method::PUT {
            s3.bucket_exists = true;
            return (StatusCode::OK, String::new());
        }
        if !s3.bucket_exists {
            return (
                StatusCode::NOT_FOUND,
                "<Error><Code>NoSuchBucket</Code></Error>".to_owned(),
            );
        }
        let prefix = q.get("prefix").cloned().unwrap_or_default();
        let contents: String = s3
            .objects
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .map(|k| format!("<Contents><Key>{k}</Key></Contents>"))
            .collect();
        (
            StatusCode::OK,
            format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
            ),
        )
    }

    async fn object_handler(
        State(s3): State<Shared>,
        method: Method,
        Path(key): Path<String>,
        body: Bytes,
    ) -> (StatusCode, Vec<u8>) {
        let mut s3 = s3.lock().unwrap();
        s3.log.push(format!("{method} {key}"));
        match method {
            Method::PUT => {
                s3.objects.insert(key, body.to_vec());
                (StatusCode::OK, Vec::new())
            }
            Method::GET => match s3.objects.get(&key) {
                Some(bytes) => (StatusCode::OK, bytes.clone()),
                None => (StatusCode::NOT_FOUND, b"<Error>NoSuchKey</Error>".to_vec()),
            },
            Method::DELETE => {
                s3.objects.remove(&key);
                (StatusCode::NO_CONTENT, Vec::new())
            }
            _ => (StatusCode::METHOD_NOT_ALLOWED, Vec::new()),
        }
    }

    async fn spawn_mock() -> (S3MapStore, Shared) {
        let shared = Shared::default();
        let app = Router::new()
            .route("/hypolia-maps", any(bucket_handler))
            .route("/hypolia-maps/{*key}", any(object_handler))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = S3MapStore::new(S3Config {
            endpoint: format!("http://{addr}/"),
            bucket: "hypolia-maps".to_owned(),
            region: "us-east-1".to_owned(),
            access_key_id: "test".to_owned(),
            secret_access_key: "secret".to_owned(),
            path_style: true,
        })
        .unwrap();
        (store, shared)
    }

    #[tokio::test]
    async fn round_trip_put_list_get_delete() {
        let (store, shared) = spawn_mock().await;
        let id = MapId::new(Game::Towerwars, "imperial".parse().unwrap());
        let bundle = valid_bundle();

        assert!(store.list(Game::Towerwars).await.unwrap().is_empty());
        store.put(&id, &bundle).await.unwrap();
        assert_eq!(
            shared.lock().unwrap().log,
            [
                "PUT towerwars/imperial/world.zip",
                "PUT towerwars/imperial/arena.json"
            ]
        );

        assert_eq!(
            store.list(Game::Towerwars).await.unwrap(),
            std::slice::from_ref(&id.name)
        );
        assert!(store.list(Game::Sheepwars).await.unwrap().is_empty());
        assert_eq!(store.get(&id).await.unwrap(), bundle);

        store.delete(&id).await.unwrap();
        assert!(store.list(Game::Towerwars).await.unwrap().is_empty());
        assert!(matches!(store.get(&id).await, Err(StoreError::NotFound(_))));
    }

    #[tokio::test]
    async fn get_reports_corrupt_map() {
        let (store, shared) = spawn_mock().await;
        let id = MapId::new(Game::Sheepwars, "broken".parse().unwrap());
        {
            let mut s3 = shared.lock().unwrap();
            s3.bucket_exists = true;
            s3.objects.insert(id.world_key(), b"not a zip".to_vec());
            s3.objects.insert(id.arena_key(), b"{}".to_vec());
        }
        assert!(matches!(
            store.get(&id).await,
            Err(StoreError::Corrupt { .. })
        ));
    }
}
