import test from "node:test";
import assert from "node:assert/strict";
import {
  syncPricing,
  extractGenaiPrices,
  extractModelsDev,
  extractLiteLLM,
  validatePrice,
  diffTables,
  formatPricingJson,
} from "../scripts/sync-pricing.mjs";

test("genai-prices enforces provider prefix isolation and ignores future start_date", () => {
  const gp = [
    {
      id: "anthropic",
      models: [
        {
          id: "claude-opus-5-5",
          match: { equals: "claude-opus-5-5" },
          prices: {
            input_mtok: 4,
            output_mtok: 20,
            cache_read_mtok: 0.2,
            cache_write_mtok: 5,
            cache_write_1h_mtok: 8,
          },
        },
        {
          id: "claude-haiku-5-5",
          prices: {
            input_mtok: { base: 0.1, tiers: [{ start: 100000, price: 0.5 }] },
            output_mtok: { base: 0.5, tiers: [{ start: 100000, price: 2.5 }] },
            cache_read_mtok: { base: 0.01, tiers: [{ start: 100000, price: 0.05 }] },
            cache_write_mtok: { base: 0.125, tiers: [{ start: 100000, price: 0.625 }] },
            cache_write_1h_mtok: { base: 0.2, tiers: [{ start: 100000, price: 1.0 }] },
          },
        },
      ],
    },
    {
      // Google Vertex entries containing Claude must NOT overwrite Anthropic
      id: "google",
      models: [
        {
          id: "claude-opus-5-5",
          prices: { input_mtok: 99, output_mtok: 99 },
        },
        {
          id: "gemini-4-argon",
          prices: [
            { prices: { input_mtok: 2, cache_read_mtok: 0.1, output_mtok: 10 } },
            {
              constraint: { start_date: "2026-12-01" },
              prices: { input_mtok: 4, cache_read_mtok: 0.2, output_mtok: 20 },
            },
          ],
        },
      ],
    },
  ];

  const beforeDec = extractGenaiPrices(gp, { todayIso: "2026-11-23" });
  assert.equal(beforeDec["claude-opus-5-5"].input, 4);
  assert.equal(beforeDec["claude-opus-5-5"].cache_create_1h, 8);
  assert.equal(beforeDec["claude-haiku-5-5"].long_context_above, 100000);
  assert.equal(beforeDec["claude-haiku-5-5"].long_context.cache_create_1h, 1);
  assert.equal(beforeDec["gemini-4-argon"].input, 2);

  const afterDec = extractGenaiPrices(gp, { todayIso: "2026-12-05" });
  assert.equal(afterDec["gemini-4-argon"].input, 4);
});

test("genai-prices converts DeepSeek UTC peak windows into HPBar peak-base + off_peak_discount", () => {
  const gp = [
    {
      id: "deepseek",
      models: [
        {
          id: "deepseek-v4-pro",
          prices: [
            { prices: { input_mtok: 0.435, cache_read_mtok: 0.003625, output_mtok: 0.87 } },
            {
              constraint: { start_date: "2026-08-17" },
              prices: { input_mtok: 0.66, cache_read_mtok: 0.022, output_mtok: 1.98 },
            },
            {
              constraint: { start_time: "01:00:00Z", end_time: "04:00:00Z" },
              prices: { input_mtok: 1.32, cache_read_mtok: 0.044, output_mtok: 3.96 },
            },
          ],
        },
      ],
    },
  ];

  const res = extractGenaiPrices(gp, { todayIso: "2026-11-23" });
  assert.deepEqual(res["deepseek-v4-pro"], {
    input: 1.32,
    output: 3.96,
    cache_read: 0.044,
    cache_create: 0,
    off_peak_discount: 0.5,
  });
});

test("litellm extracts Qwen explicit cache reads and tiered_pricing cleanly", () => {
  const ll = {
    "dashscope/qwen3.7-max": {
      litellm_provider: "dashscope",
      input_cost_per_token: 2.5e-6,
      output_cost_per_token: 7.5e-6,
      cache_read_input_token_cost: 5e-7,
      explicit_cache_read_input_token_cost: 2.5e-7,
      cache_creation_input_token_cost: 3.125e-6,
    },
    "dashscope/qwen3.7-plus": {
      litellm_provider: "dashscope",
      tiered_pricing: [
        {
          range: [0, 256000],
          input_cost_per_token: 4e-7,
          output_cost_per_token: 1.6e-6,
          cache_read_input_token_cost: 8e-8,
        },
        {
          range: [256000, 1000000],
          input_cost_per_token: 1.2e-6,
          output_cost_per_token: 4.8e-6,
          cache_read_input_token_cost: 2.4e-7,
        },
      ],
    },
    "bad/not-first-party": {
      litellm_provider: "dashscope",
      input_cost_per_token: 1e-6,
      output_cost_per_token: 2e-6,
    },
  };

  const res = extractLiteLLM(ll);
  assert.equal(res["qwen3.7-max"].cache_read, 0.25);
  assert.equal(res["qwen3.7-max"].cache_create, 3.125);
  assert.equal(res["qwen3.7-plus"].long_context_above, 256000);
  assert.equal(res["qwen3.7-plus"].long_context.output, 4.8);
  assert.equal(res["not-first-party"], undefined);
});

test("syncPricing enforces precedence, overrides, aliases, fallback aliases, and exclusions", () => {
  const existing = {
    "legacy-model": { input: 1, output: 2, cache_read: 0.1, cache_create: 0 },
    "codex-auto-review": { input: 1, output: 2, cache_read: 0.1, cache_create: 0 },
  };
  const gp = [
    {
      id: "openai",
      models: [
        {
          id: "gpt-6-astra",
          prices: { input_mtok: 10, output_mtok: 50, cache_read_mtok: 1, cache_write_mtok: 12.5 },
        },
      ],
    },
    {
      id: "deepseek",
      models: [
        {
          id: "deepseek-v4-flash",
          prices: [
            { prices: { input_mtok: 0.15, cache_read_mtok: 0.003, output_mtok: 0.6 } },
            {
              constraint: { start_time: "01:00:00Z", end_time: "04:00:00Z" },
              prices: { input_mtok: 0.3, cache_read_mtok: 0.006, output_mtok: 1.2 },
            },
          ],
        },
      ],
    },
  ];
  const md = {
    alibaba: {
      models: {
        "qwen3.8-max": { cost: { input: 2, output: 6, cache_read: 0.25 } },
      },
    },
  };
  const overrides = {
    exclude: ["codex-auto-review"],
    aliases: { "atxc44y9/gpt-6-astra": "gpt-6-astra" },
    fallback_aliases: { "deepseek-v4.1-flash": "deepseek-v4-flash" },
    models: {
      "qwen3.8-max": { input: 2, output: 6, cache_read: 0.2, cache_create: 2.5 },
    },
  };

  const { table } = syncPricing({
    existing,
    genaiPrices: gp,
    modelsDev: md,
    overrides,
    todayIso: "2026-11-23",
  });

  assert.ok(table["legacy-model"], "legacy bundled entry preserved");
  assert.equal(table["codex-auto-review"], undefined, "excluded model stripped");
  assert.deepEqual(table["atxc44y9/gpt-6-astra"], table["gpt-6-astra"]);
  assert.deepEqual(table["deepseek-v4.1-flash"], table["deepseek-v4-flash"]);
  assert.equal(table["qwen3.8-max"].cache_read, 0.2, "explicit override wins over models.dev");
  assert.ok(formatPricingJson(table).endsWith("\n}\n"));
});

test("validatePrice rejects implausible rates and diffTables flags large jumps", () => {
  assert.ok(validatePrice({ input: 5, output: 1, cache_read: 0.5, cache_create: 0 }));
  assert.ok(validatePrice({ input: 1, output: 5, cache_read: 2, cache_create: 0 }));
  assert.ok(validatePrice({ input: 1, output: 600, cache_read: 0.1, cache_create: 0 }));
  assert.equal(validatePrice({ input: 1, output: 5, cache_read: 0.1, cache_create: 0 }), null);

  const md = extractModelsDev({
    openai: {
      models: {
        "gpt-broken": { cost: { input: 10, output: 2 } },
      },
    },
  });
  assert.equal(md["gpt-broken"], undefined);

  const diff = diffTables(
    { "gpt-x": { input: 2, output: 10, cache_read: 0.2, cache_create: 0 } },
    { "gpt-x": { input: 4, output: 10, cache_read: 0.2, cache_create: 0 } }
  );
  assert.equal(diff.largeJumps.length, 1);
  assert.equal(diff.largeJumps[0].field, "input");
});
