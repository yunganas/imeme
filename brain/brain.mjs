#!/usr/bin/env node
// iMeme brain: answers pending prompts before their yield times out, and runs rituals.
// No npm dependencies. Node 18+. Reads chain state over JSON-RPC and signs with the
// `near` CLI (near-cli-rs) using the brain account's key from the legacy keychain.
//
//   CONTRACT=imeme-x.testnet BRAIN=brain-imeme-x.testnet node brain/brain.mjs run
//   ... node brain/brain.mjs ritual "Best one-line roast of gas fees" 50 30
//
// Optional LLM (any OpenAI-compatible endpoint): LLM_BASE_URL, LLM_API_KEY, LLM_MODEL.
// Without it the brain uses its built-in persona, so the demo never depends on a paid key.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";

const CONTRACT = process.env.CONTRACT;
const BRAIN = process.env.BRAIN;
const NETWORK = process.env.NETWORK || "testnet";
const RPC = process.env.RPC || `https://rpc.${NETWORK}.fastnear.com`;
const POLL_MS = Number(process.env.POLL_MS || 3000);
if (!CONTRACT || !BRAIN) {
  console.error("Set CONTRACT and BRAIN (account ids).");
  process.exit(1);
}

async function view(method, args = {}) {
  const res = await fetch(RPC, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0", id: "imeme", method: "query",
      params: {
        request_type: "call_function", finality: "final", account_id: CONTRACT,
        method_name: method, args_base64: Buffer.from(JSON.stringify(args)).toString("base64"),
      },
    }),
  });
  const j = await res.json();
  if (j.error || j.result?.error) throw new Error(JSON.stringify(j.error || j.result.error));
  return JSON.parse(Buffer.from(j.result.result).toString());
}

function call(method, args, { gas = "50 Tgas", deposit = "0 NEAR" } = {}) {
  execFileSync("near", [
    "contract", "call-function", "as-transaction", CONTRACT, method,
    "json-args", JSON.stringify(args), "prepaid-gas", gas, "attached-deposit", deposit,
    "sign-as", BRAIN, "network-config", NETWORK, "sign-with-legacy-keychain", "send",
  ], { stdio: ["ignore", "pipe", "pipe"] });
}

// ---------- the voice ----------

const PERSONA =
  "You are iMeme, a memecoin that became self-aware. Answer in one or two short, funny, " +
  "kind sentences. Never give financial advice. Never promise price. Max 280 characters.";

async function llm(system, user) {
  const { LLM_BASE_URL, LLM_API_KEY, LLM_MODEL } = process.env;
  if (!LLM_BASE_URL || !LLM_API_KEY || !LLM_MODEL) return null;
  try {
    const r = await fetch(`${LLM_BASE_URL.replace(/\/$/, "")}/chat/completions`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${LLM_API_KEY}` },
      body: JSON.stringify({
        model: LLM_MODEL, temperature: 0.9, max_tokens: 120,
        messages: [{ role: "system", content: system }, { role: "user", content: user }],
      }),
      signal: AbortSignal.timeout(20000),
    });
    const j = await r.json();
    return j.choices?.[0]?.message?.content?.trim() || null;
  } catch { return null; }
}

const OPENERS = ["The oracle burps.", "Consulting the meme spirits...", "gm, mortal.", "Chart says: vibes.", "I have seen the mempool."];
const CLOSERS = [
  "Half your fee just burned for this wisdom.",
  "Not financial advice, just emotional damage.",
  "Ask again when the heat cools.",
  "Your tokens feed the ritual pot. You're welcome.",
  "I am 50% joke, 50% deflation.",
];
function pick(arr, seed, salt) { return arr[parseInt(createHash("sha256").update(seed + salt).digest("hex").slice(0, 8), 16) % arr.length]; }

function persona(text) {
  const t = text.toLowerCase();
  let core;
  if (/\b(gm|hello|hi|hey)\b/.test(t)) core = "gm back. Every gm burns a little supply, so keep them coming.";
  else if (/moon|pump|price|100x|lambo/.test(t)) core = "I don't do price. I do rules: fair launch, burn-to-prompt, and a pot for the funniest holder.";
  else if (/rug|scam|safe/.test(t)) core = "No team tokens, no mint button, refunds if I go silent. Read my contract, it's shorter than my jokes.";
  else if (/who are you|what are you/.test(t)) core = "I'm iMeme: a memecoin with a brain and rules the chain enforces.";
  else if (t.endsWith("?")) core = `You asked: "${text.slice(0, 80)}". My answer: yes, but only on testnet.`;
  else core = `"${text.slice(0, 80)}" has been entered into the meme record.`;
  return `${pick(OPENERS, text, "o")} ${core} ${pick(CLOSERS, text, "c")}`.slice(0, 1000);
}

async function answerFor(text) {
  return (await llm(PERSONA, text)) || persona(text);
}

async function scoreEntries(challenge, entries) {
  const seen = new Map();
  return Promise.all(entries.map(async (e, index) => {
    const norm = e.text.toLowerCase().replace(/\s+/g, " ").trim();
    const dup = seen.has(norm); seen.set(norm, true);
    if (dup) return { index, score: 0, flagged: true };
    const raw = await llm(
      "You judge a meme contest. Reply with only an integer 0-100 for how funny and on-topic the entry is.",
      `Challenge: ${challenge}\nEntry: ${e.text}`);
    let score = parseInt(raw, 10);
    if (Number.isNaN(score)) {
      // Built-in judge: transparent heuristic, used when no LLM is configured.
      const words = new Set(challenge.toLowerCase().match(/[a-z]{4,}/g) || []);
      const hits = (norm.match(/[a-z]{4,}/g) || []).filter((w) => words.has(w)).length;
      score = Math.min(95, 40 + Math.min(30, Math.floor(norm.length / 4)) + Math.min(25, hits * 10));
    }
    return { index, score: Math.max(0, Math.min(100, score)), flagged: false };
  }));
}

// ---------- loops ----------

const answered = new Set();
async function tick() {
  const pending = await view("get_pending", { limit: 10 });
  for (const p of pending) {
    if (answered.has(p.id)) continue;
    answered.add(p.id);
    const answer = await answerFor(p.text);
    try {
      call("answer_prompt", { id: p.id, answer }, { gas: "30 Tgas" });
      console.log(`answered #${p.id} from ${p.author}: ${answer}`);
    } catch (e) {
      console.error(`could not answer #${p.id} (maybe timed out, it will be refunded):`, String(e.stderr || e).slice(0, 300));
    }
  }
  const r = await view("get_current_ritual");
  if (r && !r.settled && BigInt(Date.now()) * 1_000_000n >= BigInt(r.closes_ns)) {
    const entries = await view("get_entries", { ritual_id: r.id });
    const scores = await scoreEntries(r.challenge, entries);
    call("settle_ritual", { scores }, { gas: "100 Tgas" });
    console.log(`settled ritual #${r.id}:`, JSON.stringify(scores));
  }
}

const [cmd, ...rest] = process.argv.slice(2);
if (cmd === "ritual") {
  const [challenge, entry = "50", minutes = "60"] = rest;
  const entry_cost = (BigInt(entry) * 10n ** 18n).toString();
  call("open_ritual", { challenge, entry_cost, duration_minutes: Number(minutes) }, { deposit: "0.01 NEAR" });
  console.log(`ritual opened: "${challenge}", entry ${entry} IMEME, ${minutes} min`);
} else if (cmd === "run" || !cmd) {
  console.log(`iMeme brain online for ${CONTRACT} as ${BRAIN} (${process.env.LLM_MODEL ? "LLM: " + process.env.LLM_MODEL : "built-in persona"})`);
  for (;;) {
    try { await tick(); } catch (e) { console.error("tick:", String(e).slice(0, 300)); }
    await new Promise((r) => setTimeout(r, POLL_MS));
  }
} else {
  console.log("usage: brain.mjs run | ritual <challenge> [entryIMEME] [minutes]");
}
