#!/usr/bin/env node
/**
 * Sync src-tauri/src/pricing.json from upstream first-party pricing registries:
 *   1. pydantic/genai-prices (primary for Anthropic, OpenAI, Google, Moonshot, Z.ai, MiniMax, DeepSeek)
 *   2. models.dev            (secondary — fills Alibaba/Qwen and DeepSeek day-0 releases)
 *   3. BerriAI/litellm       (secondary — fills DashScope/Qwen and missing first-party variants)
 *   4. pricing-overrides.json (curated exclusions, aliases, regional policies, day-0 overrides)
 *
 * Usage:
 *   node scripts/sync-pricing.mjs --write
 *   node scripts/sync-pricing.mjs --check
 *   node scripts/sync-pricing.mjs --write --summary-file /tmp/pricing-diff.md
 *   node scripts/sync-pricing.mjs --genai-prices /path/gp.json --litellm /path/ll.json --check
 */

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, "..");

export const DEFAULT_SOURCES = {
  genaiPrices:
    "https://raw.githubusercontent.com/pydantic/genai-prices/main/prices/new_data/v2/data.json",
  modelsDev: "https://models.dev/api.json",
  litellm:
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json",
};

const DEFAULT_PRICING_PATH = path.join(REPO_ROOT, "src-tauri/src/pricing.json");
const DEFAULT_OVERRIDES_PATH = path.join(
  REPO_ROOT,
  "src-tauri/src/pricing-overrides.json"
);
const DEFAULT_SOURCES_META_PATH = path.join(
  REPO_ROOT,
  "src-tauri/src/pricing-sources.json"
);

// Strictly gate each provider to its own first-party model ID prefixes so
// reseller / cross-hosted entries (e.g. Claude listed under Google Vertex in
// genai-prices) can never overwrite first-party rates.
const PROVIDER_PREFIXES = {
  anthropic: ["claude-"],
  openai: ["gpt-", "o1", "o3", "o4", "chatgpt-", "codex-"],
  google: ["gemini-"],
  moonshot: ["kimi-", "moonshot-"],
  zai: ["glm-"],
  minimax: ["minimax-", "m2-"],
  qwen: ["qwen", "qwq"],
  deepseek: ["deepseek-"],
};

const GENAI_PROVIDER_MAP = {
  anthropic: "anthropic",
  openai: "openai",
  google: "google",
  moonshotai: "moonshot",
  zai: "zai",
  minimax: "minimax",
  deepseek: "deepseek",
};

const MODELS_DEV_PROVIDER_MAP = {
  anthropic: "anthropic",
  openai: "openai",
  google: "google",
  moonshotai: "moonshot",
  moonshot: "moonshot",
  zai: "zai",
  zhipuai: "zai",
  minimax: "minimax",
  alibaba: "qwen",
  deepseek: "deepseek",
};

const LITELLM_PROVIDER_MAP = {
  anthropic: "anthropic",
  openai: "openai",
  gemini: "google",
  moonshot: "moonshot",
  zai: "zai",
  minimax: "minimax",
  dashscope: "qwen",
  qwen_ai_platform: "qwen",
  qwencloud: "qwen",
  deepseek: "deepseek",
};

const NON_CHAT_SUBSTRINGS = [
  "-tts",
  "-image",
  "-live",
  "-realtime",
  "-audio",
  "-transcribe",
  "whisper",
  "embedding",
  "moderation",
  "speech-",
  "-ocr",
  "-robotics",
  "nano-banana",
  "computer-use",
  "-search-api",
  "babbage",
  "davinci",
  ":batch",
  ":free",
  "dall-e",
  "sora",
  "veo-",
  "imagen-",
  "lyria-",
  "chirp",
  "aqa",
];

// Exclude bare legacy / unversioned or non-LLM tokens that don't belong in the table.
const EXCLUDED_EXACT_IDS = new Set([
  "claude-v1",
  "claude-instant-1",
  "claude-instant-1.2",
  "claude-2",
  "claude-2.0",
  "claude-2.1",
  "search",
  "gpt-3.5-turbo-instruct",
]);

export function roundRate(n) {
  if (typeof n !== "number" || !Number.isFinite(n)) return null;
  return Math.round(n * 1e6) / 1e6;
}

export function stripDateSuffix(id) {
  const parts = id.split("-");
  const last = parts[parts.length - 1];
  if (last && /^\d{8}$/.test(last)) {
    return parts.slice(0, -1).join("-");
  }
  if (parts.length >= 4) {
    const tail = parts.slice(-3).join("-");
    if (/^\d{4}-\d{2}-\d{2}$/.test(tail)) {
      return parts.slice(0, -3).join("-");
    }
  }
  return id;
}

function isDatedSnapshot(id) {
  return (
    stripDateSuffix(id) !== id ||
    /-\d{4}-preview$/.test(id) ||
    /-\d{2}-\d{2}(:.*)?$/.test(id) ||
    /-\d{2}-20\d{2}$/.test(id) ||
    /-(0[1-9]|1[0-2])(0[1-9]|[12]\d|3[01])$/.test(id)
  );
}

function matchesFamily(family, id) {
  const prefixes = PROVIDER_PREFIXES[family];
  if (!prefixes) return false;
  if (!prefixes.some((p) => id.startsWith(p))) return false;
  if (EXCLUDED_EXACT_IDS.has(id)) return false;
  if (NON_CHAT_SUBSTRINGS.some((s) => id.includes(s))) return false;
  if (id.endsWith("-latest")) return false;
  return true;
}

export function normalizeModelPrice(raw) {
  const input = roundRate(raw.input);
  const output = roundRate(raw.output);
  if (input === null || output === null) return null;

  const cache_read =
    raw.cache_read !== undefined && raw.cache_read !== null
      ? roundRate(raw.cache_read)
      : input;
  const cache_create =
    raw.cache_create !== undefined && raw.cache_create !== null
      ? roundRate(raw.cache_create)
      : 0;

  if (cache_read === null || cache_create === null) return null;

  const out = {
    input,
    output,
    cache_read,
    cache_create,
  };

  if (raw.cache_create_1h !== undefined && raw.cache_create_1h !== null) {
    const c1h = roundRate(raw.cache_create_1h);
    if (c1h !== null) out.cache_create_1h = c1h;
  }

  if (
    raw.long_context_above !== undefined &&
    raw.long_context_above !== null &&
    raw.long_context
  ) {
    const lc = normalizeModelPrice(raw.long_context);
    const limit = Math.trunc(raw.long_context_above);
    if (lc && limit > 0) {
      out.long_context_above = limit;
      out.long_context = lc;
    }
  }

  if (raw.off_peak_discount !== undefined && raw.off_peak_discount !== null) {
    const d = roundRate(raw.off_peak_discount);
    if (d !== null && d < 1) out.off_peak_discount = d;
  }

  return out;
}

/**
 * Mirrors src-tauri/src/pricing.rs ModelPrice::valid() and rates_are_plausible().
 */
export function validatePrice(price) {
  if (!price || typeof price !== "object") return "not an object";
  const fields = [
    ["input", price.input],
    ["output", price.output],
    ["cache_read", price.cache_read],
    ["cache_create", price.cache_create],
  ];
  if (price.cache_create_1h !== undefined) {
    fields.push(["cache_create_1h", price.cache_create_1h]);
  }
  for (const [label, rate] of fields) {
    if (typeof rate !== "number" || !Number.isFinite(rate)) {
      return `${label} is not finite`;
    }
    if (rate < 0 || rate > 500) {
      return `${label}=${rate} out of [0, 500]`;
    }
  }
  if (price.output < price.input) {
    return `output (${price.output}) < input (${price.input})`;
  }
  if (price.cache_read > price.input) {
    return `cache_read (${price.cache_read}) > input (${price.input})`;
  }
  if (price.long_context !== undefined) {
    if (
      typeof price.long_context_above !== "number" ||
      !Number.isInteger(price.long_context_above) ||
      price.long_context_above <= 0
    ) {
      return "long_context missing positive integer long_context_above";
    }
    const lcErr = validatePrice(price.long_context);
    if (lcErr) return `long_context: ${lcErr}`;
  }
  if (price.off_peak_discount !== undefined) {
    const d = price.off_peak_discount;
    if (typeof d !== "number" || !Number.isFinite(d) || d < 0 || d > 1) {
      return `off_peak_discount=${d} out of [0, 1]`;
    }
  }
  return null;
}

function gpBase(val) {
  if (typeof val === "number") return val;
  if (val && typeof val === "object" && typeof val.base === "number") {
    return val.base;
  }
  return null;
}

function gpTier(val, start) {
  if (val && typeof val === "object" && Array.isArray(val.tiers)) {
    const hit = val.tiers.find((t) => t && t.start === start);
    if (hit && typeof hit.price === "number") return hit.price;
  }
  return gpBase(val);
}

function gpFirstTierStart(p) {
  for (const key of [
    "input_mtok",
    "output_mtok",
    "cache_read_mtok",
    "cache_write_mtok",
    "cache_write_1h_mtok",
  ]) {
    const v = p?.[key];
    if (v && typeof v === "object" && Array.isArray(v.tiers) && v.tiers.length > 0) {
      const start = v.tiers[0]?.start;
      if (typeof start === "number" && start > 0) return start;
    }
  }
  return null;
}

function gpPriceObjToModelPrice(p, offPeakDiscount = undefined) {
  if (!p || typeof p !== "object") return null;
  const input = gpBase(p.input_mtok);
  const output = gpBase(p.output_mtok);
  if (input === null || output === null) return null;

  const cache_read = gpBase(p.cache_read_mtok);
  const cache_create = gpBase(p.cache_write_mtok);
  const cache_create_1h = gpBase(p.cache_write_1h_mtok);

  const tierStart = gpFirstTierStart(p);
  let long_context;
  if (tierStart !== null) {
    const lcIn = gpTier(p.input_mtok, tierStart);
    const lcOut = gpTier(p.output_mtok, tierStart);
    const lcCr = gpTier(p.cache_read_mtok, tierStart);
    const lcCw = gpTier(p.cache_write_mtok, tierStart);
    const lcCw1h = gpTier(p.cache_write_1h_mtok, tierStart);
    if (lcIn !== null && lcOut !== null) {
      long_context = {
        input: lcIn,
        output: lcOut,
        cache_read: lcCr !== null ? lcCr : lcIn,
        cache_create: lcCw !== null ? lcCw : 0,
        ...(lcCw1h !== null ? { cache_create_1h: lcCw1h } : {}),
      };
    }
  }

  return normalizeModelPrice({
    input,
    output,
    cache_read: cache_read !== null ? cache_read : input,
    cache_create: cache_create !== null ? cache_create : 0,
    cache_create_1h,
    long_context_above: tierStart ?? undefined,
    long_context,
    off_peak_discount: offPeakDiscount,
  });
}

function resolveGenaiPricesEntry(prices, todayIso, family) {
  if (!prices) return null;
  if (!Array.isArray(prices)) {
    return gpPriceObjToModelPrice(prices);
  }
  if (prices.length === 0) return null;

  // Separate date-scoped entries from time-of-day entries (e.g. DeepSeek UTC peak windows).
  let currentBase = prices[0]?.prices ?? null;
  const timeWindows = [];

  for (let i = 1; i < prices.length; i++) {
    const entry = prices[i];
    const c = entry?.constraint ?? {};
    if (c.start_time || c.end_time) {
      timeWindows.push(entry);
      continue;
    }
    if (c.start_date && c.start_date <= todayIso) {
      currentBase = entry.prices;
    }
  }

  // DeepSeek encodes off-peak as base and peak windows (01:00-04:00Z, 06:00-10:00Z)
  // as time-constrained overrides. HPBar stores peak as base + off_peak_discount.
  if (family === "deepseek" && timeWindows.length > 0 && currentBase) {
    const peak = timeWindows[timeWindows.length - 1].prices;
    const baseIn = gpBase(currentBase.input_mtok);
    const peakIn = gpBase(peak?.input_mtok);
    if (baseIn !== null && peakIn !== null && peakIn > baseIn && baseIn > 0) {
      const discount = roundRate(baseIn / peakIn);
      return gpPriceObjToModelPrice(peak, discount);
    }
  }

  return gpPriceObjToModelPrice(currentBase);
}

function extractMatchEquals(model) {
  const ids = new Set();
  if (typeof model.id === "string") ids.add(model.id.toLowerCase());
  const match = model.match;
  if (match && typeof match === "object") {
    if (typeof match.equals === "string") ids.add(match.equals.toLowerCase());
    if (Array.isArray(match.or)) {
      for (const clause of match.or) {
        if (clause && typeof clause.equals === "string") {
          ids.add(clause.equals.toLowerCase());
        }
      }
    }
  }
  return [...ids];
}

export function extractGenaiPrices(data, { todayIso, warnings = [] } = {}) {
  const out = {};
  if (!Array.isArray(data)) return out;
  const today = todayIso || new Date().toISOString().slice(0, 10);

  for (const provider of data) {
    const pid = String(provider?.id || "").toLowerCase();
    const family = GENAI_PROVIDER_MAP[pid];
    if (!family || !Array.isArray(provider.models)) continue;

    for (const model of provider.models) {
      const resolved = resolveGenaiPricesEntry(model.prices, today, family);
      if (!resolved) continue;
      const err = validatePrice(resolved);
      if (err) {
        warnings.push(`genai-prices/${pid}/${model.id}: skipped (${err})`);
        continue;
      }

      for (const id of extractMatchEquals(model)) {
        if (isDatedSnapshot(id)) continue;
        if (!matchesFamily(family, id)) continue;
        out[id] = resolved;
      }
    }
  }
  return out;
}

export function extractModelsDev(data, { warnings = [] } = {}) {
  const out = {};
  if (!data || typeof data !== "object") return out;

  for (const [rawPid, provider] of Object.entries(data)) {
    const family = MODELS_DEV_PROVIDER_MAP[rawPid.toLowerCase()];
    if (!family || !provider || typeof provider.models !== "object") continue;

    for (const [rawId, model] of Object.entries(provider.models)) {
      const id = rawId.toLowerCase().replace(/^[^/]+\//, "");
      if (isDatedSnapshot(id) || !matchesFamily(family, id)) continue;
      const cost = model?.cost;
      if (!cost || typeof cost !== "object") continue;

      let long_context_above;
      let long_context;
      if (cost.context_over_200k && typeof cost.context_over_200k === "object") {
        const lc = cost.context_over_200k;
        if (typeof lc.input === "number" && typeof lc.output === "number") {
          long_context_above = 200000;
          long_context = {
            input: lc.input,
            output: lc.output,
            cache_read: typeof lc.cache_read === "number" ? lc.cache_read : lc.input,
            cache_create: typeof lc.cache_write === "number" ? lc.cache_write : 0,
          };
        }
      }

      const normalized = normalizeModelPrice({
        input: cost.input,
        output: cost.output,
        cache_read: typeof cost.cache_read === "number" ? cost.cache_read : cost.input,
        cache_create: typeof cost.cache_write === "number" ? cost.cache_write : 0,
        long_context_above,
        long_context,
      });
      if (!normalized) continue;
      const err = validatePrice(normalized);
      if (err) {
        warnings.push(`models.dev/${rawPid}/${rawId}: skipped (${err})`);
        continue;
      }
      out[id] = normalized;
    }
  }
  return out;
}

function perTokenToMtok(v) {
  return typeof v === "number" && Number.isFinite(v) ? roundRate(v * 1e6) : null;
}

function extractLiteLLMTier(entry, family) {
  // Style 1: tiered_pricing array (used by DashScope / Qwen)
  if (Array.isArray(entry.tiered_pricing) && entry.tiered_pricing.length >= 2) {
    const sorted = [...entry.tiered_pricing].sort(
      (a, b) => (a?.range?.[0] ?? 0) - (b?.range?.[0] ?? 0)
    );
    const t0 = sorted[0];
    const t1 = sorted[1];
    const baseIn = perTokenToMtok(t0.input_cost_per_token);
    const baseOut = perTokenToMtok(t0.output_cost_per_token);
    const lcIn = perTokenToMtok(t1.input_cost_per_token);
    const lcOut = perTokenToMtok(t1.output_cost_per_token);
    const limit = t1?.range?.[0];
    if (baseIn !== null && baseOut !== null && lcIn !== null && lcOut !== null && limit > 0) {
      const baseCr =
        family === "qwen"
          ? perTokenToMtok(t0.explicit_cache_read_input_token_cost) ??
            perTokenToMtok(t0.cache_read_input_token_cost)
          : perTokenToMtok(t0.cache_read_input_token_cost);
      const lcCr =
        family === "qwen"
          ? perTokenToMtok(t1.explicit_cache_read_input_token_cost) ??
            perTokenToMtok(t1.cache_read_input_token_cost)
          : perTokenToMtok(t1.cache_read_input_token_cost);
      const baseCw = perTokenToMtok(t0.cache_creation_input_token_cost) ?? 0;
      const lcCw = perTokenToMtok(t1.cache_creation_input_token_cost) ?? baseCw;
      return normalizeModelPrice({
        input: baseIn,
        output: baseOut,
        cache_read: baseCr ?? baseIn,
        cache_create: baseCw,
        long_context_above: limit,
        long_context: {
          input: lcIn,
          output: lcOut,
          cache_read: lcCr ?? lcIn,
          cache_create: lcCw,
        },
      });
    }
  }

  // Style 2: flat fields + optional *_above_<N>k_tokens suffix
  const input = perTokenToMtok(entry.input_cost_per_token);
  const output = perTokenToMtok(entry.output_cost_per_token);
  if (input === null || output === null) return null;

  const cache_read =
    family === "qwen"
      ? perTokenToMtok(entry.explicit_cache_read_input_token_cost) ??
        perTokenToMtok(entry.cache_read_input_token_cost)
      : perTokenToMtok(entry.cache_read_input_token_cost);
  const cache_create = perTokenToMtok(entry.cache_creation_input_token_cost) ?? 0;
  const cache_create_1h = perTokenToMtok(
    entry.cache_creation_input_token_cost_above_1hr
  );

  // Detect any `input_cost_per_token_above_<N>k_tokens` threshold.
  let long_context_above;
  let long_context;
  for (const key of Object.keys(entry)) {
    const m = /^input_cost_per_token_above_(\d+)k_tokens$/.exec(key);
    if (!m) continue;
    const k = m[1];
    const lcIn = perTokenToMtok(entry[key]);
    const lcOut = perTokenToMtok(entry[`output_cost_per_token_above_${k}k_tokens`]);
    if (lcIn !== null && lcOut !== null) {
      const lcCr = perTokenToMtok(
        entry[`cache_read_input_token_cost_above_${k}k_tokens`]
      );
      const lcCw = perTokenToMtok(
        entry[`cache_creation_input_token_cost_above_${k}k_tokens`]
      );
      const lcCw1h = perTokenToMtok(
        entry[`cache_creation_input_token_cost_above_1hr_above_${k}k_tokens`]
      );
      long_context_above = Number(k) * 1000;
      long_context = {
        input: lcIn,
        output: lcOut,
        cache_read: lcCr ?? lcIn,
        cache_create: lcCw ?? cache_create,
        ...(lcCw1h !== null ? { cache_create_1h: lcCw1h } : {}),
      };
      break;
    }
  }

  return normalizeModelPrice({
    input,
    output,
    cache_read: cache_read ?? input,
    cache_create,
    cache_create_1h,
    long_context_above,
    long_context,
  });
}

export function extractLiteLLM(data, { warnings = [], deepseekOffPeakDiscount = 0.5 } = {}) {
  const out = {};
  if (!data || typeof data !== "object") return out;

  for (const [rawKey, entry] of Object.entries(data)) {
    if (!entry || typeof entry !== "object") continue;
    const prov = String(entry.litellm_provider || "").toLowerCase();
    const family = LITELLM_PROVIDER_MAP[prov];
    if (!family) continue;

    // Only accept bare IDs or single `provider/<model>` keys for that exact provider.
    let id = rawKey.toLowerCase();
    if (id.includes("/")) {
      const [prefix, rest] = id.split("/", 2);
      if (prefix !== prov || !rest || rest.includes("/")) continue;
      id = rest;
    }

    if (isDatedSnapshot(id) || !matchesFamily(family, id)) continue;
    // Skip non-chat modes if explicitly marked
    if (
      entry.mode &&
      !["chat", "responses", "completion"].includes(String(entry.mode).toLowerCase())
    ) {
      continue;
    }

    const normalized = extractLiteLLMTier(entry, family);
    if (!normalized) continue;

    if (
      family === "deepseek" &&
      (id.includes("v4") || id === "deepseek-flash") &&
      deepseekOffPeakDiscount < 1
    ) {
      normalized.off_peak_discount = deepseekOffPeakDiscount;
    }

    const err = validatePrice(normalized);
    if (err) {
      warnings.push(`litellm/${rawKey}: skipped (${err})`);
      continue;
    }

    // Prefer bare key over prefixed duplicate if both exist, or first valid entry.
    if (!out[id] || !rawKey.includes("/")) {
      out[id] = normalized;
    }
  }
  return out;
}

export function syncPricing({
  existing = {},
  genaiPrices = null,
  modelsDev = null,
  litellm = null,
  overrides = {},
  preserveExisting = true,
  todayIso = new Date().toISOString().slice(0, 10),
} = {}) {
  const warnings = [];
  const merged = {};

  // 1. Preserve legacy entries already in bundled pricing.json so retired models
  //    keep their last published rate when upstreams prune them.
  if (preserveExisting && existing && typeof existing === "object") {
    for (const [k, v] of Object.entries(existing)) {
      const normalized = normalizeModelPrice(v);
      if (normalized && !validatePrice(normalized)) {
        merged[k.toLowerCase()] = normalized;
      }
    }
  }

  // 2. LiteLLM first-party (DashScope/Qwen + missing variants)
  const llTable = litellm
    ? extractLiteLLM(litellm, {
        warnings,
        deepseekOffPeakDiscount: overrides.deepseek_off_peak_discount ?? 0.5,
      })
    : {};
  for (const [k, v] of Object.entries(llTable)) {
    merged[k] = v;
  }

  // 3. models.dev (first-party Alibaba/Qwen, DeepSeek v4.1, etc.)
  const mdTable = modelsDev ? extractModelsDev(modelsDev, { warnings }) : {};
  for (const [k, v] of Object.entries(mdTable)) {
    merged[k] = v;
  }

  // 4. pydantic/genai-prices (highest-fidelity structured source — wins for its 7 providers)
  const gpTable = genaiPrices
    ? extractGenaiPrices(genaiPrices, { todayIso, warnings })
    : {};
  for (const [k, v] of Object.entries(gpTable)) {
    merged[k] = v;
  }

  // 5. Explicit model overrides (always win — regional policies, day-0 launches)
  if (overrides.models && typeof overrides.models === "object") {
    for (const [rawId, rawPrice] of Object.entries(overrides.models)) {
      const id = rawId.toLowerCase();
      const normalized = normalizeModelPrice(rawPrice);
      const err = validatePrice(normalized);
      if (!normalized || err) {
        throw new Error(`Invalid override model ${rawId}: ${err || "missing rates"}`);
      }
      merged[id] = normalized;
    }
  }

  // 6. Aliases (always mirror target's latest price)
  if (overrides.aliases && typeof overrides.aliases === "object") {
    for (const [rawAlias, rawTarget] of Object.entries(overrides.aliases)) {
      const alias = rawAlias.toLowerCase();
      const target = String(rawTarget).toLowerCase();
      if (!merged[target]) {
        throw new Error(`Alias ${rawAlias} points to unknown target ${rawTarget}`);
      }
      merged[alias] = structuredClone(merged[target]);
    }
  }

  // 7. Fallback aliases (only used when upstream did not provide the key directly)
  if (overrides.fallback_aliases && typeof overrides.fallback_aliases === "object") {
    for (const [rawAlias, rawTarget] of Object.entries(overrides.fallback_aliases)) {
      const alias = rawAlias.toLowerCase();
      const target = String(rawTarget).toLowerCase();
      if (merged[alias]) continue;
      if (!merged[target]) {
        throw new Error(
          `Fallback alias ${rawAlias} points to unknown target ${rawTarget}`
        );
      }
      merged[alias] = structuredClone(merged[target]);
    }
  }

  // 8. Exclusions (always deleted last)
  if (Array.isArray(overrides.exclude)) {
    for (const rawId of overrides.exclude) {
      delete merged[String(rawId).toLowerCase()];
    }
  }

  // Deterministic alphabetical key ordering for clean diffs
  const sorted = {};
  for (const key of Object.keys(merged).sort()) {
    const err = validatePrice(merged[key]);
    if (err) {
      throw new Error(`Final validation failed for ${key}: ${err}`);
    }
    sorted[key] = merged[key];
  }

  return {
    table: sorted,
    warnings,
    counts: {
      genaiPrices: Object.keys(gpTable).length,
      modelsDev: Object.keys(mdTable).length,
      litellm: Object.keys(llTable).length,
      total: Object.keys(sorted).length,
    },
  };
}

export function diffTables(before, after) {
  const added = [];
  const removed = [];
  const changed = [];
  const largeJumps = [];

  const allKeys = new Set([...Object.keys(before), ...Object.keys(after)]);
  for (const key of [...allKeys].sort()) {
    const a = before[key];
    const b = after[key];
    if (!a && b) {
      added.push({ id: key, after: b });
    } else if (a && !b) {
      removed.push({ id: key, before: a });
    } else if (JSON.stringify(a) !== JSON.stringify(b)) {
      changed.push({ id: key, before: a, after: b });
      for (const field of ["input", "output"]) {
        if (a[field] > 0 && b[field] > 0) {
          const ratio = b[field] / a[field];
          if (ratio >= 1.5 || ratio <= 0.5) {
            largeJumps.push({ id: key, field, before: a[field], after: b[field] });
          }
        }
      }
    }
  }

  return { added, removed, changed, largeJumps };
}

export function formatPricingJson(table) {
  const lines = Object.entries(table).map(
    ([id, price]) => `  ${JSON.stringify(id)}: ${JSON.stringify(price)}`
  );
  return `{\n${lines.join(",\n")}\n}\n`;
}

export function formatSummaryMarkdown(diff, counts, warnings, todayIso) {
  const parts = [];
  parts.push(`## Pricing Sync Summary (${todayIso})`);
  parts.push("");
  parts.push(
    `- **Total models:** ${counts.total} (genai-prices: ${counts.genaiPrices}, models.dev: ${counts.modelsDev}, litellm: ${counts.litellm})`
  );
  parts.push(
    `- **Changes:** +${diff.added.length} added, ~${diff.changed.length} updated, -${diff.removed.length} removed`
  );

  if (diff.largeJumps.length > 0) {
    parts.push("");
    parts.push("### ⚠️ Large Rate Changes (>=50%) — Review Carefully");
    parts.push("| Model | Field | Before ($/1M) | After ($/1M) |");
    parts.push("| :--- | :--- | ---: | ---: |");
    for (const j of diff.largeJumps) {
      parts.push(`| \`${j.id}\` | ${j.field} | ${j.before} | ${j.after} |`);
    }
  }

  if (diff.added.length > 0) {
    parts.push("");
    parts.push("### Added Models");
    parts.push("| Model | Input | Output | Cache Read | Cache Write |");
    parts.push("| :--- | ---: | ---: | ---: | ---: |");
    for (const item of diff.added) {
      const p = item.after;
      parts.push(
        `| \`${item.id}\` | ${p.input} | ${p.output} | ${p.cache_read} | ${p.cache_create} |`
      );
    }
  }

  if (diff.changed.length > 0) {
    parts.push("");
    parts.push("### Updated Models");
    parts.push("| Model | Before (in/out/cr/cw) | After (in/out/cr/cw) |");
    parts.push("| :--- | :--- | :--- |");
    for (const item of diff.changed) {
      const fmt = (p) =>
        `${p.input} / ${p.output} / ${p.cache_read} / ${p.cache_create}` +
        (p.cache_create_1h !== undefined ? ` (1h:${p.cache_create_1h})` : "") +
        (p.long_context_above ? ` (>${p.long_context_above / 1000}k)` : "") +
        (p.off_peak_discount ? ` (off-peak:${p.off_peak_discount}x)` : "");
      parts.push(`| \`${item.id}\` | ${fmt(item.before)} | ${fmt(item.after)} |`);
    }
  }

  if (diff.removed.length > 0) {
    parts.push("");
    parts.push("### Removed Models");
    for (const item of diff.removed) {
      parts.push(`- \`${item.id}\``);
    }
  }

  if (warnings.length > 0) {
    parts.push("");
    parts.push(`<details><summary>Skipped upstream entries (${warnings.length})</summary>\n`);
    for (const w of warnings.slice(0, 50)) {
      parts.push(`- ${w}`);
    }
    if (warnings.length > 50) {
      parts.push(`- ...and ${warnings.length - 50} more`);
    }
    parts.push("\n</details>");
  }

  return parts.join("\n") + "\n";
}

async function loadJsonSource(label, localPath, url, { required = true } = {}) {
  if (localPath === "none") return null;
  if (localPath) {
    return JSON.parse(fs.readFileSync(localPath, "utf8"));
  }
  try {
    const resp = await fetch(url, {
      headers: { "User-Agent": "HPBar-Pricing-Sync" },
      signal: AbortSignal.timeout(30_000),
    });
    if (!resp.ok) {
      throw new Error(`HTTP ${resp.status} ${resp.statusText}`);
    }
    return await resp.json();
  } catch (err) {
    if (required) {
      throw new Error(`Failed to fetch ${label} (${url}): ${err.message}`);
    }
    console.warn(`[sync-pricing] Warning: skipped ${label} (${err.message})`);
    return null;
  }
}

function parseArgs(argv) {
  const args = {
    write: false,
    check: false,
    strictSources: false,
    preserveExisting: true,
    genaiPrices: null,
    modelsDev: null,
    litellm: null,
    pricingPath: DEFAULT_PRICING_PATH,
    overridesPath: DEFAULT_OVERRIDES_PATH,
    sourcesMetaPath: DEFAULT_SOURCES_META_PATH,
    summaryFile: null,
    metaFile: null,
    todayIso: new Date().toISOString().slice(0, 10),
  };

  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--write") args.write = true;
    else if (a === "--check") args.check = true;
    else if (a === "--strict-sources") args.strictSources = true;
    else if (a === "--no-preserve-existing") args.preserveExisting = false;
    else if (a === "--genai-prices") args.genaiPrices = argv[++i];
    else if (a === "--models-dev") args.modelsDev = argv[++i];
    else if (a === "--litellm") args.litellm = argv[++i];
    else if (a === "--pricing") args.pricingPath = argv[++i];
    else if (a === "--overrides") args.overridesPath = argv[++i];
    else if (a === "--sources-meta") args.sourcesMetaPath = argv[++i];
    else if (a === "--summary-file") args.summaryFile = argv[++i];
    else if (a === "--meta-file") args.metaFile = argv[++i];
    else if (a === "--today") args.todayIso = argv[++i];
    else throw new Error(`Unknown argument: ${a}`);
  }
  return args;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const existing = fs.existsSync(args.pricingPath)
    ? JSON.parse(fs.readFileSync(args.pricingPath, "utf8"))
    : {};
  const overrides = fs.existsSync(args.overridesPath)
    ? JSON.parse(fs.readFileSync(args.overridesPath, "utf8"))
    : {};

  const [genaiPrices, modelsDev, litellm] = await Promise.all([
    loadJsonSource("genai-prices", args.genaiPrices, DEFAULT_SOURCES.genaiPrices, {
      required: true,
    }),
    loadJsonSource("models.dev", args.modelsDev, DEFAULT_SOURCES.modelsDev, {
      required: args.strictSources,
    }),
    loadJsonSource("litellm", args.litellm, DEFAULT_SOURCES.litellm, {
      required: args.strictSources,
    }),
  ]);

  const { table, warnings, counts } = syncPricing({
    existing,
    genaiPrices,
    modelsDev,
    litellm,
    overrides,
    preserveExisting: args.preserveExisting,
    todayIso: args.todayIso,
  });

  if (Object.keys(table).length < 30) {
    throw new Error(
      `Refusing to emit suspiciously small table (${Object.keys(table).length} models)`
    );
  }

  const diff = diffTables(existing, table);
  const summary = formatSummaryMarkdown(diff, counts, warnings, args.todayIso);
  if (args.summaryFile) {
    fs.writeFileSync(args.summaryFile, summary, "utf8");
  }
  console.log(summary);

  const hasChanges =
    diff.added.length > 0 || diff.removed.length > 0 || diff.changed.length > 0;
  const needsReview = diff.largeJumps.length > 0 || diff.removed.length > 0;

  if (args.metaFile) {
    fs.writeFileSync(
      args.metaFile,
      JSON.stringify(
        {
          changed: hasChanges,
          needs_review: needsReview,
          added: diff.added.length,
          updated: diff.changed.length,
          removed: diff.removed.length,
          large_jumps: diff.largeJumps.length,
          total: counts.total,
        },
        null,
        2
      ) + "\n",
      "utf8"
    );
  }

  if (args.write) {
    fs.writeFileSync(args.pricingPath, formatPricingJson(table), "utf8");
    if (hasChanges && fs.existsSync(args.sourcesMetaPath)) {
      const meta = JSON.parse(fs.readFileSync(args.sourcesMetaPath, "utf8"));
      meta.checked_on = args.todayIso;
      fs.writeFileSync(
        args.sourcesMetaPath,
        JSON.stringify(meta, null, 2) + "\n",
        "utf8"
      );
    }
    console.log(`Wrote ${counts.total} models to ${args.pricingPath}`);
  } else if (args.check && hasChanges) {
    console.error("pricing.json is out of date. Run: npm run pricing:sync");
    process.exit(1);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((err) => {
    console.error(`[sync-pricing] Error: ${err.message}`);
    process.exit(1);
  });
}
