//! OpenID Connect login (authorization code flow with PKCE) against
//! Pocket-ID or any other standard provider.
//!
//! - Discovery (`/.well-known/openid-configuration`) and JWKS are fetched
//!   lazily and cached; an unknown key ID triggers one refresh (rotation).
//! - Pending logins (state → nonce, PKCE verifier, return path) live in
//!   memory for [`LOGIN_TTL`]; the browser is bound to its login by a
//!   short-lived cookie holding the state.
//! - The ID token's signature, issuer, audience, expiry and nonce are
//!   verified; only the configured person may log in.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use url::Url;

use crate::app::Identity;
use crate::auth::{pkce_challenge, random_secret};
use crate::config::OidcConfig;

/// How long a started login may take.
pub const LOGIN_TTL: Duration = Duration::from_secs(10 * 60);
/// Cookie binding the browser to its pending login.
pub const LOGIN_COOKIE: &str = "zunder_login";
/// Discovery and JWKS are refreshed after this age.
const PROVIDER_MAX_AGE: Duration = Duration::from_secs(60 * 60);
/// Upper bound for pending logins (unauthenticated endpoint).
const MAX_PENDING: usize = 256;
/// Accepted clock skew for `exp`/`iat`/`nbf`.
const LEEWAY_SECS: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("Der Anmeldedienst ist nicht erreichbar: {0}")]
    Provider(String),
    #[error("Die Anmeldung ist abgelaufen oder unbekannt. Bitte erneut anmelden.")]
    UnknownState,
    #[error("Der Anmeldedienst hat die Anmeldung abgelehnt: {0}")]
    Denied(String),
    #[error("Das ID-Token ist ungültig: {0}")]
    InvalidToken(String),
    #[error("Dieses Konto darf sich bei Zunder nicht anmelden.")]
    NotAllowed,
}

/// Provider metadata (subset of OpenID Connect Discovery).
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderMetadata {
    pub issuer: String,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub jwks_uri: Url,
}

#[derive(Debug, Clone)]
struct Provider {
    metadata: ProviderMetadata,
    jwks: JwkSet,
    fetched_at: Instant,
}

#[derive(Debug, Clone)]
struct PendingLogin {
    nonce: String,
    verifier: String,
    return_to: String,
    created: Instant,
}

/// The start of a login: send the browser to `redirect` and set the login
/// cookie to `state`.
#[derive(Debug, Clone)]
pub struct LoginStart {
    pub redirect: Url,
    pub state: String,
}

/// A completed login.
#[derive(Debug, Clone)]
pub struct LoginDone {
    pub identity: Identity,
    pub return_to: String,
}

/// Claims Zunder reads from the ID token.
#[derive(Debug, Clone, Deserialize)]
pub struct IdClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preferred_username: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub azp: Option<String>,
    pub aud: serde_json::Value,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

pub struct OidcClient {
    config: OidcConfig,
    redirect_uri: Url,
    http: reqwest::Client,
    provider: tokio::sync::RwLock<Option<Provider>>,
    pending: Mutex<HashMap<String, PendingLogin>>,
}

impl OidcClient {
    pub fn new(config: OidcConfig, redirect_uri: Url, http: reqwest::Client) -> Self {
        Self {
            config,
            redirect_uri,
            http,
            provider: tokio::sync::RwLock::new(None),
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn discovery_url(&self) -> String {
        format!(
            "{}/.well-known/openid-configuration",
            self.config.issuer.as_str().trim_end_matches('/')
        )
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, OidcError> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        if !response.status().is_success() {
            return Err(OidcError::Provider(format!(
                "{url} antwortet mit {}",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|e| OidcError::Provider(format!("{url}: {e}")))
    }

    async fn fetch_provider(&self) -> Result<Provider, OidcError> {
        let metadata: ProviderMetadata = self.get_json(&self.discovery_url()).await?;
        if metadata.issuer.trim_end_matches('/')
            != self.config.issuer.as_str().trim_end_matches('/')
        {
            return Err(OidcError::Provider(format!(
                "Issuer {} passt nicht zu ZUNDER_OIDC_ISSUER",
                metadata.issuer
            )));
        }
        let jwks: JwkSet = self.get_json(metadata.jwks_uri.as_str()).await?;
        Ok(Provider {
            metadata,
            jwks,
            fetched_at: Instant::now(),
        })
    }

    /// The cached provider, fetched when missing, stale or when `refresh`.
    async fn provider(&self, refresh: bool) -> Result<Provider, OidcError> {
        if !refresh {
            if let Some(p) = self.provider.read().await.as_ref() {
                if p.fetched_at.elapsed() < PROVIDER_MAX_AGE {
                    return Ok(p.clone());
                }
            }
        }
        let fresh = self.fetch_provider().await?;
        *self.provider.write().await = Some(fresh.clone());
        Ok(fresh)
    }

    /// Starts a login and returns where to send the browser.
    pub async fn start_login(&self, return_to: String) -> Result<LoginStart, OidcError> {
        let provider = self.provider(false).await?;
        let state = random_secret();
        let nonce = random_secret();
        let verifier = random_secret();
        let mut redirect = provider.metadata.authorization_endpoint.clone();
        redirect
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", self.redirect_uri.as_str())
            .append_pair("scope", "openid email profile")
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, p| p.created.elapsed() < LOGIN_TTL);
        if pending.len() >= MAX_PENDING {
            if let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone())
            {
                pending.remove(&oldest);
            }
        }
        pending.insert(
            state.clone(),
            PendingLogin {
                nonce,
                verifier,
                return_to,
                created: Instant::now(),
            },
        );
        Ok(LoginStart { redirect, state })
    }

    /// Completes a login: exchanges the code, verifies the ID token and
    /// checks that the person is allowed.
    pub async fn finish_login(&self, state: &str, code: &str) -> Result<LoginDone, OidcError> {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(state)
            .filter(|p| p.created.elapsed() < LOGIN_TTL)
            .ok_or(OidcError::UnknownState)?;
        let provider = self.provider(false).await?;

        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", self.config.client_id.as_str()),
            ("code_verifier", pending.verifier.as_str()),
        ];
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret", secret.as_str()));
        }
        let response = self
            .http
            .post(provider.metadata.token_endpoint.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&form)
            .send()
            .await
            .map_err(|e| OidcError::Provider(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OidcError::Denied(format!(
                "Token-Endpunkt antwortet mit {status}: {}",
                body.chars().take(300).collect::<String>()
            )));
        }
        let tokens: TokenResponse = response
            .json()
            .await
            .map_err(|e| OidcError::Provider(format!("Token-Antwort: {e}")))?;

        let claims = match self.verify_id_token(&provider, &tokens.id_token, &pending.nonce) {
            Err(OidcError::InvalidToken(msg)) if msg == UNKNOWN_KEY => {
                // The provider may have rotated its keys.
                let provider = self.provider(true).await?;
                self.verify_id_token(&provider, &tokens.id_token, &pending.nonce)?
            }
            other => other?,
        };
        if !self.is_allowed(&claims) {
            tracing::warn!(subject = %claims.sub, "OIDC login of a person that is not allowed");
            return Err(OidcError::NotAllowed);
        }
        Ok(LoginDone {
            identity: Identity {
                subject: claims.sub,
                email: claims.email,
                name: claims.name.or(claims.preferred_username),
            },
            return_to: pending.return_to,
        })
    }

    fn verify_id_token(
        &self,
        provider: &Provider,
        token: &str,
        nonce: &str,
    ) -> Result<IdClaims, OidcError> {
        verify_id_token(
            token,
            &provider.jwks,
            &provider.metadata.issuer,
            &self.config.client_id,
            nonce,
        )
    }

    /// Only the configured person: by subject when
    /// `ZUNDER_OIDC_ALLOWED_SUBJECT` is set, otherwise by verified email.
    pub fn is_allowed(&self, claims: &IdClaims) -> bool {
        if let Some(subject) = &self.config.allowed_subject {
            return &claims.sub == subject;
        }
        match (&self.config.allowed_email, &claims.email) {
            (Some(allowed), Some(email)) => {
                email.to_lowercase() == *allowed && claims.email_verified != Some(false)
            }
            _ => false,
        }
    }
}

const UNKNOWN_KEY: &str = "unbekannter Schlüssel";

fn find_key<'a>(jwks: &'a JwkSet, kid: Option<&str>) -> Option<&'a Jwk> {
    match kid {
        Some(kid) => jwks.find(kid),
        None if jwks.keys.len() == 1 => jwks.keys.first(),
        None => None,
    }
}

/// Verifies an ID token: signature against the JWKS, `iss`, `aud` (and
/// `azp` with several audiences), `exp`/`iat`, and `nonce`.
pub fn verify_id_token(
    token: &str,
    jwks: &JwkSet,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> Result<IdClaims, OidcError> {
    let invalid = |msg: String| OidcError::InvalidToken(msg);
    let header = jsonwebtoken::decode_header(token).map_err(|e| invalid(e.to_string()))?;
    if matches!(
        header.alg,
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
    ) {
        return Err(invalid(
            "symmetrische Signaturen werden nicht akzeptiert".into(),
        ));
    }
    let jwk = find_key(jwks, header.kid.as_deref()).ok_or_else(|| invalid(UNKNOWN_KEY.into()))?;
    if let Some(key_alg) = jwk.common.key_algorithm {
        if key_alg.to_string().parse::<Algorithm>().ok() != Some(header.alg) {
            return Err(invalid("Algorithmus passt nicht zum Schlüssel".into()));
        }
    }
    let key = DecodingKey::from_jwk(jwk).map_err(|e| invalid(e.to_string()))?;
    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
    validation.leeway = LEEWAY_SECS;
    let data = jsonwebtoken::decode::<IdClaims>(token, &key, &validation)
        .map_err(|e| invalid(e.to_string()))?;
    let claims = data.claims;
    if claims.nonce.as_deref() != Some(nonce) {
        return Err(invalid("nonce passt nicht".into()));
    }
    if claims.aud.as_array().is_some_and(|a| a.len() > 1)
        && claims.azp.as_deref() != Some(client_id)
    {
        return Err(invalid("azp passt nicht".into()));
    }
    Ok(claims)
}
