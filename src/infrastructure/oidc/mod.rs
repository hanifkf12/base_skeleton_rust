use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, Jwk, JwkSet, KeyOperations, PublicKeyUse},
};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::{
    application::auth::{
        AccessTokenVerificationError, AccessTokenVerifier, AuthenticatedPrincipal,
    },
    config::OidcConfig,
};

impl From<jsonwebtoken::errors::Error> for AccessTokenVerificationError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        AccessTokenVerificationError::InvalidToken
    }
}

// Discovery and JWKS documents are each limited to 1 MiB, including chunked responses.
const MAX_METADATA_RESPONSE_BYTES: usize = 1_048_576;

type KeyIndex = HashMap<Algorithm, HashMap<String, Arc<DecodingKey>>>;

#[derive(Deserialize)]
struct DiscoveryMetadata {
    issuer: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct AccessTokenClaims {
    sub: String,
    exp: u64,
    iat: u64,
    #[serde(default)]
    scope: String,
}

pub struct OidcAccessTokenVerifier {
    client: reqwest::Client,
    issuer: String,
    audience: String,
    allowed_algorithms: HashSet<Algorithm>,
    jwks_uri: String,
    keys: RwLock<CachedKeys>,
    refresh: Mutex<RefreshState>,
    refresh_interval: Duration,
    jwks_max_age: Duration,
    clock_skew_seconds: u64,
    max_token_lifetime_seconds: u64,
}

struct CachedKeys {
    index: KeyIndex,
    fetched_at: Instant,
}

impl CachedKeys {
    fn key(&self, kid: &str, algorithm: Algorithm) -> Option<Arc<DecodingKey>> {
        self.index.get(&algorithm)?.get(kid).cloned()
    }
}

#[derive(Default)]
struct RefreshState {
    last_attempt: Option<Instant>,
}

impl OidcAccessTokenVerifier {
    pub async fn discover(config: &OidcConfig) -> Result<Arc<Self>> {
        config.validate()?;
        let client = metadata_client(config)?;
        let allowed_algorithms = parse_algorithms(&config.allowed_algorithms)?;
        validate_issuer_url(
            &config.issuer_url,
            config.allow_insecure_http,
            "OIDC_ISSUER_URL",
        )?;
        let configured_issuer = config.issuer_url.trim_end_matches('/');
        let discovery_url = format!("{configured_issuer}/.well-known/openid-configuration");
        let metadata: DiscoveryMetadata = fetch_json(&client, &discovery_url)
            .await
            .context("could not load OIDC discovery metadata")?;

        validate_issuer_url(
            &metadata.issuer,
            config.allow_insecure_http,
            "OIDC discovery issuer",
        )?;
        ensure!(
            metadata.issuer.trim_end_matches('/') == configured_issuer,
            "OIDC discovery issuer does not match OIDC_ISSUER_URL"
        );
        ensure!(
            !metadata.jwks_uri.trim().is_empty(),
            "OIDC discovery metadata contains an empty jwks_uri"
        );
        validate_url_scheme(
            &metadata.jwks_uri,
            config.allow_insecure_http,
            "OIDC jwks_uri",
        )?;

        let keys: JwkSet = fetch_json(&client, &metadata.jwks_uri)
            .await
            .context("could not load initial OIDC JWKS")?;
        let index = index_keys(&keys, &allowed_algorithms)?;

        Ok(Arc::new(Self {
            client,
            issuer: metadata.issuer,
            audience: config.audience.clone(),
            allowed_algorithms,
            jwks_uri: metadata.jwks_uri,
            keys: RwLock::new(CachedKeys {
                index,
                fetched_at: Instant::now(),
            }),
            refresh: Mutex::new(RefreshState::default()),
            refresh_interval: Duration::from_secs(config.jwks_refresh_interval_seconds),
            jwks_max_age: Duration::from_secs(config.jwks_max_age_seconds),
            clock_skew_seconds: config.clock_skew_seconds,
            max_token_lifetime_seconds: config.max_token_lifetime_seconds,
        }))
    }

    async fn key_for(
        &self,
        kid: &str,
        algorithm: Algorithm,
    ) -> Result<Arc<DecodingKey>, AccessTokenVerificationError> {
        {
            let keys = self.keys.read().await;
            if keys.fetched_at.elapsed() < self.jwks_max_age
                && let Some(key) = keys.key(kid, algorithm)
            {
                return Ok(key);
            }
        }

        let mut refresh = self.refresh.lock().await;
        {
            let keys = self.keys.read().await;
            if keys.fetched_at.elapsed() < self.jwks_max_age
                && let Some(key) = keys.key(kid, algorithm)
            {
                return Ok(key);
            }
        }
        let cache_is_stale = self.keys.read().await.fetched_at.elapsed() >= self.jwks_max_age;
        if refresh
            .last_attempt
            .is_some_and(|last_attempt| last_attempt.elapsed() < self.refresh_interval)
        {
            return Err(if cache_is_stale {
                AccessTokenVerificationError::AuthenticationUnavailable
            } else {
                AccessTokenVerificationError::InvalidToken
            });
        }

        refresh.last_attempt = Some(Instant::now());
        let keys = match fetch_json::<JwkSet>(&self.client, &self.jwks_uri).await {
            Ok(keys) => keys,
            Err(error) => {
                tracing::error!(error = %error, "OIDC JWKS refresh failed");
                return Err(AccessTokenVerificationError::AuthenticationUnavailable);
            }
        };
        let index = match index_keys(&keys, &self.allowed_algorithms) {
            Ok(index) => index,
            Err(error) => {
                tracing::error!(error = %error, "OIDC JWKS refresh contains invalid signing keys");
                return Err(AccessTokenVerificationError::AuthenticationUnavailable);
            }
        };

        let key = index
            .get(&algorithm)
            .and_then(|keys| keys.get(kid))
            .cloned();
        *self.keys.write().await = CachedKeys {
            index,
            fetched_at: Instant::now(),
        };
        key.ok_or(AccessTokenVerificationError::InvalidToken)
    }
}

#[async_trait]
impl AccessTokenVerifier for OidcAccessTokenVerifier {
    async fn verify(
        &self,
        access_token: &str,
    ) -> Result<AuthenticatedPrincipal, AccessTokenVerificationError> {
        let header = decode_header(access_token).map_err(AccessTokenVerificationError::from)?;
        if !self.allowed_algorithms.contains(&header.alg) {
            return Err(AccessTokenVerificationError::InvalidToken);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or(AccessTokenVerificationError::InvalidToken)?;
        let decoding_key = self.key_for(kid, header.alg).await?;

        let mut validation = Validation::new(header.alg);
        validation.leeway = self.clock_skew_seconds;
        validation.validate_nbf = true;
        validation.set_audience(&[&self.audience]);
        validation.set_issuer(&[&self.issuer]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);

        let token = decode::<AccessTokenClaims>(access_token, &decoding_key, &validation)
            .map_err(AccessTokenVerificationError::from)?;
        if token.claims.sub.trim().is_empty() {
            return Err(AccessTokenVerificationError::InvalidToken);
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AccessTokenVerificationError::AuthenticationUnavailable)?
            .as_secs();
        if token.claims.iat > now.saturating_add(self.clock_skew_seconds)
            || token.claims.exp < token.claims.iat
            || token.claims.exp - token.claims.iat > self.max_token_lifetime_seconds
        {
            return Err(AccessTokenVerificationError::InvalidToken);
        }
        let scopes = token
            .claims
            .scope
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<HashSet<_>>();

        Ok(AuthenticatedPrincipal::new(token.claims.sub, scopes))
    }
}

fn metadata_client(config: &OidcConfig) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(!config.allow_insecure_http)
        .timeout(Duration::from_secs(config.http_timeout_seconds))
        .build()
        .context("could not construct OIDC HTTP client")
}

async fn fetch_json<T>(client: &reqwest::Client, url: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("OIDC metadata request failed"))?
        .error_for_status()
        .map_err(|_| anyhow::anyhow!("OIDC metadata request returned an unsuccessful status"))?;
    let content_length = response.content_length();
    ensure!(
        content_length.is_none_or(|length| length <= MAX_METADATA_RESPONSE_BYTES as u64),
        "OIDC metadata response exceeds the 1 MiB limit"
    );
    let capacity = content_length
        .map(|length| length as usize)
        .unwrap_or(8_192);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("could not read OIDC metadata response"))?
    {
        ensure!(
            chunk.len() <= MAX_METADATA_RESPONSE_BYTES - bytes.len(),
            "OIDC metadata response exceeds the 1 MiB limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("OIDC metadata response contains invalid JSON"))
}

fn validate_issuer_url(value: &str, allow_insecure_http: bool, field: &str) -> Result<()> {
    let url = parse_url(value, field)?;
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "{field} must not contain a query or fragment"
    );
    validate_url_scheme(value, allow_insecure_http, field)
}

fn validate_url_scheme(value: &str, allow_insecure_http: bool, field: &str) -> Result<()> {
    let url = parse_url(value, field)?;
    let allowed = url.scheme() == "https" || (allow_insecure_http && url.scheme() == "http");
    ensure!(
        allowed,
        "{field} must use https{}",
        if allow_insecure_http { " or http" } else { "" }
    );
    Ok(())
}

fn parse_url(value: &str, field: &str) -> Result<reqwest::Url> {
    reqwest::Url::parse(value).with_context(|| format!("{field} must be a valid URL"))
}

fn parse_algorithms(values: &[String]) -> Result<HashSet<Algorithm>> {
    let mut algorithms = HashSet::with_capacity(values.len());
    for value in values {
        let algorithm = Algorithm::from_str(value).with_context(|| {
            format!("OIDC_ALLOWED_ALGORITHMS contains unsupported value {value}")
        })?;
        if matches!(
            algorithm,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
        ) {
            bail!("OIDC_ALLOWED_ALGORITHMS cannot enable symmetric HMAC algorithms");
        }
        algorithms.insert(algorithm);
    }
    ensure!(
        !algorithms.is_empty(),
        "OIDC_ALLOWED_ALGORITHMS must contain at least one algorithm"
    );
    Ok(algorithms)
}

fn index_keys(keys: &JwkSet, allowed_algorithms: &HashSet<Algorithm>) -> Result<KeyIndex> {
    let mut index = KeyIndex::new();
    for key in &keys.keys {
        let Some(kid) = key.common.key_id.as_deref().filter(|kid| !kid.is_empty()) else {
            continue;
        };
        let mut decoding_key = None;
        for &algorithm in allowed_algorithms {
            if !key_is_usable(key, algorithm) {
                continue;
            }
            // Preserve first-match lookup semantics, including shadowed duplicates.
            if index
                .get(&algorithm)
                .is_some_and(|keys| keys.contains_key(kid))
            {
                continue;
            }
            let decoded = match &decoding_key {
                Some(decoded) => Arc::clone(decoded),
                None => {
                    let decoded = Arc::new(DecodingKey::from_jwk(key).map_err(|_| {
                        anyhow::anyhow!("OIDC JWKS contains an invalid signing key")
                    })?);
                    decoding_key = Some(Arc::clone(&decoded));
                    decoded
                }
            };
            index
                .entry(algorithm)
                .or_default()
                .insert(kid.to_owned(), decoded);
        }
    }
    ensure!(
        !index.is_empty(),
        "OIDC JWKS contains no usable signing key"
    );
    Ok(index)
}

fn key_is_usable(key: &Jwk, algorithm: Algorithm) -> bool {
    let use_allows_verification = key
        .common
        .public_key_use
        .as_ref()
        .is_none_or(|key_use| *key_use == PublicKeyUse::Signature);
    let operations_allow_verification = key
        .common
        .key_operations
        .as_ref()
        .is_none_or(|operations| operations.contains(&KeyOperations::Verify));
    let algorithm_matches = key
        .common
        .key_algorithm
        .is_none_or(|key_algorithm| key_algorithm.to_string() == format!("{algorithm:?}"));
    let key_type_matches = matches!(
        (&key.algorithm, algorithm),
        (
            AlgorithmParameters::RSA(_),
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512
        ) | (
            AlgorithmParameters::EllipticCurve(_),
            Algorithm::ES256 | Algorithm::ES384
        ) | (AlgorithmParameters::OctetKeyPair(_), Algorithm::EdDSA)
    );

    use_allows_verification
        && operations_allow_verification
        && algorithm_matches
        && key_type_matches
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use axum::{
        Json, Router,
        body::Body,
        extract::State,
        http::{Response, header},
        response::IntoResponse,
        routing::get,
    };
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::RwLock, task::JoinHandle};

    use super::*;

    const PRIVATE_KEY: &[u8] = include_bytes!("../../../tests/fixtures/oidc_test_private_key.pem");
    const MODULUS: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";

    #[derive(Clone)]
    struct ProviderState {
        issuer: String,
        jwks: Arc<RwLock<Value>>,
        jwks_requests: Arc<AtomicUsize>,
        discovery_body: Arc<RwLock<Option<MetadataBody>>>,
        jwks_body: Arc<RwLock<Option<MetadataBody>>>,
    }

    struct TestProvider {
        config: OidcConfig,
        state: ProviderState,
        task: JoinHandle<()>,
    }

    #[derive(Clone, Copy)]
    enum MetadataBody {
        OversizedDeclared,
        OversizedChunked,
        InvalidJson,
    }

    impl MetadataBody {
        fn response(self, mut document: Value) -> Response<Body> {
            if matches!(self, Self::InvalidJson) {
                return Response::new(Body::from("{invalid-json"));
            }
            // Without the response bound, this remains valid provider metadata.
            document["padding"] = json!(" ".repeat(MAX_METADATA_RESPONSE_BYTES));
            let bytes = serde_json::to_vec(&document).unwrap();
            let mut response = Response::builder().header(header::CONTENT_TYPE, "application/json");
            let body = match self {
                Self::OversizedDeclared => {
                    response = response.header(header::CONTENT_LENGTH, bytes.len());
                    Body::from(bytes)
                }
                Self::OversizedChunked => Body::from_stream(Body::from(bytes).into_data_stream()),
                Self::InvalidJson => unreachable!(),
            };
            response.body(body).unwrap()
        }
    }

    impl TestProvider {
        async fn start(kid: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let issuer = format!("http://{}", listener.local_addr().unwrap());
            let state = ProviderState {
                issuer: issuer.clone(),
                jwks: Arc::new(RwLock::new(jwks(kid))),
                jwks_requests: Arc::new(AtomicUsize::new(0)),
                discovery_body: Arc::new(RwLock::new(None)),
                jwks_body: Arc::new(RwLock::new(None)),
            };
            let app = Router::new()
                .route("/.well-known/openid-configuration", get(discovery))
                .route("/jwks", get(jwks_response))
                .with_state(state.clone());
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                config: OidcConfig {
                    issuer_url: issuer,
                    audience: "users-api".to_owned(),
                    allowed_algorithms: vec!["RS256".to_owned()],
                    http_timeout_seconds: 5,
                    clock_skew_seconds: 30,
                    jwks_refresh_interval_seconds: 60,
                    jwks_max_age_seconds: 300,
                    max_token_lifetime_seconds: 3_600,
                    allow_insecure_http: true,
                },
                state,
                task,
            }
        }

        async fn rotate_to(&self, kid: &str) {
            *self.state.jwks.write().await = jwks(kid);
        }
    }

    async fn discovery(State(state): State<ProviderState>) -> Response<Body> {
        let document = json!({
            "issuer": state.issuer,
            "jwks_uri": format!("{}/jwks", state.issuer),
        });
        if let Some(body) = *state.discovery_body.read().await {
            return body.response(document);
        }
        Json(document).into_response()
    }

    async fn jwks_response(State(state): State<ProviderState>) -> Response<Body> {
        state.jwks_requests.fetch_add(1, Ordering::SeqCst);
        let document = state.jwks.read().await.clone();
        if let Some(body) = *state.jwks_body.read().await {
            return body.response(document);
        }
        Json(document).into_response()
    }

    fn jwks(kid: &str) -> Value {
        json!({
            "keys": [{
                "kty": "RSA",
                "n": MODULUS,
                "e": "AQAB",
                "kid": kid,
                "alg": "RS256",
                "use": "sig"
            }]
        })
    }

    fn valid_claims(issuer: &str) -> Value {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        json!({
            "sub": "operator-123",
            "scope": "users:read users:write",
            "iss": issuer,
            "aud": "users-api",
            "exp": now + 300,
            "iat": now,
            "nbf": now - 1
        })
    }

    fn sign(kid: &str, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_owned());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(PRIVATE_KEY).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn validates_signature_registered_claims_subject_and_scopes() {
        let provider = TestProvider::start("initial").await;
        let verifier = OidcAccessTokenVerifier::discover(&provider.config)
            .await
            .unwrap();
        let claims = valid_claims(&provider.config.issuer_url);
        let valid_token = sign("initial", &claims);

        let principal = verifier.verify(&valid_token).await.unwrap();
        assert_eq!(principal.subject(), "operator-123");
        assert!(principal.has_scope("users:read"));
        assert!(principal.has_scope("users:write"));

        let mut invalid_tokens = Vec::new();
        let mut invalid_issuer = claims.clone();
        invalid_issuer["iss"] = json!("https://different-issuer.example");
        invalid_tokens.push(sign("initial", &invalid_issuer));
        let mut invalid_audience = claims.clone();
        invalid_audience["aud"] = json!("different-api");
        invalid_tokens.push(sign("initial", &invalid_audience));
        let mut expired = claims.clone();
        expired["exp"] = json!(1);
        invalid_tokens.push(sign("initial", &expired));
        let mut not_yet_valid = claims.clone();
        not_yet_valid["nbf"] = json!(u64::MAX - 1);
        invalid_tokens.push(sign("initial", &not_yet_valid));
        let mut missing_subject = claims.clone();
        missing_subject.as_object_mut().unwrap().remove("sub");
        invalid_tokens.push(sign("initial", &missing_subject));
        let mut empty_subject = claims.clone();
        empty_subject["sub"] = json!("  ");
        invalid_tokens.push(sign("initial", &empty_subject));
        let mut malformed_scope = claims.clone();
        malformed_scope["scope"] = json!(["users:read"]);
        invalid_tokens.push(sign("initial", &malformed_scope));
        let mut malformed_expiration = claims.clone();
        malformed_expiration["exp"] = json!("tomorrow");
        invalid_tokens.push(sign("initial", &malformed_expiration));
        let mut missing_issued_at = claims.clone();
        missing_issued_at.as_object_mut().unwrap().remove("iat");
        invalid_tokens.push(sign("initial", &missing_issued_at));
        let mut future_issued_at = claims.clone();
        future_issued_at["iat"] = json!(u64::MAX - 1);
        invalid_tokens.push(sign("initial", &future_issued_at));
        let mut excessive_lifetime = claims.clone();
        excessive_lifetime["exp"] = json!(claims["iat"].as_u64().unwrap() + 3_601);
        invalid_tokens.push(sign("initial", &excessive_lifetime));

        let mut bad_signature = valid_token;
        let signature_offset = bad_signature.rfind('.').unwrap() + 1;
        let replacement = if &bad_signature[signature_offset..=signature_offset] == "A" {
            "B"
        } else {
            "A"
        };
        bad_signature.replace_range(signature_offset..=signature_offset, replacement);
        invalid_tokens.push(bad_signature);

        let mut hs_header = Header::new(Algorithm::HS256);
        hs_header.kid = Some("initial".to_owned());
        invalid_tokens
            .push(encode(&hs_header, &claims, &EncodingKey::from_secret(b"secret")).unwrap());

        for token in invalid_tokens {
            assert_eq!(
                verifier.verify(&token).await,
                Err(AccessTokenVerificationError::InvalidToken)
            );
        }
    }

    #[tokio::test]
    async fn accepts_an_rsa_token_when_ecdsa_is_also_allowed() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.allowed_algorithms.push("ES256".to_owned());
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let claims = valid_claims(&config.issuer_url);

        assert!(verifier.verify(&sign("initial", &claims)).await.is_ok());
    }

    #[tokio::test]
    async fn caches_keys_and_refreshes_once_for_an_unknown_kid() {
        let provider = TestProvider::start("initial").await;
        let verifier = OidcAccessTokenVerifier::discover(&provider.config)
            .await
            .unwrap();
        let claims = valid_claims(&provider.config.issuer_url);

        verifier.verify(&sign("initial", &claims)).await.unwrap();
        verifier.verify(&sign("initial", &claims)).await.unwrap();
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 1);

        provider.rotate_to("rotated").await;
        verifier.verify(&sign("rotated", &claims)).await.unwrap();
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 2);
        verifier.verify(&sign("rotated", &claims)).await.unwrap();

        assert_eq!(
            verifier.verify(&sign("unknown", &claims)).await,
            Err(AccessTokenVerificationError::InvalidToken)
        );
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stale_same_kid_refresh_is_single_flight() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.jwks_max_age_seconds = 1;
        config.jwks_refresh_interval_seconds = 1;
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let token = sign("initial", &valid_claims(&config.issuer_url));
        verifier.keys.write().await.fetched_at = Instant::now() - Duration::from_secs(2);

        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let verifier = verifier.clone();
            let token = token.clone();
            tasks.spawn(async move { verifier.verify(&token).await });
        }
        while let Some(result) = tasks.join_next().await {
            assert!(result.unwrap().is_ok());
        }
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn reports_unavailable_when_required_runtime_refresh_fails() {
        let provider = TestProvider::start("initial").await;
        let verifier = OidcAccessTokenVerifier::discover(&provider.config)
            .await
            .unwrap();
        let claims = valid_claims(&provider.config.issuer_url);
        provider.task.abort();
        let _ = provider.task.await;

        assert_eq!(
            verifier.verify(&sign("rotated", &claims)).await,
            Err(AccessTokenVerificationError::AuthenticationUnavailable)
        );
        verifier.verify(&sign("initial", &claims)).await.unwrap();
    }

    #[tokio::test]
    async fn fails_closed_when_cached_jwks_is_stale_and_refresh_fails() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.jwks_max_age_seconds = 1;
        config.jwks_refresh_interval_seconds = 1;
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let claims = valid_claims(&config.issuer_url);
        verifier.keys.write().await.fetched_at = Instant::now() - Duration::from_secs(2);
        provider.task.abort();
        let _ = provider.task.await;

        assert_eq!(
            verifier.verify(&sign("initial", &claims)).await,
            Err(AccessTokenVerificationError::AuthenticationUnavailable)
        );
        assert_eq!(
            verifier.verify(&sign("initial", &claims)).await,
            Err(AccessTokenVerificationError::AuthenticationUnavailable)
        );
    }

    #[tokio::test]
    async fn rejects_http_issuer_when_insecure_http_is_disabled() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.allow_insecure_http = false;

        assert!(OidcAccessTokenVerifier::discover(&config).await.is_err());
    }

    #[tokio::test]
    async fn accepts_configured_issuer_with_trailing_slash() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.issuer_url = format!("{}/", config.issuer_url);
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let claims = valid_claims(&provider.config.issuer_url);

        verifier.verify(&sign("initial", &claims)).await.unwrap();
    }

    #[tokio::test]
    async fn initial_provider_failure_prevents_verifier_startup() {
        let provider = TestProvider::start("initial").await;
        let config = provider.config.clone();
        provider.task.abort();
        let _ = provider.task.await;

        assert!(OidcAccessTokenVerifier::discover(&config).await.is_err());
    }

    #[tokio::test]
    async fn rejects_invalid_jwks_timers_before_fetching_metadata() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.jwks_max_age_seconds = config.jwks_refresh_interval_seconds - 1;
        assert!(OidcAccessTokenVerifier::discover(&config).await.is_err());
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejects_oversized_discovery_responses_with_declared_or_chunked_bodies() {
        for body in [
            MetadataBody::OversizedDeclared,
            MetadataBody::OversizedChunked,
        ] {
            let provider = TestProvider::start("initial").await;
            *provider.state.discovery_body.write().await = Some(body);
            assert!(
                OidcAccessTokenVerifier::discover(&provider.config)
                    .await
                    .is_err()
            );
            assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 0);
            provider.task.abort();
        }
    }

    #[tokio::test]
    async fn rejects_oversized_initial_jwks_with_declared_or_chunked_bodies() {
        for body in [
            MetadataBody::OversizedDeclared,
            MetadataBody::OversizedChunked,
        ] {
            let provider = TestProvider::start("initial").await;
            *provider.state.jwks_body.write().await = Some(body);
            assert!(
                OidcAccessTokenVerifier::discover(&provider.config)
                    .await
                    .is_err()
            );
            assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 1);
            provider.task.abort();
        }
    }

    #[tokio::test]
    async fn failed_metadata_refresh_preserves_fresh_keys_but_fails_closed_when_stale() {
        for body in [
            MetadataBody::OversizedDeclared,
            MetadataBody::OversizedChunked,
            MetadataBody::InvalidJson,
        ] {
            let provider = TestProvider::start("initial").await;
            let verifier = OidcAccessTokenVerifier::discover(&provider.config)
                .await
                .unwrap();
            let claims = valid_claims(&provider.config.issuer_url);
            *provider.state.jwks_body.write().await = Some(body);
            assert_eq!(
                verifier.verify(&sign("unknown", &claims)).await,
                Err(AccessTokenVerificationError::AuthenticationUnavailable)
            );
            verifier.verify(&sign("initial", &claims)).await.unwrap();
            verifier.keys.write().await.fetched_at =
                Instant::now() - Duration::from_secs(provider.config.jwks_max_age_seconds + 1);
            assert_eq!(
                verifier.verify(&sign("initial", &claims)).await,
                Err(AccessTokenVerificationError::AuthenticationUnavailable)
            );
            assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 2);
            verifier.refresh.lock().await.last_attempt = None;
            assert_eq!(
                verifier.verify(&sign("initial", &claims)).await,
                Err(AccessTokenVerificationError::AuthenticationUnavailable)
            );
            assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 3);
            provider.task.abort();
        }
    }

    #[tokio::test]
    async fn production_metadata_client_rejects_plaintext_before_sending() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.allow_insecure_http = false;
        let client = metadata_client(&config).unwrap();
        let error = client
            .get(format!("{}/jwks", config.issuer_url))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_builder());
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejects_malformed_signing_keys_at_startup_and_refresh() {
        let provider = TestProvider::start("initial").await;
        let verifier = OidcAccessTokenVerifier::discover(&provider.config)
            .await
            .unwrap();
        let mut malformed = jwks("initial");
        malformed["keys"][0]["n"] = json!("not valid base64!");
        *provider.state.jwks.write().await = malformed;
        assert!(
            OidcAccessTokenVerifier::discover(&provider.config)
                .await
                .is_err()
        );
        assert_eq!(
            verifier
                .verify(&sign("unknown", &valid_claims(&provider.config.issuer_url)))
                .await,
            Err(AccessTokenVerificationError::AuthenticationUnavailable)
        );
        verifier
            .verify(&sign("initial", &valid_claims(&provider.config.issuer_url)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_jwks_without_an_allowed_verification_key() {
        for rejected_key in [
            json!({"kty": "RSA", "n": MODULUS, "e": "AQAB", "kid": "initial", "alg": "RS256", "use": "enc"}),
            json!({"kty": "RSA", "n": MODULUS, "e": "AQAB", "kid": "initial", "alg": "RS256", "key_ops": ["sign"]}),
            json!({"kty": "RSA", "n": MODULUS, "e": "AQAB", "kid": "initial", "alg": "RS384"}),
            json!({"kty": "oct", "k": "c2VjcmV0", "kid": "initial", "alg": "RS256"}),
            json!({"kty": "RSA", "n": MODULUS, "e": "AQAB", "kid": "", "alg": "RS256"}),
        ] {
            let provider = TestProvider::start("initial").await;
            *provider.state.jwks.write().await = json!({"keys": [rejected_key]});
            assert!(
                OidcAccessTokenVerifier::discover(&provider.config)
                    .await
                    .is_err()
            );
            provider.task.abort();
        }
    }

    #[tokio::test]
    async fn accepts_allowed_rsa_algorithms_when_jwk_omits_alg() {
        let provider = TestProvider::start("initial").await;
        let mut keys = jwks("initial");
        keys["keys"][0].as_object_mut().unwrap().remove("alg");
        *provider.state.jwks.write().await = keys;
        let mut config = provider.config.clone();
        config.allowed_algorithms.push("RS384".to_owned());
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let claims = valid_claims(&config.issuer_url);
        verifier.verify(&sign("initial", &claims)).await.unwrap();
        let mut header = Header::new(Algorithm::RS384);
        header.kid = Some("initial".to_owned());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(PRIVATE_KEY).unwrap(),
        )
        .unwrap();
        verifier.verify(&token).await.unwrap();
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn preserves_first_usable_key_for_duplicate_kid_and_algorithm() {
        let provider = TestProvider::start("initial").await;
        let original = jwks("initial")["keys"][0].clone();
        let mut unusable = original.clone();
        unusable["key_ops"] = json!(["sign"]);
        let mut later = original.clone();
        later["n"] = json!("not valid base64!");
        *provider.state.jwks.write().await = json!({"keys": [unusable, original, later]});
        let verifier = OidcAccessTokenVerifier::discover(&provider.config)
            .await
            .unwrap();
        verifier
            .verify(&sign("initial", &valid_claims(&provider.config.issuer_url)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_symmetric_hmac_in_the_configured_allow_list() {
        let provider = TestProvider::start("initial").await;
        let mut config = provider.config.clone();
        config.allowed_algorithms.push("HS256".to_owned());
        assert!(OidcAccessTokenVerifier::discover(&config).await.is_err());
        assert_eq!(provider.state.jwks_requests.load(Ordering::SeqCst), 0);
    }
}
