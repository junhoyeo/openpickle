//! Public native OAuth configuration from the verified Kakao flow, not client secrets.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, RequestBuilder, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const KAKAO_NATIVE_APP_KEY: &str = "41aae57a320049f89503212f8452b399";
pub const KAKAO_REDIRECT_URI: &str = "kakao41aae57a320049f89503212f8452b399://oauth";
pub const KAKAO_IOS_BUNDLE_ID: &str = "com.pickle.client";
pub const KAKAO_KA: &str = "sdk/2.24.4 sdk_type/swift os/ios-18.1 lang/ko-KR res/1179x2556 device/iPhone origin/com.pickle.client appPkg/com.pickle.client";

#[derive(Clone)]
pub struct KakaoConfig {
    pub client_id: String,
    pub redirect_uri: String,
    pub ios_bundle_id: String,
    pub ka: String,
}
impl Default for KakaoConfig {
    fn default() -> Self {
        Self {
            client_id: KAKAO_NATIVE_APP_KEY.into(),
            redirect_uri: KAKAO_REDIRECT_URI.into(),
            ios_bundle_id: KAKAO_IOS_BUNDLE_ID.into(),
            ka: KAKAO_KA.into(),
        }
    }
}
// Deliberately no Debug: verifier and authorization codes must not reach logs.
pub struct KakaoAuthorization {
    pub url: String,
    pub verifier: String,
}
#[derive(Debug, thiserror::Error)]
pub enum KakaoError {
    #[error("expected a Kakao redirect URL or authorization code")]
    InvalidRedirect,
    #[error("Kakao authorization was denied")]
    Denied,
    #[error("could not generate PKCE randomness")]
    Random,
    #[error("Kakao token request failed")]
    Network,
    #[error("Kakao token request timed out")]
    Timeout,
    #[error("Kakao token endpoint returned status {0}")]
    Status(u16),
    #[error("Kakao response has no id_token")]
    MissingToken,
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
impl KakaoConfig {
    pub fn authorize(&self) -> Result<KakaoAuthorization, KakaoError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| KakaoError::Random)?;
        let verifier = URL_SAFE_NO_PAD.encode(bytes);
        let mut url = Url::parse("https://kauth.kakao.com/oauth/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
            ("code_challenge", &challenge(&verifier)),
            ("code_challenge_method", "S256"),
            ("ka", &self.ka),
        ]);
        Ok(KakaoAuthorization {
            url: url.into(),
            verifier,
        })
    }
    pub fn parse_code(&self, input: &str) -> Result<String, KakaoError> {
        let input = input.trim();
        if input.is_empty() || input.chars().any(char::is_whitespace) {
            return Err(KakaoError::InvalidRedirect);
        }
        if !input.contains(':') {
            return if input.contains(['?', '&', '#', '/']) {
                Err(KakaoError::InvalidRedirect)
            } else {
                Ok(input.into())
            };
        }
        let mut url = Url::parse(input).map_err(|_| KakaoError::InvalidRedirect)?;
        let pairs: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if url.fragment().is_some() {
            return Err(KakaoError::InvalidRedirect);
        }
        url.set_query(None);
        if url.as_str() != self.redirect_uri {
            return Err(KakaoError::InvalidRedirect);
        }
        if pairs.iter().any(|(k, _)| k == "error") {
            return Err(KakaoError::Denied);
        }
        let codes: Vec<_> = pairs.iter().filter(|(k, _)| k == "code").collect();
        match codes.as_slice() {
            [(_, code)] if !code.is_empty() && !code.chars().any(char::is_whitespace) => {
                Ok(code.clone())
            }
            _ => Err(KakaoError::InvalidRedirect),
        }
    }
    fn token_request(&self, http: &Client, code: &str, verifier: &str) -> RequestBuilder {
        let mut body = Url::parse("https://kauth.kakao.com/oauth/token").unwrap();
        body.query_pairs_mut().extend_pairs([
            ("grant_type", "authorization_code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
            ("code", code),
            ("code_verifier", verifier),
            ("ios_bundle_id", &self.ios_bundle_id),
        ]);
        http.post("https://kauth.kakao.com/oauth/token")
            .header("KA", &self.ka)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .timeout(Duration::from_secs(30))
            .body(body.query().unwrap().to_owned())
    }
    pub async fn exchange_code(&self, code: &str, verifier: &str) -> Result<String, KakaoError> {
        // Separate transport: never forward Pickle authorization headers to Kakao or follow redirects.
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| KakaoError::Network)?;
        let response = self
            .token_request(&http, code, verifier)
            .send()
            .await
            .map_err(transport_error)?;
        if !response.status().is_success() {
            return Err(KakaoError::Status(response.status().as_u16()));
        }
        #[derive(Deserialize)]
        struct Token {
            id_token: String,
        }
        let bytes = response.bytes().await.map_err(transport_error)?;
        let token: Token = serde_json::from_slice(&bytes).map_err(|_| KakaoError::MissingToken)?;
        if token.id_token.is_empty() {
            return Err(KakaoError::MissingToken);
        }
        Ok(token.id_token)
    }
}
fn transport_error(error: reqwest::Error) -> KakaoError {
    if error.is_timeout() {
        KakaoError::Timeout
    } else {
        KakaoError::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_and_wire_contract() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let config = KakaoConfig::default();
        let pending = config.authorize().unwrap();
        assert_eq!(pending.verifier.len(), 43);
        let url = Url::parse(&pending.url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["ka"], KAKAO_KA);
        assert_eq!(query["code_challenge"], challenge(&pending.verifier));
        assert_eq!(query["redirect_uri"], KAKAO_REDIRECT_URI);
        assert_ne!(pending.verifier, config.authorize().unwrap().verifier);
        let request = config
            .token_request(&Client::new(), "code+&", &pending.verifier)
            .build()
            .unwrap();
        assert_eq!(request.headers()["ka"], KAKAO_KA);
        assert_eq!(request.method(), "POST");
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body.contains("code=code%2B%26"));
        assert!(body.contains("ios_bundle_id=com.pickle.client"));
        assert!(!body.contains("client_secret"));
    }
    #[test]
    fn parses_only_expected_callback() {
        let config = KakaoConfig::default();
        assert_eq!(config.parse_code(" code ").unwrap(), "code");
        assert_eq!(
            config
                .parse_code(&format!("{KAKAO_REDIRECT_URI}?code=a%2Bb"))
                .unwrap(),
            "a+b"
        );
        for input in ["", "https://example.com?code=x", "a b", "?code=x"] {
            assert!(config.parse_code(input).is_err());
        }
        assert!(matches!(
            config.parse_code(&format!("{KAKAO_REDIRECT_URI}?error=access_denied")),
            Err(KakaoError::Denied)
        ));
        assert!(
            config
                .parse_code(&format!("{KAKAO_REDIRECT_URI}?code=a&code=b"))
                .is_err()
        );
    }
}
