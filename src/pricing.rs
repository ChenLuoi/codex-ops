use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
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
    pub url: String,
    pub checked_at: String,
    pub credit_to_usd: String,
    pub credits_per_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricingEvent {
    pub key: String,
    pub title: String,
    pub announced_at: DateTime<Utc>,
    pub url: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelPricingChange {
    pub model_key: String,
    pub model_label: String,
    pub old_pricing: ModelPricing,
    pub new_pricing: ModelPricing,
    pub effective_at: DateTime<Utc>,
    pub event: PricingEvent,
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
    events: Vec<PricingEvent>,
    models: Vec<ModelRateCard>,
    known_unpriced: Vec<KnownUnpricedModel>,
}

#[derive(Debug, Clone)]
struct ModelRateCard {
    key: String,
    label: String,
    versions: Vec<ModelPricingVersion>,
    note: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelPricingVersion {
    effective_at: Option<DateTime<Utc>>,
    event: Option<String>,
    input_credits_per_million: f64,
    cached_input_credits_per_million: f64,
    output_credits_per_million: f64,
    fast_credit_multiplier: f64,
}

#[derive(Debug, Deserialize)]
struct RawRateCard {
    source: RawRateCardSource,
    #[serde(default)]
    events: Vec<RawPricingEvent>,
    models: Vec<RawModelPricing>,
    #[serde(default)]
    #[serde(rename = "knownUnpriced")]
    known_unpriced: Vec<RawKnownUnpricedModel>,
}

#[derive(Debug, Deserialize)]
struct RawRateCardSource {
    name: String,
    url: String,
    checked_at: String,
    credit_to_usd: String,
}

#[derive(Debug, Deserialize)]
struct RawPricingEvent {
    key: String,
    title: String,
    announced_at: String,
    url: String,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawModelPricing {
    key: String,
    label: String,
    versions: Vec<RawModelPricingVersion>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawModelPricingVersion {
    effective_at: Option<String>,
    event: Option<String>,
    input_credits_per_million: f64,
    cached_input_credits_per_million: f64,
    output_credits_per_million: f64,
    fast_credit_multiplier: f64,
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
        .and_then(|model| {
            model
                .versions
                .last()
                .map(|version| model_pricing(model, version))
        })
}

pub fn get_model_pricing_at(model: &str, timestamp: DateTime<Utc>) -> Option<ModelPricing> {
    let key = pricing_key_for_model(model);
    rate_card()
        .models
        .iter()
        .find(|pricing| pricing.key == key)
        .and_then(|model| {
            pricing_version_at(model, timestamp).map(|version| model_pricing(model, version))
        })
}

pub fn list_model_pricing() -> Vec<ModelPricing> {
    let mut pricing = rate_card()
        .models
        .iter()
        .filter_map(|model| {
            model
                .versions
                .last()
                .map(|version| model_pricing(model, version))
        })
        .collect::<Vec<_>>();
    pricing.sort_by(|left, right| left.key.cmp(&right.key));
    pricing
}

pub fn list_model_pricing_changes() -> Vec<ModelPricingChange> {
    let events = rate_card()
        .events
        .iter()
        .map(|event| (event.key.as_str(), event))
        .collect::<HashMap<_, _>>();
    let mut changes = rate_card()
        .models
        .iter()
        .flat_map(|model| {
            model.versions.windows(2).map(|versions| {
                let old_version = &versions[0];
                let new_version = &versions[1];
                let effective_at = new_version
                    .effective_at
                    .expect("validated non-baseline pricing version has effective_at");
                let event_key = new_version
                    .event
                    .as_deref()
                    .expect("validated non-baseline pricing version has event");
                let event = events
                    .get(event_key)
                    .expect("validated pricing version event exists");
                ModelPricingChange {
                    model_key: model.key.clone(),
                    model_label: model.label.clone(),
                    old_pricing: model_pricing(model, old_version),
                    new_pricing: model_pricing(model, new_version),
                    effective_at,
                    event: (*event).clone(),
                }
            })
        })
        .collect::<Vec<_>>();
    changes.sort_by(|left, right| {
        left.effective_at
            .cmp(&right.effective_at)
            .then_with(|| left.model_key.cmp(&right.model_key))
    });
    changes
}

pub fn list_known_unpriced_models() -> Vec<KnownUnpricedModel> {
    let mut pricing = rate_card().known_unpriced.clone();
    pricing.sort_by(|left, right| left.key.cmp(&right.key));
    pricing
}

pub fn calculate_credit_cost(model: &str, usage: TokenUsage) -> CreditCost {
    calculate_credit_cost_with_context(model, usage, PricingContext::normal())
}

pub fn calculate_credit_cost_at(
    model: &str,
    usage: TokenUsage,
    timestamp: DateTime<Utc>,
) -> CreditCost {
    calculate_credit_cost_with_context_at(model, usage, PricingContext::normal(), timestamp)
}

pub fn calculate_credit_cost_with_context(
    model: &str,
    usage: TokenUsage,
    context: PricingContext,
) -> CreditCost {
    calculate_credit_cost_for_pricing(model, usage, context, get_model_pricing(model))
}

pub fn calculate_credit_cost_with_context_at(
    model: &str,
    usage: TokenUsage,
    context: PricingContext,
    timestamp: DateTime<Utc>,
) -> CreditCost {
    calculate_credit_cost_for_pricing(
        model,
        usage,
        context,
        get_model_pricing_at(model, timestamp),
    )
}

fn calculate_credit_cost_for_pricing(
    model: &str,
    usage: TokenUsage,
    context: PricingContext,
    pricing: Option<ModelPricing>,
) -> CreditCost {
    let cached_input_tokens = usage.cached_input_tokens.min(usage.input_tokens);
    let billable_input_tokens = usage.input_tokens.saturating_sub(cached_input_tokens);

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

fn pricing_version_at(
    model: &ModelRateCard,
    timestamp: DateTime<Utc>,
) -> Option<&ModelPricingVersion> {
    model.versions.iter().rev().find(|version| {
        version
            .effective_at
            .is_none_or(|effective_at| timestamp >= effective_at)
    })
}

fn model_pricing(model: &ModelRateCard, version: &ModelPricingVersion) -> ModelPricing {
    ModelPricing {
        key: model.key.clone(),
        label: model.label.clone(),
        input_credits_per_million: version.input_credits_per_million,
        cached_input_credits_per_million: version.cached_input_credits_per_million,
        output_credits_per_million: version.output_credits_per_million,
        fast_credit_multiplier: version.fast_credit_multiplier,
        note: model.note.clone(),
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
            url: raw.source.url,
            checked_at: raw.source.checked_at,
            credit_to_usd: raw.source.credit_to_usd,
            credits_per_usd,
        },
        events: convert_events(raw.events),
        models: convert_models(raw.models),
        known_unpriced: convert_known_unpriced_models(raw.known_unpriced),
    }
}

fn convert_events(raw: Vec<RawPricingEvent>) -> Vec<PricingEvent> {
    raw.into_iter()
        .map(|event| PricingEvent {
            key: event.key,
            title: event.title,
            announced_at: parse_timestamp(&event.announced_at, "events[].announced_at"),
            url: event.url,
            note: event.note,
        })
        .collect()
}

fn convert_models(raw: Vec<RawModelPricing>) -> Vec<ModelRateCard> {
    raw.into_iter()
        .map(|model| ModelRateCard {
            key: model.key,
            label: model.label,
            versions: model
                .versions
                .into_iter()
                .map(|version| ModelPricingVersion {
                    effective_at: version
                        .effective_at
                        .map(|value| parse_timestamp(&value, "models[].versions[].effective_at")),
                    event: version.event,
                    input_credits_per_million: version.input_credits_per_million,
                    cached_input_credits_per_million: version.cached_input_credits_per_million,
                    output_credits_per_million: version.output_credits_per_million,
                    fast_credit_multiplier: version.fast_credit_multiplier,
                })
                .collect(),
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

fn parse_timestamp(value: &str, path: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap_or_else(|error| {
            panic!("data/codex-rate-card.json field {path} must be RFC 3339: {error}")
        })
        .with_timezone(&Utc)
}

fn validate_rate_card(raw: &RawRateCard) {
    assert_non_empty(&raw.source.name, "source.name");
    assert_non_empty(&raw.source.url, "source.url");
    assert_non_empty(&raw.source.checked_at, "source.checked_at");
    assert_non_empty(&raw.source.credit_to_usd, "source.credit_to_usd");

    let mut event_keys = HashSet::new();
    for event in &raw.events {
        assert_non_empty(&event.key, "events[].key");
        assert_non_empty(&event.title, "events[].title");
        assert_non_empty(&event.announced_at, "events[].announced_at");
        assert_non_empty(&event.url, "events[].url");
        parse_timestamp(&event.announced_at, "events[].announced_at");
        if !event_keys.insert(event.key.as_str()) {
            panic!(
                "data/codex-rate-card.json has duplicate event key: {}",
                event.key
            );
        }
    }

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
        if model.versions.is_empty() {
            panic!(
                "data/codex-rate-card.json model {} must define at least one pricing version",
                model.key
            );
        }

        let mut previous_effective_at = None;
        for (index, version) in model.versions.iter().enumerate() {
            if index == 0 {
                if version.effective_at.is_some() {
                    panic!(
                        "data/codex-rate-card.json model {} first pricing version must use effective_at null",
                        model.key
                    );
                }
                if version.event.is_some() {
                    panic!(
                        "data/codex-rate-card.json model {} baseline pricing version cannot reference an event",
                        model.key
                    );
                }
            } else {
                let effective_at = version.effective_at.as_deref().unwrap_or_else(|| {
                    panic!(
                        "data/codex-rate-card.json model {} non-baseline pricing version must define effective_at",
                        model.key
                    )
                });
                let effective_at =
                    parse_timestamp(effective_at, "models[].versions[].effective_at");
                if previous_effective_at.is_some_and(|previous| effective_at <= previous) {
                    panic!(
                        "data/codex-rate-card.json model {} pricing version effective_at values must be strictly increasing",
                        model.key
                    );
                }
                previous_effective_at = Some(effective_at);

                let event = version.event.as_deref().unwrap_or_else(|| {
                    panic!(
                        "data/codex-rate-card.json model {} non-baseline pricing version must reference an event",
                        model.key
                    )
                });
                if !event_keys.contains(event) {
                    panic!(
                        "data/codex-rate-card.json model {} pricing version references unknown event: {}",
                        model.key, event
                    );
                }
            }

            assert_non_negative_finite(
                version.input_credits_per_million,
                "models[].versions[].input_credits_per_million",
            );
            assert_non_negative_finite(
                version.cached_input_credits_per_million,
                "models[].versions[].cached_input_credits_per_million",
            );
            assert_non_negative_finite(
                version.output_credits_per_million,
                "models[].versions[].output_credits_per_million",
            );
            assert_positive_finite(
                version.fast_credit_multiplier,
                "models[].versions[].fast_credit_multiplier",
            );
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
            ("gpt-5.6-sol", "GPT-5.6 Sol", 125.0, 12.5, 750.0),
            ("gpt-5.6-terra", "GPT-5.6 Terra", 50.0, 5.0, 300.0),
            ("gpt-5.6-luna", "GPT-5.6 Luna", 5.0, 0.5, 30.0),
        ];

        for (key, label, input, cached_input, output) in expected {
            let pricing = get_model_pricing(key).expect("GPT-5.6 pricing");
            assert_eq!(pricing.label, label);
            assert_eq!(pricing.input_credits_per_million, input);
            assert_eq!(pricing.cached_input_credits_per_million, cached_input);
            assert_eq!(pricing.output_credits_per_million, output);
            assert_eq!(pricing.fast_credit_multiplier, 2.5);
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
        let gpt56 =
            calculate_credit_cost_with_context("gpt-5.6-terra", usage, PricingContext::fast());

        assert_eq!(gpt55.credit_multiplier, 2.5);
        assert!((gpt55.credits - 0.81875).abs() < 0.000001);
        assert_eq!(gpt54.credit_multiplier, 2.0);
        assert!((gpt54.credits - 0.3275).abs() < 0.000001);
        assert_eq!(gpt56.credit_multiplier, 2.5);
        assert!((gpt56.credits - 0.3275).abs() < 0.000001);
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
        assert_eq!(
            CODEX_RATE_CARD_SOURCE.url,
            "https://help.openai.com/en/articles/20001106-codex-rate-card"
        );
        assert_eq!(CODEX_RATE_CARD_SOURCE.checked_at, "2026-08-04");
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
                    "url": "https://example.com/rate-card",
                    "checked_at": "2026-05-27",
                    "credit_to_usd": "50 credits = $1"
                },
                "events": [],
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "versions": [
                            {
                                "effective_at": null,
                                "input_credits_per_million": 1.0,
                                "cached_input_credits_per_million": 0.5,
                                "output_credits_per_million": 2.0,
                                "fast_credit_multiplier": 3.0
                            }
                        ]
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
        assert_eq!(rate_card.models[0].versions[0].fast_credit_multiplier, 3.0);
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
                    "url": "https://example.com/rate-card",
                    "checked_at": "2026-05-27",
                    "credit_to_usd": "50 credits = $1"
                },
                "events": [],
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "versions": [
                            {
                                "effective_at": null,
                                "input_credits_per_million": 1.0,
                                "cached_input_credits_per_million": 0.5,
                                "output_credits_per_million": 2.0,
                                "fast_credit_multiplier": 0.0
                            }
                        ]
                    }
                ],
                "knownUnpriced": []
            }"#,
        );
    }

    #[test]
    fn selects_historical_gpt_5_6_prices_at_the_cutoff() {
        let cutoff = parse_timestamp("2026-07-30T17:17:05.167Z", "test.cutoff");
        let before = cutoff - chrono::Duration::milliseconds(1);
        let after = cutoff + chrono::Duration::milliseconds(1);
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            cached_input_tokens: 0,
            output_tokens: 0,
        };

        let terra_before = calculate_credit_cost_at("gpt-5.6-terra", usage, before);
        let terra_at = calculate_credit_cost_at("gpt-5.6-terra", usage, cutoff);
        let terra_after = calculate_credit_cost_at("gpt-5.6-terra", usage, after);
        let luna_before = calculate_credit_cost_at("gpt-5.6-luna", usage, before);
        let luna_at = calculate_credit_cost_at("gpt-5.6-luna", usage, cutoff);

        assert_eq!(terra_before.credits, 62.5);
        assert_eq!(terra_at.credits, 50.0);
        assert_eq!(terra_after.credits, 50.0);
        assert_eq!(luna_before.credits, 25.0);
        assert_eq!(luna_at.credits, 5.0);
        assert_eq!(calculate_credit_cost("gpt-5.6-terra", usage).credits, 50.0);
        assert_eq!(calculate_credit_cost("gpt-5.6-luna", usage).credits, 5.0);
    }

    #[test]
    fn applies_gpt_5_6_fast_multiplier_to_old_and_new_prices() {
        let cutoff = parse_timestamp("2026-07-30T17:17:05.167Z", "test.cutoff");
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            cached_input_tokens: 0,
            output_tokens: 0,
        };

        let old = calculate_credit_cost_with_context_at(
            "gpt-5.6-terra",
            usage,
            PricingContext::fast(),
            cutoff - chrono::Duration::milliseconds(1),
        );
        let new = calculate_credit_cost_with_context_at(
            "gpt-5.6-terra",
            usage,
            PricingContext::fast(),
            cutoff,
        );

        assert_eq!(old.credit_multiplier, 2.5);
        assert_eq!(old.credits, 156.25);
        assert_eq!(new.credit_multiplier, 2.5);
        assert_eq!(new.credits, 125.0);
    }

    #[test]
    fn lists_price_changes_with_their_shared_event() {
        let changes = list_model_pricing_changes();
        let keys = changes
            .iter()
            .map(|change| change.model_key.as_str())
            .collect::<Vec<_>>();

        assert_eq!(keys, vec!["gpt-5.6-luna", "gpt-5.6-terra"]);
        for change in changes {
            assert_eq!(
                change.effective_at,
                parse_timestamp("2026-07-30T17:17:05.167Z", "test.cutoff",)
            );
            assert_eq!(change.event.key, "2026-07-30-gpt-5.6-price-reduction");
            assert_eq!(change.old_pricing.fast_credit_multiplier, 2.5);
            assert_eq!(change.new_pricing.fast_credit_multiplier, 2.5);
        }
    }

    #[test]
    #[should_panic(expected = "effective_at values must be strictly increasing")]
    fn out_of_order_pricing_versions_are_rejected() {
        load_rate_card_from_str(
            r#"{
                "source": {
                    "name": "test",
                    "url": "https://example.com/rate-card",
                    "checked_at": "2026-08-04",
                    "credit_to_usd": "25 credits = $1"
                },
                "events": [
                    {
                        "key": "change",
                        "title": "Change",
                        "announced_at": "2026-07-30T17:17:05.167Z",
                        "url": "https://example.com/change"
                    }
                ],
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "versions": [
                            {
                                "effective_at": null,
                                "input_credits_per_million": 3.0,
                                "cached_input_credits_per_million": 0.3,
                                "output_credits_per_million": 6.0,
                                "fast_credit_multiplier": 1.0
                            },
                            {
                                "effective_at": "2026-08-02T00:00:00Z",
                                "event": "change",
                                "input_credits_per_million": 2.0,
                                "cached_input_credits_per_million": 0.2,
                                "output_credits_per_million": 4.0,
                                "fast_credit_multiplier": 1.0
                            },
                            {
                                "effective_at": "2026-08-01T00:00:00Z",
                                "event": "change",
                                "input_credits_per_million": 1.0,
                                "cached_input_credits_per_million": 0.1,
                                "output_credits_per_million": 2.0,
                                "fast_credit_multiplier": 1.0
                            }
                        ]
                    }
                ],
                "knownUnpriced": []
            }"#,
        );
    }

    #[test]
    #[should_panic(expected = "references unknown event")]
    fn unknown_pricing_event_is_rejected() {
        load_rate_card_from_str(
            r#"{
                "source": {
                    "name": "test",
                    "url": "https://example.com/rate-card",
                    "checked_at": "2026-08-04",
                    "credit_to_usd": "25 credits = $1"
                },
                "events": [],
                "models": [
                    {
                        "key": "priced-model",
                        "label": "Priced Model",
                        "versions": [
                            {
                                "effective_at": null,
                                "input_credits_per_million": 2.0,
                                "cached_input_credits_per_million": 0.2,
                                "output_credits_per_million": 4.0,
                                "fast_credit_multiplier": 1.0
                            },
                            {
                                "effective_at": "2026-08-01T00:00:00Z",
                                "event": "missing",
                                "input_credits_per_million": 1.0,
                                "cached_input_credits_per_million": 0.1,
                                "output_credits_per_million": 2.0,
                                "fast_credit_multiplier": 1.0
                            }
                        ]
                    }
                ],
                "knownUnpriced": []
            }"#,
        );
    }
}
