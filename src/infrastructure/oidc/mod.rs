use std::{
    collections::HashSet,
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

impl From<reqwest::Error> for AccessTokenVerificationError {
    fn from(_: reqwest::Error) -> Self {
        AccessTokenVerificationError::AuthenticationUnavailable
    }
}

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
    allowed_algorithms: Vec<Algorithm>,
    jwks_uri: String,
    keys: RwLock<CachedKeys>,
    refresh: Mutex<RefreshState>,
    refresh_interval: Duration,
    jwks_max_age: Duration,
    clock_skew_seconds: u64,
    max_token_lifetime_seconds: u64,
}

struct CachedKeys {
    set: JwkSet,
    fetched_at: Instant,
}

#[derive(Default)]
struct RefreshState {
    last_attempt: Option<Instant>,
}

impl OidcAccessTokenVerifier {
    pub async fn discover(config: &OidcConfig) -> Result<Arc<Self>> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.http_timeout_seconds))
            .build()
            .context("could not construct OIDC HTTP client")?;
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
        ensure_usable_keys(&keys, &allowed_algorithms)?;

        Ok(Arc::new(Self {
            client,
            issuer: metadata.issuer,
            audience: config.audience.clone(),
            allowed_algorithms,
            jwks_uri: metadata.jwks_uri,
            keys: RwLock::new(CachedKeys {
                set: keys,
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
    ) -> Result<Jwk, AccessTokenVerificationError> {
        {
            let keys = self.keys.read().await;
            if keys.fetched_at.elapsed() < self.jwks_max_age
                && let Some(key) = find_key(&keys.set, kid, algorithm)
            {
                return Ok(key);
            }
        }

        let mut refresh = self.refresh.lock().await;
        {
            let keys = self.keys.read().await;
            if keys.fetched_at.elapsed() < self.jwks_max_age
                && let Some(key) = find_key(&keys.set, kid, algorithm)
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
            Ok(keys) if ensure_usable_keys(&keys, &self.allowed_algorithms).is_ok() => keys,
            Ok(_) => {
                tracing::error!("OIDC JWKS refresh returned no usable signing keys");
                return Err(AccessTokenVerificationError::AuthenticationUnavailable);
            }
            Err(error) => {
                tracing::error!(error = %error, "OIDC JWKS refresh failed");
                return Err(AccessTokenVerificationError::AuthenticationUnavailable);
            }
        };

        let key = find_key(&keys, kid, algorithm);
        *self.keys.write().await = CachedKeys {
            set: keys,
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
        let header =
            decode_header(access_token).map_err(AccessTokenVerificationError::from)?;
        if !self.allowed_algorithms.contains(&header.alg) {
            return Err(AccessTokenVerificationError::InvalidToken);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or(AccessTokenVerificationError::InvalidToken)?;
        let jwk = self.key_for(kid, header.alg).await?;
        let decoding_key =
            DecodingKey::from_jwk(&jwk).map_err(AccessTokenVerificationError::from)?;

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

async fn fetch_json<T>(client: &reqwest::Client, url: &str) -> reqwest::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
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

fn parse_algorithms(values: &[String]) -> Result<Vec<Algorithm>> {
    let mut algorithms = Vec::with_capacity(values.len());
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
        if !algorithms.contains(&algorithm) {
            algorithms.push(algorithm);
        }
    }
    ensure!(
        !algorithms.is_empty(),
        "OIDC_ALLOWED_ALGORITHMS must contain at least one algorithm"
    );
    Ok(algorithms)
}

fn ensure_usable_keys(keys: &JwkSet, allowed_algorithms: &[Algorithm]) -> Result<()> {
    ensure!(
        keys.keys.iter().any(|key| {
            key.common
                .key_id
                .as_deref()
                .is_some_and(|kid| !kid.is_empty())
                && allowed_algorithms
                    .iter()
                    .any(|algorithm| key_is_usable(key, *algorithm))
        }),
        "OIDC JWKS contains no usable signing key"
    );
    Ok(())
}

fn find_key(keys: &JwkSet, kid: &str, algorithm: Algorithm) -> Option<Jwk> {
    keys.keys
        .iter()
        .find(|key| key.common.key_id.as_deref() == Some(kid) && key_is_usable(key, algorithm))
        .cloned()
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

    use axum::{Json, Router, extract::State, routing::get};
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
    }

    struct TestProvider {
        config: OidcConfig,
        state: ProviderState,
        task: JoinHandle<()>,
    }

    impl TestProvider {
        async fn start(kid: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let issuer = format!("http://{}", listener.local_addr().unwrap());
            let state = ProviderState {
                issuer: issuer.clone(),
                jwks: Arc::new(RwLock::new(jwks(kid))),
                jwks_requests: Arc::new(AtomicUsize::new(0)),
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

    async fn discovery(State(state): State<ProviderState>) -> Json<Value> {
        Json(json!({
            "issuer": state.issuer,
            "jwks_uri": format!("{}/jwks", state.issuer),
        }))
    }

    async fn jwks_response(State(state): State<ProviderState>) -> Json<Value> {
        state.jwks_requests.fetch_add(1, Ordering::SeqCst);
        Json(state.jwks.read().await.clone())
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
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let token = sign("initial", &valid_claims(&config.issuer_url));
        tokio::time::sleep(Duration::from_millis(1_050)).await;

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
        config.jwks_refresh_interval_seconds = 60;
        let verifier = OidcAccessTokenVerifier::discover(&config).await.unwrap();
        let claims = valid_claims(&config.issuer_url);
        tokio::time::sleep(Duration::from_millis(1_050)).await;
        provider.task.abort();
        let _ = provider.task.await;

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
}
