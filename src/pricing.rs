use crate::model::TokenUsage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TokenRates {
    input_nano_usd: u64,
    output_nano_usd: u64,
}

pub fn estimate_cost_nano_usd(model: &str, usage: &TokenUsage) -> Option<u128> {
    let rates = rates_for_model(model)?;
    let input_tokens = u128::from(usage.prompt_tokens?);
    let output_tokens = u128::from(usage.completion_tokens?);
    Some(
        input_tokens
            .saturating_mul(u128::from(rates.input_nano_usd))
            .saturating_add(output_tokens.saturating_mul(u128::from(rates.output_nano_usd))),
    )
}

fn rates_for_model(model: &str) -> Option<TokenRates> {
    // Standard API pricing per token, represented as billionths of a US dollar.
    // The estimate intentionally excludes cached-input discounts, long-context
    // multipliers, regional processing, Batch, Flex, and Priority processing.
    let (input_nano_usd, output_nano_usd) =
        if model == "gpt-5.6" || matches_model(model, "gpt-5.6-sol") {
            (5_000, 30_000)
        } else if matches_model(model, "gpt-5.6-terra") {
            (2_500, 15_000)
        } else if matches_model(model, "gpt-5.6-luna") {
            (1_000, 6_000)
        } else if matches_model(model, "gpt-5.5") {
            (5_000, 30_000)
        } else if matches_model(model, "gpt-5.4-mini") {
            (750, 4_500)
        } else if matches_model(model, "gpt-5.4-nano") {
            (200, 1_250)
        } else if matches_model(model, "gpt-5.4") {
            (2_500, 15_000)
        } else if matches_model(model, "gpt-5-mini") {
            (250, 2_000)
        } else if matches_model(model, "gpt-4.1-mini") {
            (400, 1_600)
        } else if matches_model(model, "gpt-4o-mini") {
            (150, 600)
        } else {
            return None;
        };
    Some(TokenRates {
        input_nano_usd,
        output_nano_usd,
    })
}

fn matches_model(model: &str, alias: &str) -> bool {
    model == alias
        || model
            .strip_prefix(alias)
            .is_some_and(|suffix| suffix.starts_with("-20"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: Some(input),
            completion_tokens: Some(output),
            total_tokens: Some(input + output),
        }
    }

    #[test]
    fn estimates_known_models_and_snapshots() {
        assert_eq!(
            estimate_cost_nano_usd("gpt-5.6-luna", &usage(1_000, 100)),
            Some(1_600_000)
        );
        assert_eq!(
            estimate_cost_nano_usd("gpt-4.1-mini-2025-04-14", &usage(1_000, 100)),
            Some(560_000)
        );
    }

    #[test]
    fn declines_to_guess_without_pricing_or_split_usage() {
        assert_eq!(estimate_cost_nano_usd("local-model", &usage(10, 5)), None);
        assert_eq!(
            estimate_cost_nano_usd(
                "gpt-5.6-luna",
                &TokenUsage {
                    total_tokens: Some(15),
                    ..TokenUsage::default()
                }
            ),
            None
        );
    }
}
