use serde::Deserialize;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelPricing {
    pub key: String,
    pub label: String,
    pub input_credits_per_million: f64,
    pub cached_input_credits_per_million: f64,
    pub output_credits_per_million: f64,
    pub fast_credit_multiplier: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KnownUnpricedModel {
    pub key: String,
    pub label: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RateCardSource {
    pub name: String,
    pub checked_at: String,
    pub credit_to_usd: String,
    pub credits_per_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreditCost {
    pub priced: bool,
    pub pricing_label: String,
    pub unpriced_reason: Option<String>,
    pub billable_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub credit_multiplier: f64,
    pub credits: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricingContext {
    pub fast: bool,
}

impl PricingContext {
    pub const fn normal() -> Self {
        Self { fast: false }
    }

    pub const fn fast() -> Self {
        Self { fast: true }
    }
}

impl Default for PricingContext {
    fn default() -> Self {
        Self::normal()
    }
}

const RATE_CARD_JSON: &str = include_str!("../data/codex-rate-card.json");

static RATE_CARD: LazyLock<RateCard> = LazyLock::new(load_rate_card);
pub static CODEX_RATE_CARD_SOURCE: LazyLock<RateCardSource> =
    LazyLock::new(|| rate_card().source.clone());

#[derive(Debug, Clone)]
struct RateCard {
    source: RateCardSource,
    models: Vec<ModelPricing>,
    known_unpriced: Vec<KnownUnpricedModel>,
}

#[derive(Debug, Deserialize)]
struct RawRateCard {
    source: RawRateCardSource,
    models: Vec<RawModelPricing>,
    #[serde(default)]
    #[serde(rename = "knownUnpriced")]
    known_unpriced: Vec<RawKnownUnpricedModel>,
}

#[derive(Debug, Deserialize)]
struct RawRateCardSource {
    name: String,
    checked_at: String,
    credit_to_usd: String,
}

#[derive(Debug, Deserialize)]
struct RawModelPricing {
    key: String,
    label: String,
    input_credits_per_million: f64,
    cached_input_credits_per_million: f64,
    output_credits_per_million: f64,
    fast_credit_multiplier: Option<f64>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawKnownUnpricedModel {
    key: String,
    label: String,
    note: Option<String>,
}

pub fn normalize_model_name(model: &str) -> String {
    model
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn pricing_key_for_model(model: &str) -> String {
    let normalized = normalize_model_name(model);
    match normalized.as_str() {
        "gpt-5.4 mini" => "gpt-5.4-mini".to_string(),
        "gpt-5.3 codex" => "gpt-5.3-codex".to_string(),
        "gpt-image-2:image"
        | "gpt-image-2-image"
        | "gpt-image-2 image"
        | "gpt-image-2.0:image"
        | "gpt-image-2.0-image"
        | "gpt-image-2.0 image"
        | "gpt-image-2.0 (image)" => "gpt-image-2 (image)".to_string(),
        "gpt-image-2:text"
        | "gpt-image-2-text"
        | "gpt-image-2 text"
        | "gpt-image-2.0:text"
        | "gpt-image-2.0-text"
        | "gpt-image-2.0 text"
        | "gpt-image-2.0 (text)" => "gpt-image-2 (text)".to_string(),
        _ => normalized,
    }
}

pub fn get_model_pricing(model: &str) -> Option<ModelPricing> {
    let key = pricing_key_for_model(model);
    rate_card()
        .models
        .iter()
        .find(|pricing| pricing.key == key)
        .cloned()
}

pub fn list_model_pricing() -> Vec<ModelPricing> {
    let mut pricing = rate_card().models.clone();
    pricing.sort_by(|left, right| left.key.cmp(&right.key));
    pricing
}

pub fn list_known_unpriced_models() -> Vec<KnownUnpricedModel> {
    let mut pricing = rate_card().known_unpriced.clone();
    pricing.sort_by(|left, right| left.key.cmp(&right.key));
    pricing
}

pub fn calculate_credit_cost(model: &str, usage: TokenUsage) -> CreditCost {
    calculate_credit_cost_with_context(model, usage, PricingContext::normal())
}

pub fn calculate_credit_cost_with_context(
    model: &str,
    usage: TokenUsage,
    context: PricingContext,
) -> CreditCost {
    let cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    let billable_input_tokens = usage.input_tokens.saturating_sub(cached_input_tokens);
    let pricing = get_model_pricing(model);

    match pricing {
        Some(pricing) => {
            let credit_multiplier = if context.fast {
                pricing.fast_credit_multiplier
            } else {
                1.0
            };
            let normal_credits = (billable_input_tokens as f64 * pricing.input_credits_per_million
                + cached_input_tokens as f64 * pricing.cached_input_credits_per_million
                + usage.output_tokens as f64 * pricing.output_credits_per_million)
                / 1_000_000.0;

            CreditCost {
                priced: true,
                pricing_label: pricing.label.to_string(),
                unpriced_reason: None,
                billable_input_tokens,
                cached_input_tokens,
                output_tokens: usage.output_tokens,
                credit_multiplier,
                credits: normal_credits * credit_multiplier,
            }
        }
        None => CreditCost {
            priced: false,
            pricing_label: model.to_string(),
            unpriced_reason: None,
            billable_input_tokens,
            cached_input_tokens,
            output_tokens: usage.output_tokens,
            credit_multiplier: 1.0,
            credits: 0.0,
        },
    }
}

fn rate_card() -> &'static RateCard {
    &RATE_CARD
}

fn load_rate_card() -> RateCard {
    load_rate_card_from_str(RATE_CARD_JSON)
}

fn load_rate_card_from_str(content: &str) -> RateCard {
    let raw: RawRateCard = serde_json::from_str(content).unwrap_or_else(|error| {
        panic!("Failed to parse data/codex-rate-card.json: {error}");
    });
    validate_rate_card(&raw);
    let credits_per_usd = parse_credits_per_usd(&raw.source.credit_to_usd);

    RateCard {
        source: RateCardSource {
            name: raw.source.name,
            checked_at: raw.source.checked_at,
            credit_to_usd: raw.source.credit_to_usd,
            credits_per_usd,
        },
        models: convert_models(raw.models),
        known_unpriced: convert_known_unpriced_models(raw.known_unpriced),
    }
}

fn convert_models(raw: Vec<RawModelPricing>) -> Vec<ModelPricing> {
    raw.into_iter()
        .map(|model| ModelPricing {
            key: model.key,
            label: model.label,
            input_credits_per_million: model.input_credits_per_million,
            cached_input_credits_per_million: model.cached_input_credits_per_million,
            output_credits_per_million: model.output_credits_per_million,
            fast_credit_multiplier: model.fast_credit_multiplier.unwrap_or(1.0),
            note: model.note,
        })
        .collect()
}

fn convert_known_unpriced_models(raw: Vec<RawKnownUnpricedModel>) -> Vec<KnownUnpricedModel> {
    raw.into_iter()
        .map(|model| KnownUnpricedModel {
            key: model.key,
            label: model.label,
            note: model.note,
        })
        .collect()
}

fn parse_credits_per_usd(value: &str) -> f64 {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 4 || parts[1] != "credits" || parts[2] != "=" || parts[3] != "$1" {
        panic!("data/codex-rate-card.json credit_to_usd must match 'N credits = $1': {value:?}");
    }

    let credits = parts[0].parse::<f64>().unwrap_or_else(|_| {
        panic!("data/codex-rate-card.json credit_to_usd must start with a number: {value:?}");
    });
    if !credits.is_finite() || credits <= 0.0 {
        panic!(
            "data/codex-rate-card.json credit_to_usd must use a positive finite rate: {value:?}"
        );
    }
    credits
}

fn validate_rate_card(raw: &RawRateCard) {
    assert_non_empty(&raw.source.name, "source.name");
    assert_non_empty(&raw.source.checked_at, "source.checked_at");
    assert_non_empty(&raw.source.credit_to_usd, "source.credit_to_usd");

    if raw.models.is_empty() {
        panic!("data/codex-rate-card.json must define at least one model");
    }

    let mut keys = HashSet::new();
    for model in &raw.models {
        assert_non_empty(&model.key, "models[].key");
        assert_non_empty(&model.label, "models[].label");
        if !keys.insert(model.key.as_str()) {
            panic!(
                "data/codex-rate-card.json has duplicate model key: {}",
                model.key
            );
        }
        assert_non_negative_finite(
            model.input_credits_per_million,
            "models[].input_credits_per_million",
        );
        assert_non_negative_finite(
            model.cached_input_credits_per_million,
            "models[].cached_input_credits_per_million",
        );
        assert_non_negative_finite(
            model.output_credits_per_million,
            "models[].output_credits_per_million",
        );
        if let Some(multiplier) = model.fast_credit_multiplier {
            assert_positive_finite(multiplier, "models[].fast_credit_multiplier");
        }
    }

    let mut known_unpriced_keys = HashSet::new();
    for model in &raw.known_unpriced {
        assert_non_empty(&model.key, "knownUnpriced[].key");
        assert_non_empty(&model.label, "knownUnpriced[].label");
        if keys.contains(model.key.as_str()) {
            panic!(
                "data/codex-rate-card.json known unpriced model duplicates priced model key: {}",
                model.key
            );
        }
        if !known_unpriced_keys.insert(model.key.as_str()) {
            panic!(
                "data/codex-rate-card.json has duplicate known unpriced model key: {}",
                model.key
            );
        }
    }
}

fn assert_non_empty(value: &str, path: &str) {
    if value.trim().is_empty() {
        panic!("data/codex-rate-card.json field {path} cannot be empty");
    }
}

fn assert_non_negative_finite(value: f64, path: &str) {
    if !value.is_finite() || value < 0.0 {
        panic!("data/codex-rate-card.json field {path} must be finite and non-negative");
    }
}

fn assert_positive_finite(value: f64, path: &str) {
    if !value.is_finite() || value <= 0.0 {
        panic!("data/codex-rate-card.json field {path} must be finite and positive");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_model_names_and_aliases() {
        assert_eq!(normalize_model_name("  GPT-5.4   MINI "), "gpt-5.4 mini");
        assert_eq!(pricing_key_for_model("GPT-5.4   MINI"), "gpt-5.4-mini");
        assert_eq!(
            get_model_pricing("gpt-image-2.0:image")
                .expect("image pricing")
                .label,
            "GPT-Image-2 (image)"
        );
    }

    #[test]
    fn calculates_credit_cost_from_billable_cached_and_output_tokens() {
        let cost = calculate_credit_cost(
            "gpt-5.5",
            TokenUsage {
                input_tokens: 1000,
                cached_input_tokens: 200,
                output_tokens: 300,
            },
        );

        assert!(cost.priced);
        assert_eq!(cost.pricing_label, "GPT-5.5");
        assert_eq!(cost.billable_input_tokens, 800);
        assert_eq!(cost.cached_input_tokens, 200);
        assert_eq!(cost.output_tokens, 300);
        assert_eq!(cost.credit_multiplier, 1.0);
        assert!((cost.credits - 0.3275).abs() < 0.000001);
    }

    #[test]
    fn loads_gpt_5_6_pricing_from_rate_card() {
        let expected = [
            ("gpt-5.6-sol", "GPT-5.6 Sol", 250.0, 25.0, 1500.0),
            ("gpt-5.6-terra", "GPT-5.6 Terra", 125.0, 12.5, 125.0),
            ("gpt-5.6-luna", "GPT-5.6 Luna", 50.0, 5.0, 300.0),
        ];

        for (key, label, input, cached_input, output) in expected {
            let pricing = get_model_pricing(key).expect("GPT-5.6 pricing");
            assert_eq!(pricing.label, label);
            assert_eq!(pricing.input_credits_per_million, input);
            assert_eq!(pricing.cached_input_credits_per_million, cached_input);
            assert_eq!(pricing.output_credits_per_million, output);
            assert_eq!(pricing.fast_credit_multiplier, 1.0);
        }
    }

    #[test]
    fn applies_fast_credit_multiplier_from_rate_card() {
        let usage = TokenUsage {
            input_tokens: 1000,
            cached_input_tokens: 200,
            output_tokens: 300,
        };

        let gpt55 = calculate_credit_cost_with_context("gpt-5.5", usage, PricingContext::fast());
        let gpt54 = calculate_credit_cost_with_context("gpt-5.4", usage, PricingContext::fast());

        assert_eq!(gpt55.credit_multiplier, 2.5);
        assert!((gpt55.credits - 0.81875).abs() < 0.000001);
        assert_eq!(gpt54.credit_multiplier, 2.0);
        assert!((gpt54.credits - 0.3275).abs() < 0.000001);
    }

    #[test]
    fn models_without_fast_multiplier_default_to_one() {
        let cost = calculate_credit_cost_with_context(
            "gpt-5.2",
            TokenUsage {
                input_tokens: 1000,
                cached_input_tokens: 0,
                output_tokens: 0,
            },
            PricingContext::fast(),
        );

        assert_eq!(cost.credit_multiplier, 1.0);
        assert!((cost.credits - 0.04375).abs() < 0.000001);
    }

    #[test]
    fn clamps_cached_input_and_handles_unknown_models() {
        let cost = calculate_credit_cost(
            "future-model",
            TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 250,
                output_tokens: 50,
            },
        );

        assert!(!cost.priced);
        assert_eq!(cost.pricing_label, "future-model");
        assert_eq!(cost.billable_input_tokens, 0);
        assert_eq!(cost.cached_input_tokens, 100);
        assert_eq!(cost.credit_multiplier, 1.0);
        assert_eq!(cost.credits, 0.0);
    }

    #[test]
    fn spark_model_is_priced_at_zero_credits() {
        let cost = calculate_credit_cost(
            "gpt-5.3-codex-spark",
            TokenUsage {
                input_tokens: 500,
                cached_input_tokens: 0,
                output_tokens: 100,
            },
        );

        assert!(cost.priced);
        assert_eq!(cost.pricing_label, "GPT-5.3-Codex-Spark");
        assert_eq!(cost.credits, 0.0);
    }

    #[test]
    fn pricing_inventory_is_sorted() {
        let keys = list_model_pricing()
            .into_iter()
            .map(|pricing| pricing.key)
            .collect::<Vec<_>>();

        assert_eq!(keys.first().map(String::as_str), Some("gpt-5.2"));
        assert!(keys.iter().any(|k| k == "gpt-5.5"));
    }

    #[test]
    fn loads_source_metadata_from_static_rate_card() {
        assert_eq!(
            CODEX_RATE_CARD_SOURCE.name,
            "OpenAI Help Center Codex rate card"
        );
        assert_eq!(CODEX_RATE_CARD_SOURCE.checked_at, "2026-07-10");
        assert_eq!(CODEX_RATE_CARD_SOURCE.credit_to_usd, "25 credits = $1");
        assert!((CODEX_RATE_CARD_SOURCE.credits_per_usd - 25.0).abs() < f64::EPSILON);
        assert_eq!(list_model_pricing().len(), 11);
        assert!(list_known_unpriced_models().is_empty());
    }

    #[test]
    fn known_unpriced_models_do_not_require_price_fields() {
        let rate_card = load_rate_card_from_str(
            r#"{
                "source": {
                    "name": "test",
                    "checked_at": "2026-05-27",
                    "credit_to_usd": "50 credits = $1"
                },
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "input_credits_per_million": 1.0,
                        "cached_input_credits_per_million": 0.5,
                        "output_credits_per_million": 2.0,
                        "fast_credit_multiplier": 3.0
                    }
                ],
                "knownUnpriced": [
                    {
                        "key": "future-model",
                        "label": "Future Model",
                        "note": "not yet priced"
                    }
                ]
            }"#,
        );

        assert_eq!(rate_card.source.credits_per_usd, 50.0);
        assert_eq!(rate_card.models[0].fast_credit_multiplier, 3.0);
        assert_eq!(rate_card.known_unpriced.len(), 1);
        assert_eq!(rate_card.known_unpriced[0].key, "future-model");
        assert_eq!(
            rate_card.known_unpriced[0].note.as_deref(),
            Some("not yet priced")
        );
    }

    #[test]
    #[should_panic(expected = "credit_to_usd must match")]
    fn invalid_credit_exchange_rate_format_is_rejected() {
        parse_credits_per_usd("25 = $1");
    }

    #[test]
    #[should_panic(expected = "fast_credit_multiplier")]
    fn invalid_fast_multiplier_is_rejected() {
        load_rate_card_from_str(
            r#"{
                "source": {
                    "name": "test",
                    "checked_at": "2026-05-27",
                    "credit_to_usd": "50 credits = $1"
                },
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "input_credits_per_million": 1.0,
                        "cached_input_credits_per_million": 0.5,
                        "output_credits_per_million": 2.0,
                        "fast_credit_multiplier": 0.0
                    }
                ],
                "knownUnpriced": []
            }"#,
        );
    }
}
