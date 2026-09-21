//! Provider usage normalization and best-effort token-cost estimation.
//!
//! Token counts always come from the provider response. Cost is an estimate
//! from the generated model catalog and is intentionally absent when no rate
//! is known (or when OpenAI is reached through a subscription credential).

use std::collections::BTreeMap;
use std::ops::AddAssign;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::provider::{catalog_model_pricing, ModelPricingTier, Provider};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Total logical input tokens, including cache reads and cache writes.
    pub input_tokens: u64,
    /// Input tokens processed normally rather than read from or written to cache.
    pub uncached_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<UsageCost>,
    /// Per-model token totals for gateways that may fail over between models
    /// during one agent run. Empty for providers that use one fixed model.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_usage: BTreeMap<String, ModelTokenUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageCost {
    pub currency: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_creation: f64,
    pub total: f64,
    pub estimated: bool,
    pub pricing_source: String,
    /// Present for a single call when one pricing tier applies. Run aggregates
    /// drop this field if different calls crossed pricing tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rates_per_million_tokens: Option<ModelPricingTier>,
}

impl TokenUsage {
    /// OpenAI Responses and OpenAI-compatible Chat Completions count cached
    /// tokens inside the reported input total. Provider-specific aliases are
    /// accepted so the raw usage object remains useful across all backends.
    pub fn from_openai_compatible(
        provider: Provider,
        model: &str,
        raw: &Value,
        estimate_cost: bool,
    ) -> Self {
        let input_tokens = first_u64(raw, &["/prompt_tokens", "/input_tokens"]);
        let output_tokens = first_u64(raw, &["/completion_tokens", "/output_tokens"]);
        let cache_read_input_tokens = first_u64(
            raw,
            &[
                "/prompt_tokens_details/cached_tokens",
                "/input_tokens_details/cached_tokens",
                "/cached_tokens",
                "/prompt_cache_hit_tokens",
                "/cache_read_input_tokens",
            ],
        );
        let cache_creation_input_tokens = first_u64(
            raw,
            &[
                "/prompt_tokens_details/cache_creation_input_tokens",
                "/input_tokens_details/cache_creation_input_tokens",
                "/cache_creation_input_tokens",
            ],
        );
        let reported_miss = first_u64(raw, &["/prompt_cache_miss_tokens"]);
        let uncached_input_tokens = if raw.pointer("/prompt_cache_miss_tokens").is_some() {
            reported_miss
        } else {
            input_tokens
                .saturating_sub(cache_read_input_tokens.saturating_add(cache_creation_input_tokens))
        };
        let reasoning_output_tokens = optional_u64(
            raw,
            &[
                "/completion_tokens_details/reasoning_tokens",
                "/output_tokens_details/reasoning_tokens",
            ],
        );
        let mut usage = Self {
            input_tokens,
            uncached_input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            reasoning_output_tokens,
            cost: None,
            model_usage: BTreeMap::new(),
        };
        if estimate_cost {
            usage.estimate_cost(provider, model);
        }
        usage
    }

    /// Anthropic reports ordinary input, cache reads, and cache writes as three
    /// disjoint counters, so the logical input total is their sum.
    pub fn from_anthropic(provider: Provider, model: &str, raw: &Value) -> Self {
        let uncached_input_tokens = first_u64(raw, &["/input_tokens"]);
        let output_tokens = first_u64(raw, &["/output_tokens"]);
        let cache_read_input_tokens = first_u64(raw, &["/cache_read_input_tokens"]);
        let cache_creation_input_tokens = first_u64(raw, &["/cache_creation_input_tokens"]);
        let mut usage = Self {
            input_tokens: uncached_input_tokens
                .saturating_add(cache_read_input_tokens)
                .saturating_add(cache_creation_input_tokens),
            uncached_input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            reasoning_output_tokens: None,
            cost: None,
            model_usage: BTreeMap::new(),
        };
        usage.estimate_cost(provider, model);
        usage
    }

    fn estimate_cost(&mut self, provider: Provider, model: &str) {
        use chrono::{Datelike, Timelike};
        let at = chrono::Utc::now();
        let official = if provider == Provider::DeepSeek {
            let rates = match model {
                "deepseek-v4-pro" | "deepseek-v4-pro-0813" => Some((9.0, 27.0, 0.30)),
                "deepseek-flash" | "deepseek-v4-flash" | "deepseek-v4-flash-vision-exp" => {
                    Some((2.0, 8.0, 0.04))
                }
                _ => None,
            };
            rates.map(|(input, output, hit)| {
                let peak = at.weekday().num_days_from_monday() < 5
                    && ((1..4).contains(&at.hour()) || (6..10).contains(&at.hour()));
                let factor = if peak { 1.0 } else { 0.5 };
                crate::agent::provider::ModelPricing {
                    currency: "CNY".into(),
                    source: "deepseek-official-2026-09-17".into(),
                    tiers: vec![ModelPricingTier {
                        max_input_tokens: None,
                        input_per_million: input * factor,
                        output_per_million: output * factor,
                        cache_read_per_million: hit * factor,
                        cache_write_per_million: 0.0,
                    }],
                    overrides: Default::default(),
                }
            })
        } else {
            None
        };
        let Some(pricing) = official.or_else(|| catalog_model_pricing(provider, model)) else {
            return;
        };
        let Some(rates) = pricing.tier_for_input(self.input_tokens).cloned() else {
            return;
        };
        let per_million = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
        let input = per_million(self.uncached_input_tokens, rates.input_per_million);
        let output = per_million(self.output_tokens, rates.output_per_million);
        let cache_read = per_million(self.cache_read_input_tokens, rates.cache_read_per_million);
        let cache_creation = per_million(
            self.cache_creation_input_tokens,
            rates.cache_write_per_million,
        );
        self.cost = Some(UsageCost {
            currency: pricing.currency,
            input,
            output,
            cache_read,
            cache_creation,
            total: input + output + cache_read + cache_creation,
            estimated: true,
            pricing_source: pricing.source,
            rates_per_million_tokens: Some(rates),
        });
    }

    pub fn record_model(&mut self, model: &str) {
        let model = model.trim();
        if model.is_empty() {
            return;
        }
        let usage = self.model_usage.entry(model.to_string()).or_default();
        usage.input_tokens = usage.input_tokens.saturating_add(self.input_tokens);
        usage.output_tokens = usage.output_tokens.saturating_add(self.output_tokens);
        usage.cache_read_input_tokens = usage
            .cache_read_input_tokens
            .saturating_add(self.cache_read_input_tokens);
        usage.cache_creation_input_tokens = usage
            .cache_creation_input_tokens
            .saturating_add(self.cache_creation_input_tokens);
    }
}

impl AddAssign<&TokenUsage> for TokenUsage {
    fn add_assign(&mut self, rhs: &TokenUsage) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.uncached_input_tokens = self
            .uncached_input_tokens
            .saturating_add(rhs.uncached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(rhs.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(rhs.cache_creation_input_tokens);
        self.reasoning_output_tokens =
            match (self.reasoning_output_tokens, rhs.reasoning_output_tokens) {
                (Some(left), Some(right)) => Some(left.saturating_add(right)),
                (Some(left), None) => Some(left),
                (None, Some(right)) => Some(right),
                (None, None) => None,
            };
        match (&mut self.cost, &rhs.cost) {
            (Some(left), Some(right))
                if left.currency == right.currency
                    && left.pricing_source == right.pricing_source =>
            {
                left.input += right.input;
                left.output += right.output;
                left.cache_read += right.cache_read;
                left.cache_creation += right.cache_creation;
                left.total += right.total;
                if left.rates_per_million_tokens != right.rates_per_million_tokens {
                    left.rates_per_million_tokens = None;
                }
            }
            (slot @ None, Some(right)) => *slot = Some(right.clone()),
            (Some(_), Some(_)) => self.cost = None,
            _ => {}
        }
        for (model, usage) in &rhs.model_usage {
            let total = self.model_usage.entry(model.clone()).or_default();
            total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
            total.cache_read_input_tokens = total
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
            total.cache_creation_input_tokens = total
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
        }
    }
}

fn first_u64(value: &Value, pointers: &[&str]) -> u64 {
    optional_u64(value, pointers).unwrap_or_default()
}

fn optional_u64(value: &Value, pointers: &[&str]) -> Option<u64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_usage_survives_run_aggregation() {
        let mut primary = TokenUsage::from_openai_compatible(
            Provider::OpenRouter,
            "deepseek/deepseek-v4.1-flash",
            &serde_json::json!({"prompt_tokens": 10, "completion_tokens": 4}),
            false,
        );
        primary.record_model("deepseek/deepseek-v4.1-flash");
        let mut fallback = TokenUsage::from_openai_compatible(
            Provider::OpenRouter,
            "openai/gpt-6-luna",
            &serde_json::json!({"prompt_tokens": 7, "completion_tokens": 3}),
            false,
        );
        fallback.record_model("openai/gpt-6-luna");
        primary += &fallback;

        assert_eq!(primary.input_tokens, 17);
        assert_eq!(
            primary.model_usage["deepseek/deepseek-v4.1-flash"].input_tokens,
            10
        );
        assert_eq!(primary.model_usage["openai/gpt-6-luna"].output_tokens, 3);
    }
}
