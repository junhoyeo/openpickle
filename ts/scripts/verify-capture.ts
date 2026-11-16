#!/usr/bin/env bun
// Replays a Proxyman capture (.proxymanlogv2) through PickleClient and checks that, for every
// recorded request, the client sends the same method, path, query, JSON body and app headers,
// and that it parses the recorded response. Nothing goes to the network: fetch is replaced by
// one that answers with the recorded response.
//
//   bun scripts/verify-capture.ts ~/Downloads/character-api-prod.picklemem.com_<date>.proxymanlogv2

import { execFileSync } from "node:child_process";
import { isDeepStrictEqual } from "node:util";
import { PickleApiError, PickleClient, type ClientProfile } from "../src/client.ts";

interface Flow {
  name: string;
  method: string;
  path: string;
  query: string;
  headers: Map<string, string>;
  body: string;
  status: number;
  responseType: string;
  responseBody: Uint8Array<ArrayBuffer>;
}

interface Sent {
  method: string;
  path: string;
  query: string;
  headers: Map<string, string>;
  body: string;
}

type Call = (client: PickleClient, flow: Flow, json: any, params: string[]) => Promise<unknown>;

const drain = async (events: AsyncIterable<unknown>) => {
  const all = [];
  for await (const event of events) all.push(event);
  return all;
};

const routes: [method: string, path: RegExp, call: Call][] = [
  ["POST", /^\/api\/login-sessions\/phone-code$/, (c, _f, j) =>
    c.requestPhoneCode(j.phone, { region: j.region, carrier: j.carrier })],
  ["POST", /^\/api\/login-sessions$/, (c, _f, j) => c.verifyPhoneCode(j.pending_id, j.code)],
  ["POST", /^\/api\/login-sessions\/refresh$/, (c, _f, j) => c.refreshLoginSession(j.login_session_token)],
  ["POST", /^\/api\/account\/register$/, (c) => c.registerAccount()],
  ["POST", /^\/api\/session$/, (c, _f, json) => c.startSession(json)],
  ["GET", /^\/api\/sync$/, (c) => c.sync()],
  ["GET", /^\/api\/events$/, (c) => drain(c.events())],
  ["GET", /^\/api\/rooms\/([^/]+)\/history$/, (c, _f, _j, [room]) => c.getHistory(room!)],
  ["GET", /^\/api\/rooms\/([^/]+)\/history\/delta$/, (c, f, _j, [room]) => {
    const q = new URLSearchParams(f.query);
    return c.getHistoryDelta(room!, { afterSeq: Number(q.get("after_seq")), limit: Number(q.get("limit")) });
  }],
  ["GET", /^\/api\/rooms\/([^/]+)\/history\/page$/, (c, f, _j, [room]) => {
    const q = new URLSearchParams(f.query);
    return c.getHistoryPage(room!, { beforeSeq: Number(q.get("before_seq")), limit: Number(q.get("limit")) });
  }],
  ["POST", /^\/api\/rooms\/([^/]+)\/messages$/, (c, _f, j, [room]) =>
    c.sendMessage(room!, j.text, { clientMsgId: j.client_msg_id, inCall: j.in_call, context: j.context })],
  ["PUT", /^\/api\/rooms\/([^/]+)\/read$/, (c, _f, j, [room]) => c.markRead(room!, j.seq)],
  ["GET", /^\/api\/character$/, (c) => c.getCharacter()],
  ["GET", /^\/api\/profile$/, (c, f) => c.getProfile(new URLSearchParams(f.query).get("region")!)],
  ["GET", /^\/api\/social\/snapshot$/, (c, f) =>
    c.getSocialSnapshot(new URLSearchParams(f.query).get("scopes")!.split(",") as never)],
  ["GET", /^\/api\/screens\/message-search$/, (c, f) => c.searchMessages(new URLSearchParams(f.query).get("query")!)],
  ["GET", /^\/api\/screens\/explore-sections$/, (c) => c.getExploreSections()],
  ["GET", /^\/api\/screens\/(handoffs|extra-conversations|roulette-promo)$/, (c, _f, _j, [kind]) =>
    c.getScreen(kind!)],
  ["POST", /^\/api\/offers\/impressions$/, (c, _f, j) => c.reportOfferImpressions(j.offer_ids)],
  ["PUT", /^\/api\/device\/permissions$/, (c, _f, j) => c.reportPermissions(j.snapshots)],
  ["POST", /^\/api\/device\/location\/token$/, (c) => c.requestLocationToken()],
  ["GET", /^\/api\/ui-definitions$/, (c) => c.getUiDefinitions()],
  ["GET", /^\/api\/secure\/account$/, (c) => c.getSecureAccount()],
  ["GET", /^\/api\/secure\/nodes$/, (c, f) => c.getEnclaveNodes(new URLSearchParams(f.query).get("nonce")!)],
  ["POST", /^\/api\/secure\/nodes\/([^/]+)\/status$/, (c, _f, j, [node]) => c.getNodeStatus(node!, j.nonce)],
  ["GET", /^\/api\/secure\/log\/nodes$/, (c) => c.getLogNodes()],
  ["POST", /^\/api\/secure\/log\/store\/listings$/, (c, _f, j) => c.getLogStoreListings(j)],
  ["GET", /^\/api\/secure\/ledger$/, (c) => c.getSharedLedger()],
  ["GET", /^\/api\/secure\/transfers$/, (c) => c.getTransfers()],
];

// Hop-by-hop and transport headers the client leaves to fetch.
const TRANSPORT_HEADERS = new Set(["host", "connection", "content-length", "accept-encoding"]);

function readCapture(file: string): Flow[] {
  const names = execFileSync("unzip", ["-Z1", file], { encoding: "utf8" }).split("\n").filter(Boolean);
  const flows: Flow[] = [];
  for (const name of names) {
    const entry = JSON.parse(execFileSync("unzip", ["-p", file, name], { maxBuffer: 1 << 28 }).toString("utf8"));
    const request = entry.request;
    if (request.method.name === "CONNECT" || !entry.response?.status) continue;
    const uri = new URL(request.fullPath);
    const headerMap = (h: any) =>
      new Map<string, string>(h.entries.map((e: any) => [e.key.name.toLowerCase(), e.value]));
    const responseHeaders = headerMap(entry.response.header);
    flows.push({
      name,
      method: request.method.name,
      path: uri.pathname,
      query: uri.search.slice(1),
      headers: headerMap(request.header),
      body: request.bodyData ? Buffer.from(request.bodyData, "base64").toString("utf8") : "",
      status: entry.response.status.code,
      responseType: responseHeaders.get("content-type") ?? "application/json",
      responseBody: new Uint8Array(Buffer.from(entry.response.bodyData ?? "", "base64")),
    });
  }
  return flows;
}

/** A client that identifies itself exactly as the recorded app launch did. */
function clientFor(flow: Flow, json: any, sent: Sent[]): PickleClient {
  const h = flow.headers;
  const profile: Partial<ClientProfile> = {
    userAgent: h.get("user-agent"),
    acceptLanguage: h.get("accept-language"),
    vocabulary: h.get("x-pickle-vocabulary") ?? json?.vocabulary,
  };
  if (h.has("x-client-version")) {
    Object.assign(profile, {
      appVersion: h.get("x-client-version"),
      appBuild: h.get("x-client-build"),
      language: h.get("x-pickle-language"),
      languageSource: h.get("x-pickle-language-source"),
      deviceRegion: h.get("x-pickle-device-region"),
      storefrontCountry: h.get("x-pickle-storefront-country") ?? null,
    });
  }
  if (h.has("x-client-timezone")) profile.timezone = h.get("x-client-timezone");
  for (const key of Object.keys(profile) as (keyof ClientProfile)[]) {
    if (profile[key] === undefined) delete profile[key];
  }
  return new PickleClient({
    accessToken: (h.get("authorization") ?? "Bearer -").replace(/^Bearer /, ""),
    launchId: h.get("x-client-launch-id"),
    deviceId: json?.device_id,
    profile,
    fetch: (async (input: string | URL | Request, init?: RequestInit) => {
      const url = new URL(String(input));
      sent.push({
        method: init?.method ?? "GET",
        path: url.pathname,
        query: url.search.slice(1),
        headers: new Map(Object.entries(init?.headers ?? {}).map(([k, v]) => [k.toLowerCase(), String(v)])),
        body: typeof init?.body === "string" ? init.body : "",
      });
      return new Response(flow.responseBody, { status: flow.status, headers: { "content-type": flow.responseType } });
    }) as typeof fetch,
  });
}

function compare(flow: Flow, sent: Sent | undefined): string[] {
  if (!sent) return ["no request was sent"];
  const problems: string[] = [];
  if (sent.method !== flow.method) problems.push(`method ${sent.method} != ${flow.method}`);
  if (sent.path !== flow.path) problems.push(`path ${sent.path} != ${flow.path}`);
  if (sent.query !== flow.query) problems.push(`query "${sent.query}" != "${flow.query}"`);
  const parse = (text: string) => (text === "" ? undefined : JSON.parse(text));
  if (!isDeepStrictEqual(parse(sent.body), parse(flow.body))) problems.push(`body ${sent.body} != ${flow.body}`);
  for (const [name, value] of flow.headers) {
    if (TRANSPORT_HEADERS.has(name)) continue;
    const ours = sent.headers.get(name);
    if (ours === undefined) problems.push(`missing header ${name}`);
    else if (ours !== value) problems.push(`header ${name}: "${ours}" != "${value}"`);
  }
  for (const name of sent.headers.keys()) {
    if (!flow.headers.has(name)) problems.push(`extra header ${name}`);
  }
  return problems;
}

const file = process.argv[2];
if (!file) {
  console.error("usage: bun scripts/verify-capture.ts <capture.proxymanlogv2>");
  process.exit(2);
}

const flows = readCapture(file).sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
const results = new Map<string, { count: number; problems: Set<string> }>();
const uncovered = new Map<string, number>();

for (const flow of flows) {
  const route = routes.find(([method, path]) => method === flow.method && path.test(flow.path));
  const label = `${flow.method} ${flow.path.replace(/\/rooms\/[^/]+/, "/rooms/{room}").replace(/\/nodes\/[^/]{43}/, "/nodes/{node}")}`;
  if (!route) {
    uncovered.set(label, (uncovered.get(label) ?? 0) + 1);
    continue;
  }
  const json = flow.body ? JSON.parse(flow.body) : undefined;
  const sent: Sent[] = [];
  const problems: string[] = [];
  try {
    await route[2](clientFor(flow, json, sent), flow, json, route[1].exec(flow.path)!.slice(1).map(decodeURIComponent));
    if (flow.status >= 300) problems.push(`status ${flow.status} did not throw`);
  } catch (error) {
    if (!(error instanceof PickleApiError && error.status === flow.status)) problems.push(`threw ${error}`);
  }
  problems.push(...compare(flow, sent[0]));
  if (sent.length > 1) problems.push(`sent ${sent.length} requests`);
  const result = results.get(label) ?? { count: 0, problems: new Set() };
  result.count += 1;
  for (const p of problems) result.problems.add(p);
  results.set(label, result);
}

let failed = 0;
for (const [label, { count, problems }] of results) {
  if (problems.size) failed += 1;
  console.log(`${problems.size ? "FAIL" : "ok  "} ${label} x${count}`);
  for (const p of problems) console.log(`       ${p}`);
}
for (const [label, count] of uncovered) console.log(`skip ${label} x${count} (no client method)`);
console.log(`\n${results.size - failed}/${results.size} endpoints match, ${uncovered.size} not covered`);
process.exit(failed ? 1 : 0);
