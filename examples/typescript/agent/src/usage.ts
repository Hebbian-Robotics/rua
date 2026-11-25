import { isDeepStrictEqual } from "node:util";
import { calcPrice, updatePrices } from "@pydantic/genai-prices";
import type { LiveRun } from "hebbian-rua";
import {
  DEFAULT_INFERENCE_MODE,
  type InferenceMode,
  type Provider,
  type TokenPriceContext,
  type TokenUsage,
} from "./providers/types.js";

const PRICING_REFRESH_INTERVAL_MILLISECONDS = 60 * 60 * 1000;
let lastPricingRefresh = 0;
let pricingRefresh: Promise<void> | undefined;

function requestedPriceContext(
  provider: Provider,
  inferenceMode: InferenceMode,
): TokenPriceContext | undefined {
  if (inferenceMode === "standard") return undefined;
  switch (provider) {
    case "openai":
      return { service_tier: inferenceMode };
    case "anthropic":
      return { speed: inferenceMode };
    case "gemini":
      return { service_tier: "priority" };
  }
}

export async function estimateModelCost(
  provider: Provider,
  usage: TokenUsage,
  inferenceMode: InferenceMode = DEFAULT_INFERENCE_MODE,
): Promise<number | null> {
  const priceContext =
    usage.priceContext ?? requestedPriceContext(provider, inferenceMode);
  const billedTier =
    priceContext &&
    ("speed" in priceContext ? priceContext.speed : priceContext.service_tier);
  const needsTierPrice =
    billedTier !== undefined &&
    billedTier !== "standard" &&
    billedTier !== "default";
  const tokens = {
    input_tokens: usage.inputTokens,
    output_tokens: usage.outputTokens,
    cache_read_tokens: usage.cachedInputTokens,
    cache_write_tokens: usage.cacheWriteTokens,
  };
  const options = {
    providerId: provider === "gemini" ? "google" : provider,
    timestamp: new Date(),
  };
  const calculate = (): number | null => {
    try {
      const result = calcPrice(tokens, usage.model, {
        ...options,
        priceContext,
      });
      // CONSTRAINT: The catalog silently uses standard rates when a tier has no
      // applicable rates. Compare the rates themselves so zero tokens can still be priced.
      if (
        needsTierPrice &&
        isDeepStrictEqual(
          result?.model_price,
          calcPrice(tokens, usage.model, options)?.model_price,
        )
      ) {
        return null;
      }
      return result &&
        Number.isFinite(result.total_price) &&
        result.total_price >= 0
        ? result.total_price
        : null;
    } catch {
      return null;
    }
  };
  const bundledEstimate = calculate();
  if (bundledEstimate !== null) return bundledEstimate;
  if (
    !pricingRefresh &&
    Date.now() - lastPricingRefresh >= PRICING_REFRESH_INTERVAL_MILLISECONDS
  ) {
    lastPricingRefresh = Date.now();
    pricingRefresh = new Promise<void>((resolve) => {
      updatePrices(({ remoteDataUrl, setProviderData }) => {
        const download = fetch(remoteDataUrl, {
          signal: AbortSignal.timeout(3000),
        }).then((response) => {
          if (!response.ok) throw new Error("Pricing data unavailable");
          return response.json();
        });
        setProviderData(download);
        void download.catch(() => null).then(() => resolve());
      });
    }).finally(() => {
      pricingRefresh = undefined;
    });
  }
  await pricingRefresh;
  return calculate();
}

export type UsageSnapshot = NonNullable<LiveRun["usage"]>;

export class RunUsage {
  readonly turns: TokenUsage[] = [];

  constructor(private readonly runId: string = new Date().toISOString()) {}

  snapshot(): UsageSnapshot {
    const total = (pick: (turn: TokenUsage) => number): number =>
      this.turns.reduce((sum, turn) => sum + pick(turn), 0);
    return {
      run_id: this.runId,
      turns: this.turns.length,
      input_tokens: total((turn) => turn.inputTokens),
      cached_input_tokens: total((turn) => turn.cachedInputTokens),
      output_tokens: total((turn) => turn.outputTokens),
      reasoning_tokens: total((turn) => turn.reasoningTokens),
    };
  }
}
