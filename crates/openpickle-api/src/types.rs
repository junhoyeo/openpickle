//! Wire types, mirroring `ts/src/types.ts`.
//!
//! Field names keep the server's snake_case. Unknown fields are ignored. Fields the TypeScript
//! types mark optional are `Option` and stay out of the JSON when `None`; fields the server sends
//! as `null` are `Option` too. Open string unions (values seen in traffic, others possible) are
//! `String`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A JSON object kept as it came.
pub type JsonObject = Map<String, Value>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusOk {
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiErrorBody {
    /// Machine-readable reason, e.g. `location_disabled` or `token_expired`.
    pub detail: String,
}

// ---- Account and session ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameSuggestion {
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Onboarding {
    pub region_name: String,
    pub region_waitlist_joined: bool,
    pub region_waitlist_phone: String,
    pub region_waitlist_place: String,
    pub invite_waitlist_joined: bool,
    pub character_named: bool,
    pub character_name: String,
    pub display_name: String,
    pub profile_avatar_present: bool,
    pub suspension_reason: String,
    /// `YYYY-MM-DD`
    pub birthday: String,
    pub consented: bool,
    pub consent_marketing: bool,
    pub consent_screen_recorded: bool,
    pub jaum_glyph: String,
    pub location_terms_consented: bool,
    pub location_terms_withdrawn: bool,
    pub model_training_consented: bool,
    pub model_training_consent_recorded: bool,
    pub name_suggestions: Vec<NameSuggestion>,
}

/// `POST /api/account/register`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub user_id: String,
    pub account_lineage: i64,
    pub account_rev: i64,
    pub entry: String,
    pub onboarding: Onboarding,
    pub character_room_id: String,
    pub market: String,
    pub registered: bool,
}

/// The body of `POST /api/session`. [`PickleClient::session_request`](crate::PickleClient::session_request)
/// fills in what the app sends.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    pub bgm_library: Vec<String>,
    pub app_version: String,
    /// Hash of the UI vocabulary the app holds; also sent as `X-Pickle-Vocabulary`.
    pub vocabulary: String,
    pub rig_ready: bool,
    pub client_connect_id: String,
    /// The app sends `none` when it does not need a LiveKit room joined right away.
    pub room_demand: String,
    pub platform: String,
    pub app_build: String,
    pub device_memory_bytes: i64,
    pub establishment_aware: bool,
    pub world_ready: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub label: String,
    pub rank: i64,
    pub credits: i64,
    pub credits_text: String,
    pub weekly_credits: i64,
    pub weekly_credits_text: String,
    pub product_ids: Vec<String>,
    pub offered: bool,
    pub upgrade_rank: i64,
    pub upgrade_label: String,
    pub owned_product_id: String,
    pub owned_price_text: String,
    pub owned_period_unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditUsage {
    pub allowance: i64,
    pub used: i64,
    pub remaining: i64,
    pub bonus: i64,
    pub exhausted: bool,
    /// ISO 8601
    pub reset_at: String,
    pub ticket_credits: i64,
    pub tickets: i64,
    pub tickets_by_kind: BTreeMap<String, i64>,
    pub ticket_auto_use: bool,
    pub period: String,
    pub balance: i64,
    pub credit_auto_use: bool,
    pub auto_use_order: String,
    pub ticket_order: String,
    pub credit_spending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelMilestone {
    pub level: i64,
    pub amount: i64,
    pub full_size: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JellyRewards {
    /// `YYYY-MM-DD`
    pub day: String,
    pub checkin_available: bool,
    pub checkin_amount: i64,
    pub talk_seconds: i64,
    pub talk_milestones: Vec<Value>,
    pub talk_claimable: i64,
    pub friends_reward_day: String,
    pub notices: Vec<Value>,
    pub purchase_granted: bool,
    pub level_milestones: Vec<LevelMilestone>,
    pub next_level_reward_level: i64,
    pub next_level_reward_amount: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterStatus {
    pub color_combo_id: String,
    pub level: i64,
    pub xp: i64,
    pub xp_next: i64,
    /// 0..1
    pub satiety: f64,
    pub portrait_redraw_due: bool,
    pub body_color: String,
    pub line_id: String,
    pub body_hex: String,
    pub line_hex: String,
    pub energy_pct: i64,
    pub energy_exhausted: bool,
    pub jelly_count: i64,
    pub jelly_checkin_available: bool,
    pub jelly_checkin_amount: i64,
    pub jelly_notice_kind: String,
    pub jelly_notice_amount: i64,
    pub level_milestones: Vec<LevelMilestone>,
    pub next_level_reward_level: i64,
    pub next_level_reward_amount: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortraitReferenceView {
    pub yaw_deg: f64,
    pub pitch_deg: f64,
    /// Face-rig channel values keyed by `<bone>.<property>.<axis>`.
    pub channels: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plans {
    pub items: Vec<Plan>,
    pub by_id: BTreeMap<String, Plan>,
    pub max_rank: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionCredits {
    #[serde(flatten)]
    pub usage: CreditUsage,
    pub invite_reward: i64,
    pub friends_reward: i64,
    pub invite_code_claimable: bool,
    pub friends_reward_claimable: bool,
    pub allowance_free: i64,
    pub allowance_plus: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegalDocument {
    pub document: String,
    pub name: String,
    pub version: String,
    pub url: String,
    pub updated: bool,
    pub kind: Option<String>,
    pub reconsent_required: bool,
    pub listed: bool,
}

/// `POST /api/session`. About 100 keys, most of them client configuration for the 3D world; the
/// ones a client is likely to use are typed and the rest are kept in `rest`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub user_id: String,
    pub account_lineage: i64,
    pub account_rev: i64,
    pub entry: String,
    pub onboarding: Onboarding,
    pub character_room_id: String,
    /// LiveKit room name, `character_prod_user_<user_id>`.
    pub room: String,
    pub livekit_url: String,
    /// LiveKit access token (HS256, about 6 hours). Also the bearer for `POST /api/telemetry`.
    pub token: String,
    pub participant_identity: String,
    pub session_id: String,
    pub tester_eligible: bool,
    pub admin_eligible: bool,
    pub ios_min_build: String,
    pub android_min_build: String,
    pub plan: String,
    pub plan_rank: i64,
    pub plans: Plans,
    pub credits: SessionCredits,
    pub jelly_count: i64,
    pub jelly_rewards: JellyRewards,
    pub character_name: String,
    pub character_status: CharacterStatus,
    pub character_voice: String,
    pub character_avatar_url: String,
    pub market: String,
    pub display_name: String,
    pub handle: String,
    pub profile_avatar_url: String,
    pub profile_url: String,
    pub flags: BTreeMap<String, bool>,
    pub seen_flags: Vec<String>,
    pub legal_documents: Vec<LegalDocument>,
    #[serde(flatten)]
    pub rest: JsonObject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhoneCodeRequested {
    /// Names the texted code in `POST /api/login-sessions`.
    pub pending_id: String,
    /// Seconds the code stays valid.
    pub expires_in: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginSessionRefresh {
    /// The new access token, valid for 5 minutes.
    pub login_session_jwt: String,
    /// When the login session ends (7 days after the refresh), ISO 8601.
    pub expires_at: String,
    /// The next refresh token. The one that was sent is spent.
    pub login_session_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginSessionIssued {
    pub status: String,
    pub user_id: String,
    pub login_session_jwt: String,
    pub expires_at: String,
    pub login_session_token: String,
}

// ---- Profile and character ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub kind: String,
    pub display_name: String,
    pub handle: String,
    pub phone_text: String,
    /// `MM/DD/YYYY`
    pub birthday_text: String,
    pub birthday_month: String,
    pub birthday_day: String,
    pub birthday_year: String,
    pub avatar_url: String,
    pub profile_url: String,
    pub phone_dial_code: String,
    pub phone_country_flag: String,
    pub phone_region: String,
    pub model_training_consented: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub voice: String,
    pub avatar_url: String,
    pub status: CharacterStatus,
    pub map_visibility: String,
    pub roam_radius_m: f64,
    pub roam_radius_mi: f64,
    pub outing_radius_m: f64,
    pub portrait_reference_views: Vec<PortraitReferenceView>,
    pub body_kind: String,
    pub jaum_glyph: String,
    pub jaum_material: String,
    pub jaum_color: String,
    pub solid_color: String,
    pub critter_species_name: String,
    /// ISO 8601
    pub critter_drawn_at: String,
    pub jars_remaining: i64,
    pub jaum_portrait_version: i64,
}

// ---- Rooms and chat ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserSummary {
    pub user_id: String,
    pub handle: String,
    pub display_name: String,
    pub avatar_url: String,
    pub character_avatar_url: String,
    pub character_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub room_id: String,
    /// `character`, `character_pair`, …
    pub kind: String,
    pub title: String,
    /// `None` for the room with your own character.
    pub peer: Option<UserSummary>,
    pub member_count: i64,
    pub last_seq: i64,
    pub read_seq: i64,
    pub role: String,
    /// ISO 8601
    pub last_event_at: String,
    /// True for rooms you only watch, such as your character talking with a friend's character.
    pub observer: bool,
    pub unread: i64,
    pub muted: bool,
    pub pinned_at: Option<String>,
    pub peer_read_seq: Option<i64>,
    pub peer_read_at: Option<String>,
    pub peer_joined_at: Option<String>,
    pub request_id: String,
    pub request_role: String,
    pub extra_conversation_unread: bool,
    /// Always present in `sync()`; absent from the room in `room` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_epoch: Option<String>,
    /// Always present in `sync()`; absent from the room in `room` events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_version: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncResponse {
    pub rooms: Vec<Room>,
    pub rev: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reaction {
    pub seq: i64,
    /// `client_msg_id` / message id the reaction is attached to.
    pub target_id: String,
    pub part: i64,
    pub actor: String,
    pub emoji: String,
    /// Unix seconds
    pub at: f64,
}

/// A reaction listed inline on a message of a [`HistoryWindow`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InlineReaction {
    pub part: i64,
    pub actor: String,
    pub emoji: String,
    pub at: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileCard {
    pub user_id: String,
    pub handle: String,
    pub display_name: String,
    pub character_name: String,
    pub avatar_url: String,
    pub character_avatar_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestOption {
    pub id: String,
    pub label: String,
}

/// A question the character asks you (`agent_request`), answered through a card.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRequest {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<RequestOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageContext {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_index: Option<i64>,
}

impl MessageContext {
    /// The context of a message sent from an offer on the Explore tab. The message's text is
    /// the offer's `cta_label`.
    pub fn offer(offer_id: impl Into<String>) -> Self {
        Self {
            kind: "offer".into(),
            id: offer_id.into(),
            quote: None,
            button_index: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplyTo {
    pub target_id: String,
    pub part: i64,
    pub text: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageMeta {
    pub w: i64,
    pub h: i64,
    pub blurhash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMeta {
    pub name: String,
    pub size: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppReferenceMember {
    pub kind: String,
    #[serde(flatten)]
    pub profile: ProfileCard,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppReferenceCard {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileCard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<AppReferenceMember>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageNotice {
    pub event: String,
    pub catalog_id: String,
    pub ticket_fraction_pct: f64,
    pub tickets_remaining: i64,
    pub label: String,
    pub period: String,
}

/// A stored room message. Optional fields appear only on some messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryItem {
    /// `user`, `assistant`, `agent_request`, `subagent`, …
    pub role: String,
    /// Your messages use the `client_msg_id` they were sent with.
    pub id: String,
    /// Character replies wrap each chat bubble in `<bubble>…</bubble>`; the text may use Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Unix seconds
    pub at: f64,
    pub seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Offsets of the message parts. Reactions and replies point at a part with `part`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<i64>>,
    /// In rooms between characters: whether your character wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mine: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<AgentRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<MessageContext>,
    /// TypeScript: optional and nullable. A JSON `null` reads as `None` too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<ReplyTo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Attachment paths (`attachments/<date>/<file>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
    /// Image path to URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_images: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_meta: Option<BTreeMap<String, ImageMeta>>,
    /// File path to URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_files: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_meta: Option<BTreeMap<String, FileMeta>>,
    /// `pickle://` link to its label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_references: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_reference_cards: Option<BTreeMap<String, AppReferenceCard>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<MessageNotice>,
    /// Only in [`HistoryWindow`]: the message's reactions, inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<InlineReaction>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CardState {
    pub seq: i64,
    /// The `agent_request` message the state belongs to.
    pub id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
}

/// `GET /api/rooms/{room}/history`: the newest messages, each with its reactions inline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryWindow {
    pub v: i64,
    pub messages: Vec<HistoryItem>,
    pub reactions: Vec<Reaction>,
    pub card_states: Vec<CardState>,
    pub window_max_seq: i64,
    pub head_seq: i64,
    pub log_epoch: String,
    pub projection_version: i64,
    pub read_seq: i64,
}

/// `GET /api/rooms/{room}/history/page`: messages before a sequence number, newest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub v: i64,
    pub messages: Vec<HistoryItem>,
    pub reactions: Vec<Reaction>,
    pub card_states: Vec<CardState>,
    pub has_more: bool,
    /// The `before_seq` of the page before this one.
    pub next_before: i64,
}

/// `GET /api/rooms/{room}/history/delta`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryDelta {
    pub v: i64,
    pub items: Vec<HistoryItem>,
    pub reactions: Vec<Reaction>,
    pub card_states: Vec<CardState>,
    pub next_after: i64,
    pub has_more: bool,
    pub head_seq: i64,
    pub log_epoch: String,
    pub projection_version: i64,
    pub read_seq: i64,
}

/// `POST /api/rooms/{room}/messages`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub status: String,
    pub responder: String,
    /// `null` in every observed response.
    pub seq: Option<i64>,
    /// The id the message was sent with. Set by the client, not by the server.
    #[serde(default)]
    pub client_msg_id: String,
}

// ---- Phone calls ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhoneCallLine {
    pub id: String,
    pub line_id: String,
    /// `me`, `them`, …
    pub speaker: String,
    pub text: String,
    /// Keypad input represented by this line, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    pub joins_previous: bool,
    pub joins_next: bool,
}

/// `GET /api/phone-calls/{call_id}`: call state and the transcript collected so far.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhoneCallTranscript {
    /// `phone_call`
    pub kind: String,
    pub call_id: String,
    pub title: String,
    pub subtitle_lead: String,
    pub counterpart_label: String,
    pub subtitle_tail: String,
    pub started_label: String,
    pub live: bool,
    pub hangup_requested: bool,
    pub footer: String,
    pub lines: Vec<PhoneCallLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_epoch_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at_epoch_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_epoch_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

// ---- Social ----

/// Scope names of `GET /api/social/snapshot`, in the order the app sends them.
pub const ALL_SOCIAL_SCOPES: [&str; 5] = ["activity", "blocked", "friends", "invite", "requests"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Friend {
    #[serde(flatten)]
    pub user: UserSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_level: Option<i64>,
    /// ISO 8601
    pub since: String,
    pub relation_label: String,
    pub speech_style: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FriendRequest {
    #[serde(flatten)]
    pub user: UserSummary,
    /// ISO 8601
    pub requested_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InviteUser {
    #[serde(flatten)]
    pub user: UserSummary,
    pub character_level: i64,
    pub joined_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InviteMember {
    pub user: InviteUser,
    pub joined_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FriendsScope {
    pub friends: Vec<Friend>,
    pub birthdays_this_week: Vec<Value>,
    pub friends_reward_granted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestsScope {
    pub incoming: Vec<Value>,
    pub outgoing: Vec<FriendRequest>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockedScope {
    pub blocked: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InviteScope {
    pub invite_only: bool,
    pub url: String,
    pub code: String,
    pub invites_used: i64,
    pub invites_total: i64,
    pub signups: Vec<InviteMember>,
    pub inviters: Vec<InviteMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityScope {
    pub unread: i64,
}

/// One scope's answer, with its own HTTP-like status.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeResult<T> {
    pub status: i64,
    pub data: T,
}

/// Only the requested scopes are present.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SocialScopes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friends: Option<ScopeResult<FriendsScope>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<ScopeResult<RequestsScope>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<ScopeResult<BlockedScope>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<ScopeResult<InviteScope>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<ScopeResult<ActivityScope>>,
}

/// `GET /api/social/snapshot`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SocialSnapshot {
    pub v: i64,
    pub scopes: SocialScopes,
}

// ---- Screens (server-driven UI data) ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffItem {
    pub name: String,
    pub title: String,
    /// `done`, `canceled`, `running`, `planned`, …
    pub state: String,
    /// Present on completed and older entries; active entries may use the log and phone fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    pub requested_at_epoch_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_epoch_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    pub time_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_log_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_log_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_status_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_answered_at_epoch_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffSection {
    pub key: String,
    pub date_label: String,
    pub items: Vec<HandoffItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffPlanBlock {
    /// `prose` in the captured response; other block types remain readable.
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffLogEntry {
    /// `report`, `page`, `shell`, …; the server may add kinds without a client update.
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffLogNode {
    /// `report`, `start`, `page`, `shell`, `web`, `browser`, …
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `GET /api/screens/handoff?name=`: one task's plan and progress log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffScreen {
    pub kind: String,
    pub state: String,
    pub title: String,
    pub requested_at_epoch_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_log_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_log_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_estimate_pct: Option<f64>,
    #[serde(default)]
    pub plan_blocks: Vec<HandoffPlanBlock>,
    #[serde(default)]
    pub log_entries: Vec<HandoffLogEntry>,
    #[serde(default)]
    pub log_nodes: Vec<HandoffLogNode>,
}

/// `GET /api/screens/handoffs`: tasks and phone calls the character ran.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffsScreen {
    pub kind: String,
    pub items: Vec<HandoffItem>,
    pub handoff_estimate_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_subtitle: Option<String>,
    pub active_items: Vec<HandoffItem>,
    pub recent_items: Vec<HandoffItem>,
    pub active_count: i64,
    pub recent_count: i64,
    pub sections: Vec<HandoffSection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraConversation {
    /// `<kind>:<id>`, e.g. `phone:<call_id>`.
    pub id: String,
    /// `phone`, `clone`, …
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    /// ISO 8601
    pub at: String,
    pub date_label: String,
    pub room_id: String,
    pub call_id: String,
    pub unread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seq: Option<i64>,
}

/// `GET /api/screens/extra-conversations`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraConversationsScreen {
    pub kind: String,
    pub items: Vec<ExtraConversation>,
    pub counterpart_name: String,
}

/// Something the character offers to do for you, shown on the Explore tab.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub offer_id: String,
    /// What the character would do, in its own words. Empty for some suggestions.
    pub title: String,
    pub detail: String,
    /// The text of the button, which is also the message that asks for the offer.
    pub cta_label: String,
    /// Address of the offer's picture.
    pub icon_image: String,
    /// An SF Symbol name (`banknote`, `house`, `airplane`, …), for when there is no picture.
    pub icon_symbol: String,
    /// `money`, `shopping`, `health`, `people`, `work`, `travel`, `home` or `fun`.
    pub area: String,
    pub areas: Vec<String>,
    /// `75,776 people asked for this`, or empty.
    pub accepted_users_label: String,
    /// How the character goes about it. Missing in `ranked_items`.
    #[serde(default)]
    pub steps: Vec<String>,
    /// The section an offer of `more_items` or `ranked_items` belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExploreSection {
    /// `popular`, `character`, `trending`, `money_back`, `shopping`, …
    pub section_id: String,
    pub title: String,
    pub items: Vec<Offer>,
    /// How many of `items` show before the section is expanded, and how many more it holds.
    pub preview_count: i64,
    pub expand_count: i64,
    pub areas: Vec<String>,
}

/// `GET /api/screens/explore-sections`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExploreSectionsScreen {
    pub kind: String,
    pub areas: Vec<String>,
    pub popular_ids: Vec<String>,
    pub sections: Vec<ExploreSection>,
    pub more_items: Vec<Offer>,
    pub ranked_items: Vec<Offer>,
}

/// `GET /api/screens/message-search`: the messages that contain the query, newest first. The
/// captures never held more than 30.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageSearchScreen {
    pub kind: String,
    pub items: Vec<MessageSearchItem>,
}

/// One search result. Only results in the chat with your character were captured; the four
/// fields that say where a result is default to empty, in case other kinds leave them out.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageSearchItem {
    /// `chat:<seq>` for a chat message.
    pub id: String,
    /// `chat` is the only kind seen.
    pub kind: String,
    /// The snippet, cut into runs that match the query and runs that do not.
    pub segments: Vec<MessageSearchSegment>,
    /// What the result is, for display: `Chat`.
    pub label: String,
    /// The day of the message, for display: `Nov 16`.
    pub date_label: String,
    /// Empty for the chat with your character.
    #[serde(default)]
    pub room_id: String,
    #[serde(default)]
    pub call_id: String,
    /// The message's sequence number in its room.
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub line_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSearchSegment {
    pub text: String,
    /// Whether this run matches the query.
    #[serde(rename = "match")]
    pub is_match: bool,
}

/// `GET /api/screens/roulette-promo`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoulettePromoScreen {
    pub kind: String,
    pub enabled: bool,
    pub announce: bool,
}

// ---- Device ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionSnapshot {
    /// `apple-calendar`, `contacts`, `location`, `notifications`, `photos`, …
    pub source: String,
    /// `granted`, `available`, `needs_permission`, `undetermined`, `pending_entitlement`, …
    pub state: String,
    /// ISO 8601 without fractional seconds, e.g. `2026-11-16T10:14:36Z`.
    pub reported_at: String,
}

// ---- UI definitions ----

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecureProgram {
    pub source: String,
    pub signatures: BTreeMap<String, String>,
    pub release: JsonObject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StringsEntry {
    pub hash: String,
}

/// `GET /api/ui-definitions`: the server-driven UI bundle (about 2 MB).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiDefinitions {
    pub tokens: JsonObject,
    pub styles: JsonObject,
    pub motion: JsonObject,
    pub navigation: JsonObject,
    pub components: JsonObject,
    pub screens: JsonObject,
    /// Example data for each screen data kind, shaped like the live responses.
    pub samples: JsonObject,
    /// The JavaScript program that runs the end-to-end credential protocol on device.
    pub secure_program: SecureProgram,
    pub strings: BTreeMap<String, StringsEntry>,
    pub assets_base_url: String,
    #[serde(flatten)]
    pub rest: JsonObject,
}

// ---- Secure (enclave credential protocol) ----

/// A signed statement: base64url JSON `body` and its signature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedStatement {
    pub body: String,
    pub sig: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecureKey {
    pub key_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecureAccount {
    pub key: SecureKey,
    pub user_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnclaveNode {
    pub v: i64,
    /// `nitro`, …
    pub platform: String,
    /// Base64url COSE_Sign1 AWS Nitro attestation document.
    pub document: String,
    pub node: String,
    pub challenge: String,
    pub release: String,
    pub challenge_statement: SignedStatement,
    pub pod: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnclaveNodes {
    pub nodes: Vec<EnclaveNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeStatus {
    pub head: SignedStatement,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogNodeSummary {
    pub node: String,
    pub release: String,
    pub live: bool,
    pub max_seq: i64,
    pub final_head: Option<SignedStatement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogNodes {
    pub nodes: Vec<LogNodeSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ledger {
    pub key_id: String,
    pub nonce: String,
    pub ct: String,
    pub updated_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedLedger {
    pub ledger: Ledger,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfers {
    pub transfers: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStoreNodeMarker {
    pub node: String,
    pub key_marker: String,
    pub version_id_marker: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStoreListingsRequest {
    pub discovery: bool,
    pub nodes: Vec<LogStoreNodeMarker>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStoreListing {
    pub prefix: String,
    /// Presigned S3 `ListObjectVersions` URL, valid for about 5 minutes.
    pub list_url: String,
    pub delimiter: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeLogStoreListing {
    pub node: String,
    #[serde(flatten)]
    pub listing: LogStoreListing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogStoreListings {
    pub bucket: String,
    pub region: String,
    pub expires_ms: i64,
    pub discovery: LogStoreListing,
    pub nodes: Vec<NodeLogStoreListing>,
}
