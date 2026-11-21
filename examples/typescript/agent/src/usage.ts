// Tokens a model used across a run, in the shape the station records and the viewer shows.

import { calcPrice, updatePrices } from "@pydantic/genai-prices";
import type { Provider, TokenUsage } from "./providers/types.js";

const PRICING_REFRESH_INTERVAL_MILLISECONDS = 60 * 60 * 1000;
let lastPricingRefresh = 0;
let pricingRefresh: Promise<void> | undefined;

export async function estimateModelCost(
  provider: Provider,
  usage: TokenUsage,
): Promise<number | null> {
  const calculate = (): number | null => {
    try {
      const result = calcPrice(
        {
          input_tokens: usage.inputTokens,
          output_tokens: usage.outputTokens,
          cache_read_tokens: usage.cachedInputTokens,
          cache_write_tokens: usage.cacheWriteTokens,
        },
        usage.model,
        { providerId: provider === "gemini" ? "google" : provider },
      );
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

export interface UsageSnapshot {
  run_id: string;
  turns: number;
  input_tokens: number;
  cached_input_tokens: number;
  output_tokens: number;
  reasoning_tokens: number;
}

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
