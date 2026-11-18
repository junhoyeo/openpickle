//! The client, mirroring `ts/src/client.ts`.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use futures_core::Stream;
use futures_util::{StreamExt as _, stream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::events::{PickleEvent, SseFrame, SseParser};
use crate::types::*;
use crate::{ApiError, Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://character-api-prod.picklemem.com";

/// Bodies larger than this are parsed on the blocking pool so a 2 MB UI bundle does not hold up
/// the async runtime.
const BLOCKING_PARSE_THRESHOLD: usize = 256 * 1024;

/// How long a request and its response may take together. The event stream is exempt.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the download of an attachment may take. A video of some hundred megabytes on a
/// slow line needs minutes, where the 30 seconds of a call would cut it off.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// A refresh gets longer. Giving up on one that the server has already processed loses the new
/// token, and with it the login session, so a slow answer is worth waiting for.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(120);

/// What the app says about itself in request headers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProfile {
    /// `X-Client-Version`
    pub app_version: String,
    /// `X-Client-Build`
    pub app_build: String,
    /// `X-Client-Platform` (UI definitions, sign-in) and `platform` (session): `ios` or `android`.
    pub platform: String,
    pub user_agent: String,
    /// `X-Pickle-Language`
    pub language: String,
    /// `X-Pickle-Language-Source`
    pub language_source: String,
    pub accept_language: String,
    /// `X-Pickle-Storefront-Country`, an App Store storefront code such as `KOR`. When `None`, the
    /// header is left out and `X-Pickle-Store-Country-Resolved` is `false`, as the app does before
    /// StoreKit answers.
    pub storefront_country: Option<String>,
    /// `X-Pickle-Device-Region`
    pub device_region: String,
    /// `X-Client-Timezone`, sent with session, register and sign-in calls.
    pub timezone: String,
    /// Hash of the UI vocabulary, `X-Pickle-Vocabulary`.
    pub vocabulary: String,
}

impl ClientProfile {
    /// The iOS app the studied traffic came from (version 1.1.19, build 202611071128, Korea).
    pub fn ios() -> Self {
        Self {
            app_version: "1.1.19".into(),
            app_build: "202611071128".into(),
            platform: "ios".into(),
            user_agent: "Pickle/202611071128 CFNetwork/3860.700.1 Darwin/25.6.0".into(),
            language: "en".into(),
            language_source: "device_language".into(),
            accept_language: "en-US,en;q=0.9".into(),
            storefront_country: Some("KOR".into()),
            device_region: "KR".into(),
            timezone: "Asia/Seoul".into(),
            vocabulary: "6d926b057792cf46".into(),
        }
    }
}

impl Default for ClientProfile {
    fn default() -> Self {
        Self::ios()
    }
}

/// Called with every refresh response, before its access token is used. It must store
/// `login_session_token`: the one that was sent is spent, and a lost token ends the login session.
pub type RotateCallback = Arc<dyn Fn(&LoginSessionRefresh) -> io::Result<()> + Send + Sync>;

/// Asked for an access token before every request.
pub type TokenProvider = Arc<dyn Fn() -> String + Send + Sync>;

/// A login session: a refresh token (`ls_…`) the client trades for access tokens as they run out.
#[derive(Clone)]
pub struct LoginSession {
    pub token: String,
    /// An access token already issued with `token` (e.g. by `verify_phone_code`), used until it
    /// is a minute from expiry so the first request needs no refresh.
    pub access_token: Option<String>,
    pub on_rotate: RotateCallback,
}

impl LoginSession {
    pub fn new(
        token: impl Into<String>,
        on_rotate: impl Fn(&LoginSessionRefresh) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            token: token.into(),
            access_token: None,
            on_rotate: Arc::new(on_rotate),
        }
    }

    /// The session `verify_phone_code` just issued, with its access token.
    pub fn from_issued(
        issued: &LoginSessionIssued,
        on_rotate: impl Fn(&LoginSessionRefresh) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self::new(issued.login_session_token.clone(), on_rotate)
            .with_access_token(issued.login_session_jwt.clone())
    }

    pub fn with_access_token(mut self, jwt: impl Into<String>) -> Self {
        self.access_token = Some(jwt.into());
        self
    }
}

impl fmt::Debug for LoginSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginSession").finish_non_exhaustive()
    }
}

/// How the client gets its bearer token.
#[derive(Clone, Default)]
pub enum Credentials {
    /// Can only request and verify a phone code and read the UI definitions.
    #[default]
    SignedOut,
    /// A fixed access token. Access tokens expire 5 minutes after issue.
    AccessToken(String),
    AccessTokenProvider(TokenProvider),
    LoginSession(LoginSession),
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SignedOut => "SignedOut",
            Self::AccessToken(_) => "AccessToken(..)",
            Self::AccessTokenProvider(_) => "AccessTokenProvider(..)",
            Self::LoginSession(_) => "LoginSession(..)",
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    pub credentials: Credentials,
    /// The device id the app reports (iOS `identifierForVendor`). Defaults to a new random one.
    pub device_id: Option<String>,
    /// `X-Client-Launch-Id`: one id per app launch. Defaults to a new random one.
    pub launch_id: Option<String>,
    pub profile: ClientProfile,
    /// Defaults to [`DEFAULT_BASE_URL`].
    pub base_url: Option<String>,
    /// An HTTP client to share with other `PickleClient`s. Defaults to a new one.
    pub http: Option<reqwest::Client>,
}

/// Which identity a request carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    /// App identity headers and the bearer token.
    #[default]
    Bearer,
    /// The bearer token without the app identity headers, as the app loads pictures.
    BearerOnly,
    /// App identity headers only.
    App,
    /// Neither.
    None,
}

/// Options of [`PickleClient::request`] and [`PickleClient::send`].
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    pub query: Vec<(String, String)>,
    /// A JSON body, already serialized; see [`RequestOptions::json`].
    pub body: Option<Vec<u8>>,
    /// Added last, replacing headers of the same name.
    pub headers: HeaderMap,
    pub auth: Auth,
}

impl RequestOptions {
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Result<Self> {
        self.body = Some(serde_json::to_vec(body).map_err(Error::Encode)?);
        Ok(self)
    }

    fn query(mut self, key: &str, value: impl ToString) -> Self {
        self.query.push((key.to_owned(), value.to_string()));
        self
    }

    fn header(mut self, name: &'static str, value: &str) -> Result<Self> {
        self.headers
            .insert(HeaderName::from_static(name), header_value(value)?);
        Ok(self)
    }

    fn auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }
}

/// Options of [`PickleClient::send_message`].
#[derive(Clone, Debug, Default)]
pub struct SendMessageOptions {
    /// Defaults to a new random UUID.
    pub client_msg_id: Option<String>,
    pub in_call: bool,
    /// What the message is about, when it is sent from something on screen: an offer on the
    /// Explore tab ([`MessageContext::offer`]).
    pub context: Option<MessageContext>,
}

/// Options of [`PickleClient::end_phone_call`].
#[derive(Clone, Debug, Default)]
pub struct EndPhoneCallOptions {
    /// Defaults to a new random UUID. Reuse an id when retrying the same action.
    pub action_id: Option<String>,
    pub in_call: bool,
}

/// The claims of an access token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessTokenClaims {
    /// `user-live-<uuid>`
    pub sub: String,
    /// Login session id.
    pub sid: String,
    pub iat: i64,
    /// Access token expiry, Unix seconds (5 minutes after `iat`).
    pub exp: i64,
    /// Session expiry, Unix seconds (7 days after `iat`).
    pub sexp: i64,
    #[serde(flatten)]
    pub rest: JsonObject,
}

/// Reads the claims of a bearer token without verifying it, e.g. to see when it expires.
pub fn decode_access_token(token: &str) -> Result<AccessTokenClaims> {
    let token = token.strip_prefix("Bearer ").unwrap_or(token);
    let payload = token.split('.').nth(1).ok_or(Error::InvalidToken)?;
    let json = URL_SAFE_NO_PAD_INDIFFERENT
        .decode(payload)
        .map_err(|_| Error::InvalidToken)?;
    serde_json::from_slice(&json).map_err(|_| Error::InvalidToken)
}

/// 32 random bytes in unpadded base64url, the nonce format the app uses.
pub fn random_nonce() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A cheap-to-clone client for the Pickle character API. Clones share one connection pool and,
/// for login sessions, one refresh chain.
#[derive(Clone)]
pub struct PickleClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    base_url: String,
    profile: ClientProfile,
    device_id: String,
    launch_id: String,
    /// Sent with every request.
    base_headers: HeaderMap,
    /// `base_headers` plus the app identity headers.
    app_headers: HeaderMap,
    bearer: Bearer,
}

enum Bearer {
    SignedOut,
    Static(HeaderValue),
    Provider(TokenProvider),
    Session {
        state: Arc<tokio::sync::Mutex<SessionState>>,
        on_rotate: RotateCallback,
    },
}

struct SessionState {
    refresh_token: String,
    access: Option<SessionAccess>,
    /// A refresh whose new token the rotation callback could not store. It is offered to the
    /// callback again before anything else, and its access token is not used until then.
    unsaved: Option<LoginSessionRefresh>,
}

struct SessionAccess {
    header: HeaderValue,
    exp: i64,
}

impl fmt::Debug for PickleClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickleClient")
            .field("base_url", &self.inner.base_url)
            .field("profile", &self.inner.profile)
            .finish_non_exhaustive()
    }
}

impl PickleClient {
    pub fn new(options: ClientOptions) -> Result<Self> {
        let http = match options.http {
            Some(http) => http,
            // Keep-alives let a connection that died quietly (sleep, a dropped network) fail
            // instead of hanging; that matters most for the event stream, which has no timeout.
            None => reqwest::Client::builder()
                .gzip(true)
                .brotli(true)
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Duration::from_secs(30))
                .http2_keep_alive_interval(Duration::from_secs(30))
                .http2_keep_alive_timeout(Duration::from_secs(20))
                .http2_keep_alive_while_idle(true)
                .build()?,
        };
        let profile = options.profile;
        let device_id = options
            .device_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase());
        let launch_id = options
            .launch_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().hyphenated().to_string());

        let mut base_headers = HeaderMap::new();
        base_headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        base_headers.insert("accept-language", header_value(&profile.accept_language)?);
        base_headers.insert("user-agent", header_value(&profile.user_agent)?);
        base_headers.insert("pragma", HeaderValue::from_static("no-cache"));
        base_headers.insert("cache-control", HeaderValue::from_static("no-cache"));

        let mut app_headers = base_headers.clone();
        app_headers.insert("x-client-version", header_value(&profile.app_version)?);
        app_headers.insert("x-client-build", header_value(&profile.app_build)?);
        app_headers.insert("x-client-launch-id", header_value(&launch_id)?);
        app_headers.insert("x-pickle-language", header_value(&profile.language)?);
        app_headers.insert(
            "x-pickle-language-source",
            header_value(&profile.language_source)?,
        );
        app_headers.insert(
            "x-pickle-device-region",
            header_value(&profile.device_region)?,
        );
        app_headers.insert(
            "x-pickle-store-country-resolved",
            HeaderValue::from_static(if profile.storefront_country.is_some() {
                "true"
            } else {
                "false"
            }),
        );
        if let Some(country) = &profile.storefront_country {
            app_headers.insert("x-pickle-storefront-country", header_value(country)?);
        }

        let bearer = match options.credentials {
            Credentials::SignedOut => Bearer::SignedOut,
            Credentials::AccessToken(token) => Bearer::Static(bearer_value(&token)?),
            Credentials::AccessTokenProvider(provider) => Bearer::Provider(provider),
            Credentials::LoginSession(session) => {
                let access = match session.access_token {
                    Some(jwt) => Some(SessionAccess {
                        exp: decode_access_token(&jwt)?.exp,
                        header: bearer_value(&jwt)?,
                    }),
                    None => None,
                };
                Bearer::Session {
                    state: Arc::new(tokio::sync::Mutex::new(SessionState {
                        refresh_token: session.token,
                        access,
                        unsaved: None,
                    })),
                    on_rotate: session.on_rotate,
                }
            }
        };

        let base_url = options
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
            .to_owned();
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base_url,
                profile,
                device_id,
                launch_id,
                base_headers,
                app_headers,
                bearer,
            }),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.inner.profile
    }

    pub fn device_id(&self) -> &str {
        &self.inner.device_id
    }

    pub fn launch_id(&self) -> &str {
        &self.inner.launch_id
    }

    // ---- Account and session ----

    /// `POST /api/login-sessions/phone-code`: texts a 6-digit sign-in code to `phone` (E.164).
    /// `region` defaults to the profile's device region; `carrier` is sent only when given.
    pub async fn request_phone_code(
        &self,
        phone: &str,
        region: Option<&str>,
        carrier: Option<&str>,
    ) -> Result<PhoneCodeRequested> {
        let region = region.unwrap_or(&self.inner.profile.device_region);
        let mut body = json!({ "phone": phone, "region": region });
        if let Some(carrier) = carrier {
            body["carrier"] = carrier.into();
        }
        let options = self.sign_in_options()?.json(&body)?;
        self.request(Method::POST, "/api/login-sessions/phone-code", options)
            .await
    }

    /// `POST /api/login-sessions`: trades the texted code for a new login session. Pass the result
    /// to [`LoginSession::from_issued`].
    pub async fn verify_phone_code(
        &self,
        pending_id: &str,
        code: &str,
    ) -> Result<LoginSessionIssued> {
        let body = json!({ "pending_id": pending_id, "code": code, "kind": "phone_code" });
        let options = self.sign_in_options()?.json(&body)?;
        self.request(Method::POST, "/api/login-sessions", options)
            .await
    }

    /// `POST /api/login-sessions/refresh`: trades a refresh token for an access token and the next
    /// refresh token. The sent token is spent. A client with a login session calls this itself.
    pub async fn refresh_login_session(
        &self,
        login_session_token: &str,
    ) -> Result<LoginSessionRefresh> {
        let options = self
            .sign_in_options()?
            .header("x-pickle-login-session-rotation", "1")?
            .json(&json!({ "login_session_token": login_session_token }))?;
        const PATH: &str = "/api/login-sessions/refresh";
        // Not `request()`: `bearer_header()` calls this, and `request()` calls `bearer_header()`.
        let response = self
            .execute(Method::POST, PATH, options, None, Some(REFRESH_TIMEOUT))
            .await?;
        parse_response(Method::POST, PATH, response).await
    }

    /// `POST /api/account/register`. The app calls it on every launch.
    pub async fn register_account(&self) -> Result<RegisterResponse> {
        let options = RequestOptions::default()
            .header("x-pickle-onboarding-flow", "4")?
            .header("x-client-timezone", &self.inner.profile.timezone)?
            .header("x-pickle-reported-language", &self.inner.profile.language)?;
        self.request(Method::POST, "/api/account/register", options)
            .await
    }

    /// The `POST /api/session` body the app sends when it does not need a call room.
    pub fn session_request(&self) -> SessionRequest {
        let profile = &self.inner.profile;
        SessionRequest {
            bgm_library: Vec::new(),
            app_version: profile.app_version.clone(),
            vocabulary: profile.vocabulary.clone(),
            rig_ready: true,
            client_connect_id: uuid::Uuid::new_v4().to_string(),
            room_demand: "none".into(),
            platform: profile.platform.clone(),
            app_build: profile.app_build.clone(),
            device_memory_bytes: 12_262_113_280,
            establishment_aware: true,
            world_ready: false,
        }
    }

    /// `POST /api/session` with [`session_request`](Self::session_request): account, plan,
    /// credit and character state, plus a LiveKit room token.
    pub async fn start_session(&self) -> Result<SessionResponse> {
        self.start_session_with(&self.session_request()).await
    }

    pub async fn start_session_with(&self, request: &SessionRequest) -> Result<SessionResponse> {
        let options = RequestOptions::default()
            .header("x-client-timezone", &self.inner.profile.timezone)?
            .json(request)?;
        self.request(Method::POST, "/api/session", options).await
    }

    // ---- Rooms and chat ----

    /// `GET /api/sync`: every room with its cursors and unread counts.
    pub async fn sync(&self) -> Result<SyncResponse> {
        self.request(Method::GET, "/api/sync", RequestOptions::default())
            .await
    }

    /// `GET /api/rooms/{room}/history/page`: up to `limit` messages before `before_seq`, newest
    /// first. The answer's `next_before` is the `before_seq` of the page before it. The app
    /// asks for 250 at a time.
    pub async fn get_history_page(
        &self,
        room_id: &str,
        before_seq: i64,
        limit: u32,
    ) -> Result<HistoryPage> {
        let path = format!("/api/rooms/{}/history/page", encode_component(room_id));
        let options = RequestOptions::default()
            .query("limit", limit)
            .query("before_seq", before_seq);
        self.request(Method::GET, &path, options).await
    }

    /// `GET /api/attachments/{path}/thumbnail`: a JPEG of a message's attachment, a picture or
    /// a document's first page. `path` is as messages name it: `attachments/<day>/<file>`.
    pub async fn get_attachment_thumbnail(&self, path: &str) -> Result<bytes::Bytes> {
        let path = format!("{}/thumbnail", attachment_path(path));
        let options = RequestOptions::default().auth(Auth::BearerOnly);
        let response = self.send(Method::GET, &path, options).await?;
        Ok(response.bytes().await?)
    }

    /// `GET /api/attachments/{path}`: the attachment itself, the file as it was sent or as the
    /// character made it. `path` is as messages name it and ends in the file's name. A file
    /// may be large, so the exchange gets more time than a call.
    pub async fn get_attachment(&self, path: &str) -> Result<bytes::Bytes> {
        let options = RequestOptions::default().auth(Auth::BearerOnly);
        let response = self
            .dispatch(
                Method::GET,
                &attachment_path(path),
                options,
                Some(DOWNLOAD_TIMEOUT),
            )
            .await?;
        Ok(response.bytes().await?)
    }

    /// `GET /api/rooms/{room}/history`: the latest messages of a room (up to 200).
    pub async fn get_history(&self, room_id: &str) -> Result<HistoryWindow> {
        let path = format!("/api/rooms/{}/history", encode_component(room_id));
        self.request(Method::GET, &path, RequestOptions::default())
            .await
    }

    /// `GET /api/rooms/{room}/history/delta`: what happened in a room after `after_seq`. `limit`
    /// defaults to 500.
    pub async fn get_history_delta(
        &self,
        room_id: &str,
        after_seq: i64,
        limit: Option<u32>,
    ) -> Result<HistoryDelta> {
        let path = format!("/api/rooms/{}/history/delta", encode_component(room_id));
        let options = RequestOptions::default()
            .query("after_seq", after_seq)
            .query("limit", limit.unwrap_or(500));
        self.request(Method::GET, &path, options).await
    }

    /// Pages through [`get_history_delta`](Self::get_history_delta) from `after_seq` until the
    /// server has no more. The stream ends after the first error.
    pub fn history(
        &self,
        room_id: &str,
        after_seq: i64,
        limit: Option<u32>,
    ) -> impl Stream<Item = Result<HistoryDelta>> + Send + 'static {
        let client = self.clone();
        let room_id = room_id.to_owned();
        stream::unfold(Some(after_seq), move |cursor| {
            let client = client.clone();
            let room_id = room_id.clone();
            async move {
                let after_seq = cursor?;
                let page = client.get_history_delta(&room_id, after_seq, limit).await;
                let next = match &page {
                    Ok(page) if page.has_more && page.next_after > after_seq => {
                        Some(page.next_after)
                    }
                    _ => None,
                };
                Some((page, next))
            }
        })
    }

    /// `POST /api/rooms/{room}/messages`. The reply arrives on [`events`](Self::events): `chat`
    /// events carry `client_msg_id` as the message `id` and as the `target_id` of reactions to it.
    pub async fn send_message(
        &self,
        room_id: &str,
        text: &str,
        options: SendMessageOptions,
    ) -> Result<SendMessageResponse> {
        let client_msg_id = options
            .client_msg_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let path = format!("/api/rooms/{}/messages", encode_component(room_id));
        let mut body =
            json!({ "text": text, "client_msg_id": client_msg_id, "in_call": options.in_call });
        if let Some(context) = &options.context {
            body["context"] = serde_json::to_value(context).map_err(Error::Encode)?;
        }
        let mut response: SendMessageResponse = self
            .request(Method::POST, &path, RequestOptions::default().json(&body)?)
            .await?;
        response.client_msg_id = client_msg_id;
        Ok(response)
    }

    /// `PUT /api/rooms/{room}/read`: moves this device's read cursor.
    pub async fn mark_read(&self, room_id: &str, seq: i64) -> Result<StatusOk> {
        let path = format!("/api/rooms/{}/read", encode_component(room_id));
        let body = json!({ "seq": seq, "device_id": self.inner.device_id });
        self.request(Method::PUT, &path, RequestOptions::default().json(&body)?)
            .await
    }

    /// `GET /api/phone-calls/{call_id}`: call state and the transcript collected so far.
    pub async fn get_phone_call(&self, call_id: &str) -> Result<PhoneCallTranscript> {
        let path = format!("/api/phone-calls/{}", encode_component(call_id));
        self.request(Method::GET, &path, RequestOptions::default())
            .await
    }

    /// Sends the captured `end_phone_call` action. Keep `action_id` stable when retrying.
    pub async fn end_phone_call(
        &self,
        call_id: &str,
        options: EndPhoneCallOptions,
    ) -> Result<StatusOk> {
        let action_id = options
            .action_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let body = json!({
            "action_id": action_id,
            "in_call": options.in_call,
            "payload": { "call_id": call_id },
            "kind": "end_phone_call",
        });
        self.request(
            Method::POST,
            "/api/actions",
            RequestOptions::default().json(&body)?,
        )
        .await
    }

    /// `GET /api/events`: the live event stream. The server opens with its current state and
    /// suggests a reconnection delay ([`PickleEvent::Retry`]). The stream ends when the server
    /// closes it; reconnecting is up to the caller. Dropping the stream closes the connection.
    pub async fn events(&self) -> Result<impl Stream<Item = Result<PickleEvent>> + Send + 'static> {
        let mut options = RequestOptions::default();
        options
            .headers
            .insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        // No overall timeout: the stream stays open for as long as the app runs.
        let response = self
            .dispatch(Method::GET, "/api/events", options, None)
            .await?;
        Ok(event_stream(response))
    }

    // ---- Profile, character and social ----

    /// `GET /api/character`: your character's look, level, energy and jelly.
    pub async fn get_character(&self) -> Result<Character> {
        self.request(Method::GET, "/api/character", RequestOptions::default())
            .await
    }

    /// `GET /api/profile?region=`: your own profile. `region` defaults to the device region.
    pub async fn get_profile(&self, region: Option<&str>) -> Result<Profile> {
        let region = region.unwrap_or(&self.inner.profile.device_region);
        self.request(
            Method::GET,
            "/api/profile",
            RequestOptions::default().query("region", region),
        )
        .await
    }

    /// `GET /api/social/snapshot?scopes=`. Pass [`ALL_SOCIAL_SCOPES`] for what the app asks for.
    pub async fn get_social_snapshot(&self, scopes: &[&str]) -> Result<SocialSnapshot> {
        let options = RequestOptions::default().query("scopes", scopes.join(","));
        self.request(Method::GET, "/api/social/snapshot", options)
            .await
    }

    /// `GET /api/screens/{kind}`: data for a server-driven screen, untyped.
    pub async fn get_screen(
        &self,
        kind: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        self.get_screen_as(kind, query).await
    }

    /// `GET /api/screens/{kind}`, parsed as `T`.
    pub async fn get_screen_as<T: DeserializeOwned + Send + 'static>(
        &self,
        kind: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let path = format!("/api/screens/{}", encode_component(kind));
        let options = RequestOptions {
            query: query
                .iter()
                .map(|&(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            ..RequestOptions::default()
        };
        self.request(Method::GET, &path, options).await
    }

    /// `GET /api/screens/handoffs`: tasks and phone calls the character ran.
    pub async fn get_handoffs(&self) -> Result<HandoffsScreen> {
        self.get_screen_as("handoffs", &[]).await
    }

    /// `GET /api/screens/handoff?name=`: one task's plan and progress log.
    pub async fn get_handoff(&self, name: &str) -> Result<HandoffScreen> {
        self.get_screen_as("handoff", &[("name", name)]).await
    }

    /// `GET /api/screens/extra-conversations`.
    pub async fn get_extra_conversations(&self) -> Result<ExtraConversationsScreen> {
        self.get_screen_as("extra-conversations", &[]).await
    }

    /// `GET /api/screens/explore-sections`: what the character offers to do, in sections.
    pub async fn get_explore_sections(&self) -> Result<ExploreSectionsScreen> {
        self.get_screen_as("explore-sections", &[]).await
    }

    /// `POST /api/offers/impressions`: tells the server which offers were shown on screen.
    pub async fn report_offer_impressions(&self, offer_ids: &[String]) -> Result<()> {
        let body = json!({ "offer_ids": offer_ids });
        self.request::<serde_json::Value>(
            Method::POST,
            "/api/offers/impressions",
            RequestOptions::default().json(&body)?,
        )
        .await
        .map(drop)
    }

    /// `GET /api/screens/message-search`: searches your messages for `query`.
    pub async fn search_messages(&self, query: &str) -> Result<MessageSearchScreen> {
        self.get_screen_as("message-search", &[("query", query)])
            .await
    }

    /// `GET /api/screens/roulette-promo`.
    pub async fn get_roulette_promo(&self) -> Result<RoulettePromoScreen> {
        self.get_screen_as("roulette-promo", &[]).await
    }

    // ---- Device ----

    /// `PUT /api/device/permissions`: reports which device data sources the app may read.
    pub async fn report_permissions(&self, snapshots: &[PermissionSnapshot]) -> Result<StatusOk> {
        let body = json!({ "device_id": self.inner.device_id, "snapshots": snapshots });
        self.request(
            Method::PUT,
            "/api/device/permissions",
            RequestOptions::default().json(&body)?,
        )
        .await
    }

    /// `POST /api/device/location/token`. Fails with 409 `location_disabled` while location
    /// sharing is off; the success shape was not present in traffic.
    pub async fn request_location_token(&self) -> Result<JsonObject> {
        let body = json!({ "device_id": self.inner.device_id });
        self.request(
            Method::POST,
            "/api/device/location/token",
            RequestOptions::default().json(&body)?,
        )
        .await
    }

    // ---- UI definitions ----

    /// `GET /api/ui-definitions`: the server-driven UI bundle. Needs no token.
    pub async fn get_ui_definitions(&self) -> Result<UiDefinitions> {
        let options = RequestOptions::default()
            .auth(Auth::None)
            .header("x-pickle-vocabulary", &self.inner.profile.vocabulary)?
            .header("x-client-platform", &self.inner.profile.platform)?;
        self.request(Method::GET, "/api/ui-definitions", options)
            .await
    }

    // ---- Secure (enclave credential protocol, read side) ----

    /// `GET /api/secure/account`: the id of the account's master key.
    pub async fn get_secure_account(&self) -> Result<SecureAccount> {
        self.request(
            Method::GET,
            "/api/secure/account",
            RequestOptions::default(),
        )
        .await
    }

    /// `GET /api/secure/nodes?nonce=`: live enclave nodes with attestation documents bound to
    /// `nonce` (a new [`random_nonce`] when `None`).
    pub async fn get_enclave_nodes(&self, nonce: Option<&str>) -> Result<EnclaveNodes> {
        let nonce = nonce.map_or_else(random_nonce, str::to_owned);
        self.request(
            Method::GET,
            "/api/secure/nodes",
            RequestOptions::default().query("nonce", nonce),
        )
        .await
    }

    /// `POST /api/secure/nodes/{node}/status`: the node's signed log head, bound to `nonce`.
    pub async fn get_node_status(&self, node: &str, nonce: Option<&str>) -> Result<NodeStatus> {
        let nonce = nonce.map_or_else(random_nonce, str::to_owned);
        let path = format!("/api/secure/nodes/{}/status", encode_component(node));
        let options = RequestOptions::default().json(&json!({ "nonce": nonce }))?;
        self.request(Method::POST, &path, options).await
    }

    /// `GET /api/secure/log/nodes`: every node that ever held a log for this account.
    pub async fn get_log_nodes(&self) -> Result<LogNodes> {
        self.request(
            Method::GET,
            "/api/secure/log/nodes",
            RequestOptions::default(),
        )
        .await
    }

    /// `POST /api/secure/log/store/listings`: presigned S3 listing URLs for the credential log.
    pub async fn get_log_store_listings(
        &self,
        request: &LogStoreListingsRequest,
    ) -> Result<LogStoreListings> {
        let options = RequestOptions::default().json(request)?;
        self.request(Method::POST, "/api/secure/log/store/listings", options)
            .await
    }

    /// `GET /api/secure/ledger`: the node ledger, sealed with a key only devices hold.
    pub async fn get_shared_ledger(&self) -> Result<SharedLedger> {
        self.request(Method::GET, "/api/secure/ledger", RequestOptions::default())
            .await
    }

    /// `GET /api/secure/transfers`: pending master-key transfers between devices.
    pub async fn get_transfers(&self) -> Result<Transfers> {
        self.request(
            Method::GET,
            "/api/secure/transfers",
            RequestOptions::default(),
        )
        .await
    }

    // ---- Transport ----

    /// Calls any endpoint and parses the JSON response. An empty body parses as `{}`.
    pub async fn request<T: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        options: RequestOptions,
    ) -> Result<T> {
        let response = self.send(method.clone(), path, options).await?;
        parse_response(method, path, response).await
    }

    /// Calls any endpoint and returns the raw response. Fails with [`Error::Api`] unless it is 2xx.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        options: RequestOptions,
    ) -> Result<Response> {
        self.dispatch(method, path, options, Some(REQUEST_TIMEOUT))
            .await
    }

    /// `timeout` covers the whole exchange, body included; the event stream passes `None`.
    async fn dispatch(
        &self,
        method: Method,
        path: &str,
        options: RequestOptions,
        timeout: Option<Duration>,
    ) -> Result<Response> {
        if !matches!(options.auth, Auth::Bearer | Auth::BearerOnly) {
            return self.execute(method, path, options, None, timeout).await;
        }
        let bearer = self.bearer_header().await?;
        // Only a login session can get a new token, so only then is the request kept for a retry.
        let retry = matches!(self.inner.bearer, Bearer::Session { .. }).then(|| options.clone());
        let result = self
            .execute(method.clone(), path, options, Some(bearer.clone()), timeout)
            .await;
        match (result, retry) {
            // The server says the token expired although this clock says it has not: a clock
            // that runs behind would otherwise fail every request and never refresh.
            (Err(error), Some(options)) if is_token_expired(&error) => {
                self.forget_access_token(&bearer).await;
                let bearer = self.bearer_header().await?;
                self.execute(method, path, options, Some(bearer), timeout)
                    .await
            }
            (result, _) => result,
        }
    }

    async fn execute(
        &self,
        method: Method,
        path: &str,
        options: RequestOptions,
        bearer: Option<HeaderValue>,
        timeout: Option<Duration>,
    ) -> Result<Response> {
        let inner = &self.inner;
        let mut headers = match options.auth {
            Auth::None | Auth::BearerOnly => inner.base_headers.clone(),
            Auth::Bearer | Auth::App => inner.app_headers.clone(),
        };
        if let Some(bearer) = bearer {
            headers.insert(AUTHORIZATION, bearer);
        }
        if options.body.is_some() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        headers.extend(options.headers);

        let mut url = String::with_capacity(inner.base_url.len() + path.len() + 64);
        url.push_str(&inner.base_url);
        url.push_str(path);
        append_query(&mut url, &options.query, path.contains('?'));

        let mut request = inner.http.request(method.clone(), url).headers(headers);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(body) = options.body {
            request = request.body(body);
        }
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = response.bytes().await?;
        let detail = serde_json::from_slice::<ApiErrorBody>(&body)
            .ok()
            .map(|b| b.detail);
        Err(Error::Api(Box::new(ApiError {
            method,
            path: path.to_owned(),
            status,
            detail,
            body,
        })))
    }

    /// Drops the cached access token if it is still `rejected`, so the next request refreshes.
    async fn forget_access_token(&self, rejected: &HeaderValue) {
        if let Bearer::Session { state, .. } = &self.inner.bearer {
            let mut state = state.lock().await;
            if state
                .access
                .as_ref()
                .is_some_and(|access| access.header == *rejected)
            {
                state.access = None;
            }
        }
    }

    fn sign_in_options(&self) -> Result<RequestOptions> {
        RequestOptions::default()
            .auth(Auth::App)
            .header("x-pickle-onboarding-flow", "4")?
            .header("x-pickle-sign-in-flow", "2")?
            .header("x-client-timezone", &self.inner.profile.timezone)?
            .header("x-client-platform", &self.inner.profile.platform)
    }

    async fn bearer_header(&self) -> Result<HeaderValue> {
        match &self.inner.bearer {
            Bearer::SignedOut => Err(Error::SignedOut),
            Bearer::Static(header) => Ok(header.clone()),
            Bearer::Provider(provider) => bearer_value(&provider()),
            Bearer::Session { state, on_rotate } => {
                // Holding the lock across the refresh makes it single-flight: two concurrent
                // refreshes would spend the same token twice. Waiters then see the new token.
                let mut state = state.clone().lock_owned().await;
                // Refresh a minute early so clock skew and slow requests do not meet an expired token.
                if let Some(access) = &state.access
                    && access.exp - 60 > now_secs()
                {
                    return Ok(access.header.clone());
                }
                // A spawned task finishes even if this request is dropped mid-refresh; otherwise the
                // server could spend the old token before the new one is stored.
                let client = self.clone();
                let on_rotate = on_rotate.clone();
                let refresh = tokio::spawn(async move {
                    state.access = None;
                    loop {
                        let (refresh, fresh) = match state.unsaved.take() {
                            Some(refresh) => (refresh, false),
                            None => {
                                let refresh =
                                    client.refresh_login_session(&state.refresh_token).await?;
                                state.refresh_token.clone_from(&refresh.login_session_token);
                                (refresh, true)
                            }
                        };
                        if let Err(error) = on_rotate(&refresh) {
                            state.unsaved = Some(refresh);
                            return Err(Error::Rotate(error));
                        }
                        let exp = decode_access_token(&refresh.login_session_jwt)?.exp;
                        // A refresh that waited to be stored may have aged out meanwhile.
                        if !fresh && exp - 60 <= now_secs() {
                            continue;
                        }
                        let access = SessionAccess {
                            exp,
                            header: bearer_value(&refresh.login_session_jwt)?,
                        };
                        let header = access.header.clone();
                        state.access = Some(access);
                        return Ok(header);
                    }
                });
                refresh.await.unwrap_or(Err(Error::RefreshAborted))
            }
        }
    }
}

/// `401 {"detail": "token_expired"}`.
fn is_token_expired(error: &Error) -> bool {
    error
        .api()
        .is_some_and(|api| api.status == 401 && api.detail.as_deref() == Some("token_expired"))
}

async fn parse_response<T: DeserializeOwned + Send + 'static>(
    method: Method,
    path: &str,
    response: Response,
) -> Result<T> {
    let body = response.bytes().await?;
    let parsed = if body.len() > BLOCKING_PARSE_THRESHOLD {
        tokio::task::spawn_blocking(move || serde_json::from_slice(&body))
            .await
            .expect("JSON parsing panicked")
    } else {
        serde_json::from_slice(if body.is_empty() { b"{}" } else { &body })
    };
    parsed.map_err(|source| Error::Decode {
        method,
        path: path.to_owned(),
        source,
    })
}

fn event_stream(response: Response) -> impl Stream<Item = Result<PickleEvent>> + Send + 'static {
    struct State<S> {
        body: S,
        parser: SseParser,
        frames: Vec<SseFrame>,
        ready: VecDeque<Result<PickleEvent>>,
        failed: bool,
    }
    let state = State {
        body: Box::pin(response.bytes_stream()),
        parser: SseParser::new(),
        frames: Vec::new(),
        ready: VecDeque::new(),
        failed: false,
    };
    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.ready.pop_front() {
                return Some((item, state));
            }
            if state.failed {
                return None;
            }
            let chunk = match state.body.next().await? {
                Ok(chunk) => chunk,
                // The connection is gone; report it once and end.
                Err(error) => {
                    state.failed = true;
                    return Some((Err(error.into()), state));
                }
            };
            state.parser.push(&chunk, &mut state.frames);
            for frame in state.frames.drain(..) {
                if let Some(ms) = frame.retry {
                    state
                        .ready
                        .push_back(Ok(PickleEvent::Retry(Duration::from_millis(ms))));
                }
                if let Some(event) = PickleEvent::from_frame(&frame) {
                    state.ready.push_back(Ok(event));
                }
            }
        }
    })
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn header_value(value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|_| Error::InvalidHeader)
}

fn bearer_value(token: &str) -> Result<HeaderValue> {
    let mut value = header_value(&format!("Bearer {token}"))?;
    value.set_sensitive(true);
    Ok(value)
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is percent-encoded.
/// The request path of an attachment. Each part of the attachment's own path is encoded by
/// itself; the slashes between them stay.
fn attachment_path(path: &str) -> String {
    let encoded: Vec<String> = path.split('/').map(encode_component).collect();
    format!("/api/attachments/{}", encoded.join("/"))
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    encode_into(&mut out, value, false);
    out
}

fn encode_into(out: &mut String, value: &str, keep_commas: bool) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in value.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) || (keep_commas && b == b',') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[usize::from(b >> 4)] as char);
            out.push(HEX[usize::from(b & 15)] as char);
        }
    }
}

/// The app leaves commas unescaped (`scopes=a,b`); everything else is encoded as by
/// `encodeURIComponent`.
///
/// `has_query` says the path already carries a query, so these pairs join it with `&`.
fn append_query(url: &mut String, query: &[(String, String)], has_query: bool) {
    for (i, (key, value)) in query.iter().enumerate() {
        url.push(if i == 0 && !has_query { '?' } else { '&' });
        encode_into(url, key, true);
        url.push('=');
        encode_into(url, value, true);
    }
}
