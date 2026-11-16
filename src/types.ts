// Wire types of the Pickle character API (https://character-api-prod.picklemem.com).
//
// Every type here was read off real traffic of the iOS app (build 202611071128, version 1.1.19)
// and, for room history, off live responses.
// Field names stay in the server's snake_case so that a response can be passed through untouched.
// A string union that ends in `(string & {})` lists the values seen in traffic; other values exist.

type Open<T extends string> = T | (string & {});

export interface StatusOk {
  status: "ok";
}

export interface ApiErrorBody {
  /** Machine-readable reason, e.g. `token_expired`, `unauthenticated`, `location_disabled`. */
  detail: string;
}

// ---- Account and session ----

export interface Onboarding {
  region_name: string;
  region_waitlist_joined: boolean;
  region_waitlist_phone: string;
  region_waitlist_place: string;
  invite_waitlist_joined: boolean;
  character_named: boolean;
  character_name: string;
  display_name: string;
  profile_avatar_present: boolean;
  suspension_reason: string;
  /** `YYYY-MM-DD` */
  birthday: string;
  consented: boolean;
  consent_marketing: boolean;
  consent_screen_recorded: boolean;
  jaum_glyph: string;
  location_terms_consented: boolean;
  location_terms_withdrawn: boolean;
  model_training_consented: boolean;
  model_training_consent_recorded: boolean;
  name_suggestions: { name: string }[];
}

export interface AccountState {
  user_id: string;
  account_lineage: number;
  account_rev: number;
  entry: Open<"session">;
  onboarding: Onboarding;
  character_room_id: string;
}

export interface RegisterResponse extends AccountState {
  market: Open<"kr">;
  registered: boolean;
}

export interface SessionRequest {
  bgm_library: string[];
  app_version: string;
  /** Hash of the UI vocabulary the app holds; also sent as `X-Pickle-Vocabulary`. */
  vocabulary: string;
  rig_ready: boolean;
  client_connect_id: string;
  /** The app sends `none` when it does not need a LiveKit room joined right away. */
  room_demand: Open<"none">;
  platform: Open<"ios" | "android">;
  app_build: string;
  device_memory_bytes: number;
  establishment_aware: boolean;
  world_ready: boolean;
}

export interface Plan {
  id: Open<"free" | "plus" | "pool20" | "pool50" | "pool100" | "pool200">;
  label: string;
  rank: number;
  credits: number;
  credits_text: string;
  weekly_credits: number;
  weekly_credits_text: string;
  product_ids: string[];
  offered: boolean;
  upgrade_rank: number;
  upgrade_label: string;
  owned_product_id: string;
  owned_price_text: string;
  owned_period_unit: string;
}

export interface CreditUsage {
  allowance: number;
  used: number;
  remaining: number;
  bonus: number;
  exhausted: boolean;
  /** ISO 8601 */
  reset_at: string;
  ticket_credits: number;
  tickets: number;
  tickets_by_kind: Record<string, number>;
  ticket_auto_use: boolean;
  period: Open<"day">;
  balance: number;
  credit_auto_use: boolean;
  auto_use_order: Open<"credits_first">;
  ticket_order: Open<"half_first">;
  credit_spending: boolean;
}

export interface LevelMilestone {
  level: number;
  amount: number;
  full_size: boolean;
}

export interface JellyRewards {
  /** `YYYY-MM-DD` */
  day: string;
  checkin_available: boolean;
  checkin_amount: number;
  talk_seconds: number;
  talk_milestones: unknown[];
  talk_claimable: number;
  friends_reward_day: string;
  notices: unknown[];
  purchase_granted: boolean;
  level_milestones: LevelMilestone[];
  next_level_reward_level: number;
  next_level_reward_amount: number;
}

export interface CharacterStatus {
  color_combo_id: string;
  level: number;
  xp: number;
  xp_next: number;
  /** 0..1 */
  satiety: number;
  portrait_redraw_due: boolean;
  body_color: string;
  line_id: string;
  body_hex: string;
  line_hex: string;
  energy_pct: number;
  energy_exhausted: boolean;
  jelly_count: number;
  jelly_checkin_available: boolean;
  jelly_checkin_amount: number;
  jelly_notice_kind: string;
  jelly_notice_amount: number;
  level_milestones: LevelMilestone[];
  next_level_reward_level: number;
  next_level_reward_amount: number;
}

export interface PortraitReferenceView {
  yaw_deg: number;
  pitch_deg: number;
  /** Face-rig channel values keyed by `<bone>.<property>.<axis>`. */
  channels: Record<string, number>;
}

/**
 * `POST /api/session`. The response carries about 110 keys, most of them client
 * configuration for the 3D world; the ones a client is likely to use are typed.
 */
export interface SessionResponse extends AccountState {
  /** LiveKit room name, `character_prod_user_<user_id>`. */
  room: string;
  livekit_url: string;
  /** LiveKit access token (HS256, about 6 hours). Also the bearer for `POST /api/telemetry`. */
  token: string;
  participant_identity: string;
  session_id: string;
  tester_eligible: boolean;
  admin_eligible: boolean;
  ios_min_build: string;
  android_min_build: string;
  plan: Plan["id"];
  plan_rank: number;
  plans: { items: Plan[]; by_id: Record<string, Plan>; max_rank: number };
  credits: CreditUsage & {
    invite_reward: number;
    friends_reward: number;
    invite_code_claimable: boolean;
    friends_reward_claimable: boolean;
    allowance_free: number;
    allowance_plus: number;
  };
  jelly_count: number;
  jelly_rewards: JellyRewards;
  character_name: string;
  character_status: CharacterStatus;
  character_voice: string;
  character_avatar_url: string;
  market: Open<"kr">;
  display_name: string;
  handle: string;
  profile_avatar_url: string;
  profile_url: string;
  flags: Record<string, boolean>;
  seen_flags: string[];
  legal_documents: {
    document: string;
    name: string;
    version: string;
    url: string;
    updated: boolean;
    kind: string | null;
    reconsent_required: boolean;
    listed: boolean;
  }[];
  [key: string]: unknown;
}

export interface LoginSessionRefresh {
  /** The new access token, valid for 5 minutes. */
  login_session_jwt: string;
  /** When the login session ends (7 days after the refresh), ISO 8601. */
  expires_at: string;
  /** The next refresh token. The one that was sent is spent. */
  login_session_token: string;
}

// ---- Profile and character ----

export interface Profile {
  kind: "profile";
  display_name: string;
  handle: string;
  phone_text: string;
  /** `MM/DD/YYYY` */
  birthday_text: string;
  birthday_month: string;
  birthday_day: string;
  birthday_year: string;
  avatar_url: string;
  profile_url: string;
  phone_dial_code: string;
  phone_country_flag: string;
  phone_region: string;
  model_training_consented: boolean;
}

export interface Character {
  name: string;
  voice: string;
  avatar_url: string;
  status: CharacterStatus;
  map_visibility: Open<"everyone" | "friends">;
  roam_radius_m: number;
  roam_radius_mi: number;
  outing_radius_m: number;
  portrait_reference_views: PortraitReferenceView[];
  body_kind: Open<"jaum" | "critter">;
  jaum_glyph: string;
  jaum_material: string;
  jaum_color: string;
  solid_color: string;
  critter_species_name: string;
  /** ISO 8601 */
  critter_drawn_at: string;
  jars_remaining: number;
  jaum_portrait_version: number;
}

// ---- Rooms and chat ----

export interface UserSummary {
  user_id: string;
  handle: string;
  display_name: string;
  avatar_url: string;
  character_avatar_url: string;
  character_name: string;
}

export type RoomKind = Open<"character" | "character_pair">;

export interface Room {
  room_id: string;
  kind: RoomKind;
  title: string;
  /** `null` for the room with your own character. */
  peer: UserSummary | null;
  member_count: number;
  last_seq: number;
  read_seq: number;
  role: Open<"member">;
  /** ISO 8601 */
  last_event_at: string;
  /** True for rooms you only watch, such as your character talking with a friend's character. */
  observer: boolean;
  unread: number;
  muted: boolean;
  pinned_at: string | null;
  peer_read_seq: number | null;
  peer_read_at: string | null;
  peer_joined_at: string | null;
  request_id: string;
  request_role: Open<"" | "requester" | "target">;
  extra_conversation_unread: boolean;
  log_epoch: string;
  projection_version: number;
}

export interface SyncResponse {
  rooms: Room[];
  rev: number;
}

export interface Reaction {
  seq: number;
  /** `client_msg_id` / message id the reaction is attached to. */
  target_id: string;
  part: number;
  actor: Open<"assistant" | "user">;
  emoji: string;
  /** Unix seconds */
  at: number;
}

export interface ProfileCard {
  user_id: string;
  handle: string;
  display_name: string;
  character_name: string;
  avatar_url: string;
  character_avatar_url: string;
}

/** A stored room message. Optional fields appear only on some messages. */
export interface HistoryItem {
  role: Open<"user" | "assistant" | "agent_request" | "subagent">;
  /** Your messages use the `client_msg_id` they were sent with. */
  id: string;
  /** Character replies wrap each chat bubble in `<bubble>…</bubble>`; the text may use Markdown. */
  text?: string;
  /** Unix seconds */
  at: number;
  seq: number;
  channel?: Open<"chat" | "voice">;
  /** Offsets of the message parts. Reactions and replies point at a part with `part`. */
  parts?: number[];
  /** In rooms between characters: whether your character wrote it. */
  mine?: boolean;
  /** A question the character asks you (`agent_request`), answered through a card. */
  request?: {
    kind: Open<"choice" | "card" | "otp">;
    question?: string;
    options?: { id: string; label: string }[];
    images?: string[];
    recommended?: number;
    service?: string;
  };
  state?: Open<"pending">;
  context?: { kind: Open<"offer" | "button">; id: string; quote?: string; button_index?: number };
  reply_to?: { target_id: string; part: number; text: string; role: string; kind?: string; path?: string } | null;
  path?: string;
  title?: string;
  /** Attachment paths (`attachments/<date>/<file>`). */
  attachments?: string[];
  /** Image path to URL. */
  inline_images?: Record<string, string>;
  image_meta?: Record<string, { w: number; h: number; blurhash: string }>;
  /** File path to URL. */
  inline_files?: Record<string, string>;
  file_meta?: Record<string, { name: string; size: number; pages?: number }>;
  /** `pickle://` link to its label. */
  app_references?: Record<string, string>;
  app_reference_cards?: Record<
    string,
    {
      kind: string;
      user_id?: string;
      profile?: ProfileCard;
      room_kind?: string;
      members?: (ProfileCard & { kind: string })[];
    }
  >;
  notice?: {
    event: string;
    catalog_id: string;
    ticket_fraction_pct: number;
    tickets_remaining: number;
    label: string;
    period: string;
  };
}

export interface CardState {
  seq: number;
  /** The `agent_request` message the state belongs to. */
  id: string;
  state: string;
  option_id?: string;
}

export interface HistoryDelta {
  v: 1;
  items: HistoryItem[];
  reactions: Reaction[];
  card_states: CardState[];
  next_after: number;
  has_more: boolean;
  head_seq: number;
  log_epoch: string;
  projection_version: number;
  read_seq: number;
}

export interface SendMessageResponse {
  status: "ok";
  responder: Open<"chat">;
  seq: number | null;
}

// ---- Social ----

export interface Friend extends UserSummary {
  character_level?: number;
  /** ISO 8601 */
  since: string;
  relation_label: string;
  speech_style: string;
}

export interface FriendRequest extends UserSummary {
  /** ISO 8601 */
  requested_at: string;
}

export interface InviteMember {
  user: UserSummary & { character_level: number; joined_at: string };
  joined_at: string;
}

export interface SocialScopes {
  friends: { friends: Friend[]; birthdays_this_week: unknown[]; friends_reward_granted: boolean };
  /** `incoming` was empty in the studied traffic; it is assumed to mirror `outgoing`. */
  requests: { incoming: FriendRequest[]; outgoing: FriendRequest[] };
  /** Empty in the studied traffic. */
  blocked: { blocked: unknown[] };
  invite: {
    invite_only: boolean;
    url: string;
    code: string;
    invites_used: number;
    invites_total: number;
    signups: InviteMember[];
    inviters: InviteMember[];
  };
  activity: { unread: number };
}

export type SocialScope = keyof SocialScopes;

/** Each scope is answered on its own, with its own HTTP-like status. */
export interface SocialSnapshot<S extends SocialScope = SocialScope> {
  v: 1;
  scopes: { [K in S]: { status: number; data: SocialScopes[K] } };
}

// ---- Screens (server-driven UI data) ----

export interface HandoffItem {
  name: string;
  title: string;
  state: Open<"done" | "canceled" | "running" | "planned">;
  subtitle: string;
  requested_at_epoch_ms: number;
  finished_at_epoch_ms?: number;
  result_text: string;
  time_text: string;
  phone_call_id?: string;
}

export interface HandoffsScreen {
  kind: "list";
  items: HandoffItem[];
  handoff_estimate_pct: number;
  active_items: HandoffItem[];
  recent_items: HandoffItem[];
  active_count: number;
  recent_count: number;
  sections: { key: string; date_label: string; items: HandoffItem[] }[];
}

export interface ExtraConversation {
  /** `<kind>:<id>`, e.g. `phone:7a7539eef672`. */
  id: string;
  kind: Open<"phone" | "clone">;
  title: string;
  subtitle: string;
  /** ISO 8601 */
  at: string;
  date_label: string;
  room_id: string;
  call_id: string;
  unread: boolean;
  first_seq?: number;
}

export interface ExtraConversationsScreen {
  kind: "extra-conversations";
  items: ExtraConversation[];
  counterpart_name: string;
}

export interface RoulettePromoScreen {
  kind: "roulette-promo";
  enabled: boolean;
  announce: boolean;
}

/** Screen kinds whose `/api/screens/<kind>` response was present in traffic. */
export interface KnownScreens {
  handoffs: HandoffsScreen;
  "extra-conversations": ExtraConversationsScreen;
  "roulette-promo": RoulettePromoScreen;
}

// ---- Device ----

export type PermissionSource = Open<
  | "apple-calendar"
  | "apple-music"
  | "contacts"
  | "finance"
  | "location"
  | "location_background"
  | "notifications"
  | "photos"
  | "reminders"
  | "screen-time"
  | "steps"
>;

export type PermissionState = Open<
  "granted" | "available" | "needs_permission" | "undetermined" | "pending_entitlement"
>;

export interface PermissionSnapshot {
  source: PermissionSource;
  state: PermissionState;
  /** ISO 8601 without fractional seconds, e.g. `2026-11-16T10:14:36Z`. */
  reported_at: string;
}

// ---- UI definitions ----

/** `GET /api/ui-definitions`: the server-driven UI bundle (about 2 MB). */
export interface UiDefinitions {
  tokens: Record<string, unknown>;
  styles: Record<string, unknown>;
  motion: Record<string, unknown>;
  navigation: Record<string, unknown>;
  components: Record<string, unknown>;
  screens: Record<string, unknown>;
  /** Example data for each screen data kind, shaped like the live responses. */
  samples: Record<string, unknown>;
  /** The JavaScript program that runs the end-to-end credential protocol on device. */
  secure_program: { source: string; signatures: Record<string, string>; release: Record<string, unknown> };
  strings: Record<string, { hash: string }>;
  assets_base_url: string;
  [key: string]: unknown;
}

// ---- Secure (enclave credential protocol) ----

/** A signed statement: base64url JSON `body` and its signature. */
export interface SignedStatement {
  body: string;
  sig: string;
}

export interface SecureAccount {
  key: { key_id: string };
  user_id: string;
}

export interface EnclaveNode {
  v: 1;
  platform: Open<"nitro">;
  /** Base64url COSE_Sign1 AWS Nitro attestation document. */
  document: string;
  node: string;
  challenge: string;
  release: string;
  challenge_statement: SignedStatement;
  pod: number;
}

export interface LogNodeSummary {
  node: string;
  release: string;
  live: boolean;
  max_seq: number;
  final_head: SignedStatement | null;
}

export interface SharedLedger {
  ledger: { key_id: string; nonce: string; ct: string; updated_ms: number };
}

export interface LogStoreListingsRequest {
  discovery: boolean;
  nodes: { node: string; key_marker: string; version_id_marker: string | null }[];
}

export interface LogStoreListing {
  prefix: string;
  /** Presigned S3 `ListObjectVersions` URL, valid for about 5 minutes. */
  list_url: string;
  delimiter: string;
}

export interface LogStoreListings {
  bucket: string;
  region: string;
  expires_ms: number;
  discovery: LogStoreListing;
  nodes: (LogStoreListing & { node: string })[];
}
