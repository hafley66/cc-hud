/// Centralized model registry: pricing, energy coefficients, context windows, provider metadata.
///
/// Single source of truth for all model-specific constants. Replaces scattered
/// `model_pricing()`, `model_context_window()`, and `ModelTier::from_model_str()`.
///
/// Energy coefficients (J/output_token) are the core number. Everything else in the
/// energy pipeline derives from it. For models with unknown parameter counts, the
/// estimation chain is:
///   1. Published parameter count (Llama, DeepSeek) -> direct J/tok from benchmarks
///   2. Benchmark-class equivalence (performs like Sonnet -> Sonnet-class energy)
///   3. Pricing bracket as last resort (loose correlation, lots of margin noise)
///
/// For MoE models, active parameter count determines energy, not total params.

/// Energy class. Determines J/output_token when a model profile doesn't override
/// with an explicit value. Kept as a coarse bucketing for models where we only
/// know "it's roughly 8B-class" or "it's frontier-class."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnergyTier {
    /// ~8B active params. 0.15 J/output_tok.
    Small,
    /// ~70B active params. 0.9 J/output_tok.
    Medium,
    /// ~200B+ active/dense params. 5.0 J/output_tok.
    Large,
}

impl EnergyTier {
    pub fn j_per_output_tok(self) -> f64 {
        match self {
            EnergyTier::Small => 0.15,
            EnergyTier::Medium => 0.9,
            EnergyTier::Large => 5.0,
        }
    }
}

/// Token pricing per 1M tokens (USD).
#[derive(Debug, Clone, Copy)]
pub struct TokenPricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_create: f64,
}

impl TokenPricing {
    /// Compute (input_cost, output_cost) for given token counts.
    pub fn cost_split(
        &self,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_create: u64,
    ) -> (f64, f64) {
        let in_cost = (input as f64 * self.input
            + cache_read as f64 * self.cache_read
            + cache_create as f64 * self.cache_create)
            / 1_000_000.0;
        let out_cost = (output as f64 * self.output) / 1_000_000.0;
        (in_cost, out_cost)
    }
}

/// Energy coefficients for a specific model.
#[derive(Debug, Clone, Copy)]
pub struct EnergyCoefficients {
    /// Joules per output token (decode phase). The primary number.
    pub j_per_output_tok: f64,
    /// Input energy as fraction of output energy. Default 0.05 (5%).
    pub input_energy_factor: f64,
}

impl EnergyCoefficients {
    pub fn from_tier(tier: EnergyTier) -> Self {
        Self {
            j_per_output_tok: tier.j_per_output_tok(),
            input_energy_factor: 0.05,
        }
    }

    pub fn j_per_input_tok(&self) -> f64 {
        self.j_per_output_tok * self.input_energy_factor
    }
}

/// Complete profile for a model.
#[derive(Debug, Clone)]
pub struct ModelProfile {
    pub provider: &'static str,
    /// Pattern(s) that match model ID strings. Checked with `contains()`.
    pub patterns: &'static [&'static str],
    pub display_name: &'static str,
    pub energy_tier: EnergyTier,
    pub energy: EnergyCoefficients,
    pub pricing: TokenPricing,
    pub context_window: u64,
    /// Estimated active parameter count in billions (for display/sorting). 0 = unknown.
    pub active_params_b: f64,
}

/// Fallback profile for unrecognized models. Sonnet-class energy, $3/$15 pricing.
pub const FALLBACK_PROFILE: ModelProfile = ModelProfile {
    provider: "unknown",
    patterns: &[],
    display_name: "Unknown Model",
    energy_tier: EnergyTier::Medium,
    energy: EnergyCoefficients {
        j_per_output_tok: 0.9,
        input_energy_factor: 0.05,
    },
    pricing: TokenPricing {
        input: 3.0,
        output: 15.0,
        cache_read: 0.30,
        cache_create: 6.0,
    },
    context_window: 200_000,
    active_params_b: 0.0,
};

/// Built-in model profiles. Ordered by provider, then by size descending.
/// Pattern matching is first-match-wins, so more specific patterns come first.
static BUILTIN_PROFILES: &[ModelProfile] = &[
    // -----------------------------------------------------------------------
    // Anthropic
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "anthropic",
        patterns: &["opus-4-6", "opus-4-5"],
        display_name: "Claude Opus 4.6/4.5",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 5.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
            cache_create: 10.0,
        },
        context_window: 1_000_000,
        active_params_b: 200.0,
    },
    ModelProfile {
        provider: "anthropic",
        patterns: &["opus"],
        display_name: "Claude Opus 4.1/4",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 5.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 15.0,
            output: 75.0,
            cache_read: 1.50,
            cache_create: 30.0,
        },
        context_window: 200_000,
        active_params_b: 200.0,
    },
    ModelProfile {
        provider: "anthropic",
        patterns: &["sonnet"],
        display_name: "Claude Sonnet",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.9,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_create: 6.0,
        },
        context_window: 200_000,
        active_params_b: 70.0,
    },
    ModelProfile {
        provider: "anthropic",
        patterns: &["haiku-4-5"],
        display_name: "Claude Haiku 4.5",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 1.0,
            output: 5.0,
            cache_read: 0.10,
            cache_create: 2.0,
        },
        context_window: 200_000,
        active_params_b: 8.0,
    },
    ModelProfile {
        provider: "anthropic",
        patterns: &["haiku"],
        display_name: "Claude Haiku 3.5",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.80,
            output: 4.0,
            cache_read: 0.08,
            cache_create: 1.60,
        },
        context_window: 200_000,
        active_params_b: 8.0,
    },
    // -----------------------------------------------------------------------
    // OpenAI
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "openai",
        patterns: &["gpt-4o-mini"],
        display_name: "GPT-4o Mini",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.15,
            output: 0.60,
            cache_read: 0.075,
            cache_create: 0.15,
        },
        context_window: 128_000,
        active_params_b: 8.0,
    },
    ModelProfile {
        provider: "openai",
        patterns: &["gpt-4o"],
        display_name: "GPT-4o",
        energy_tier: EnergyTier::Medium,
        // MoE ~200B total, ~50B active. Between Small and Medium, closer to Medium.
        energy: EnergyCoefficients {
            j_per_output_tok: 0.6,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 2.50,
            output: 10.0,
            cache_read: 1.25,
            cache_create: 2.50,
        },
        context_window: 128_000,
        active_params_b: 50.0,
    },
    ModelProfile {
        provider: "openai",
        patterns: &["o3-mini"],
        display_name: "o3-mini",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 1.10,
            output: 4.40,
            cache_read: 0.55,
            cache_create: 1.10,
        },
        context_window: 200_000,
        active_params_b: 8.0,
    },
    ModelProfile {
        provider: "openai",
        patterns: &["o3"],
        display_name: "o3",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 5.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 10.0,
            output: 40.0,
            cache_read: 2.50,
            cache_create: 10.0,
        },
        context_window: 200_000,
        active_params_b: 200.0,
    },
    ModelProfile {
        provider: "openai",
        patterns: &["o4-mini"],
        display_name: "o4-mini",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 1.10,
            output: 4.40,
            cache_read: 0.55,
            cache_create: 1.10,
        },
        context_window: 200_000,
        active_params_b: 8.0,
    },
    // -----------------------------------------------------------------------
    // DeepSeek
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "deepseek",
        patterns: &["deepseek-v3", "deepseek-chat"],
        display_name: "DeepSeek V3",
        energy_tier: EnergyTier::Medium,
        // 671B total MoE, 37B active. Closer to Small on energy, but denser routing.
        energy: EnergyCoefficients {
            j_per_output_tok: 0.5,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.27,
            output: 1.10,
            cache_read: 0.07,
            cache_create: 0.27,
        },
        context_window: 128_000,
        active_params_b: 37.0,
    },
    ModelProfile {
        provider: "deepseek",
        patterns: &["deepseek-r1", "deepseek-reasoner"],
        display_name: "DeepSeek R1",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.5,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.55,
            output: 2.19,
            cache_read: 0.14,
            cache_create: 0.55,
        },
        context_window: 128_000,
        active_params_b: 37.0,
    },
    // -----------------------------------------------------------------------
    // Moonshot / Kimi
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "moonshot",
        patterns: &["kimi-k2.5", "kimi-k2-5"],
        display_name: "Kimi K2.5",
        energy_tier: EnergyTier::Large,
        // Frontier-class performance. MoE, estimated ~1T total, ~100B+ active.
        energy: EnergyCoefficients {
            j_per_output_tok: 3.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 2.0,
            output: 8.0,
            cache_read: 0.50,
            cache_create: 2.0,
        },
        context_window: 131_072,
        active_params_b: 100.0,
    },
    ModelProfile {
        provider: "moonshot",
        patterns: &["kimi-k2", "moonshot-v1"],
        display_name: "Kimi K2",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.9,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 1.0,
            output: 4.0,
            cache_read: 0.25,
            cache_create: 1.0,
        },
        context_window: 131_072,
        active_params_b: 70.0,
    },
    // -----------------------------------------------------------------------
    // Google
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "google",
        patterns: &["gemini-2.5-pro"],
        display_name: "Gemini 2.5 Pro",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 3.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 1.25,
            output: 10.0,
            cache_read: 0.31,
            cache_create: 1.25,
        },
        context_window: 1_000_000,
        active_params_b: 100.0,
    },
    ModelProfile {
        provider: "google",
        patterns: &["gemini-2.5-flash"],
        display_name: "Gemini 2.5 Flash",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.15,
            output: 0.60,
            cache_read: 0.04,
            cache_create: 0.15,
        },
        context_window: 1_000_000,
        active_params_b: 8.0,
    },
    ModelProfile {
        provider: "google",
        patterns: &["gemini-2.0-flash", "gemini-flash"],
        display_name: "Gemini 2.0 Flash",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.10,
            output: 0.40,
            cache_read: 0.025,
            cache_create: 0.10,
        },
        context_window: 1_000_000,
        active_params_b: 8.0,
    },
    // -----------------------------------------------------------------------
    // Meta (Llama via Together, Groq, etc.)
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "meta",
        patterns: &["llama-4-maverick"],
        display_name: "Llama 4 Maverick",
        energy_tier: EnergyTier::Medium,
        // 400B MoE, ~17B active per expert, 128 experts, 1 active. ~17B active.
        energy: EnergyCoefficients {
            j_per_output_tok: 0.3,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.27,
            output: 0.85,
            cache_read: 0.07,
            cache_create: 0.27,
        },
        context_window: 1_000_000,
        active_params_b: 17.0,
    },
    ModelProfile {
        provider: "meta",
        patterns: &["llama-4-scout"],
        display_name: "Llama 4 Scout",
        energy_tier: EnergyTier::Small,
        // 109B MoE, ~17B active.
        energy: EnergyCoefficients {
            j_per_output_tok: 0.3,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.18,
            output: 0.59,
            cache_read: 0.05,
            cache_create: 0.18,
        },
        context_window: 512_000,
        active_params_b: 17.0,
    },
    ModelProfile {
        provider: "meta",
        patterns: &["llama-3.3-70b", "llama-3.1-70b", "llama-3-70b"],
        display_name: "Llama 3.x 70B",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.9,
            input_energy_factor: 0.05,
        },
        // Together AI pricing
        pricing: TokenPricing {
            input: 0.88,
            output: 0.88,
            cache_read: 0.22,
            cache_create: 0.88,
        },
        context_window: 131_072,
        active_params_b: 70.0,
    },
    ModelProfile {
        provider: "meta",
        patterns: &["llama-3.1-8b", "llama-3-8b"],
        display_name: "Llama 3.x 8B",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.15,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.18,
            output: 0.18,
            cache_read: 0.05,
            cache_create: 0.18,
        },
        context_window: 131_072,
        active_params_b: 8.0,
    },
    ModelProfile {
        provider: "meta",
        patterns: &["llama-3.1-405b", "llama-3-405b"],
        display_name: "Llama 3.1 405B",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 5.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 3.50,
            output: 3.50,
            cache_read: 0.88,
            cache_create: 3.50,
        },
        context_window: 131_072,
        active_params_b: 405.0,
    },
    // -----------------------------------------------------------------------
    // Mistral
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "mistral",
        patterns: &["mistral-large"],
        display_name: "Mistral Large",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.9,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 2.0,
            output: 6.0,
            cache_read: 0.50,
            cache_create: 2.0,
        },
        context_window: 128_000,
        active_params_b: 70.0,
    },
    ModelProfile {
        provider: "mistral",
        patterns: &["codestral"],
        display_name: "Codestral",
        energy_tier: EnergyTier::Small,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.3,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.30,
            output: 0.90,
            cache_read: 0.08,
            cache_create: 0.30,
        },
        context_window: 256_000,
        active_params_b: 22.0,
    },
    // -----------------------------------------------------------------------
    // Together AI hosted (GLM, Qwen, etc.)
    // -----------------------------------------------------------------------
    ModelProfile {
        provider: "together",
        patterns: &["GLM-5", "glm-5"],
        display_name: "GLM-5",
        energy_tier: EnergyTier::Large,
        energy: EnergyCoefficients {
            j_per_output_tok: 3.0,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 2.0,
            output: 8.0,
            cache_read: 0.50,
            cache_create: 2.0,
        },
        context_window: 128_000,
        active_params_b: 100.0,
    },
    ModelProfile {
        provider: "together",
        patterns: &["qwen-2.5-72b", "qwen2.5-72b", "Qwen2.5-72B"],
        display_name: "Qwen 2.5 72B",
        energy_tier: EnergyTier::Medium,
        energy: EnergyCoefficients {
            j_per_output_tok: 0.9,
            input_energy_factor: 0.05,
        },
        pricing: TokenPricing {
            input: 0.90,
            output: 0.90,
            cache_read: 0.22,
            cache_create: 0.90,
        },
        context_window: 131_072,
        active_params_b: 72.0,
    },
];

/// Look up a model profile by model ID string. First match wins.
/// Falls back to FALLBACK_PROFILE for unrecognized models.
pub fn lookup(model: &str) -> &'static ModelProfile {
    let lower = model.to_lowercase();
    for profile in BUILTIN_PROFILES {
        for pattern in profile.patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return profile;
            }
        }
    }
    &FALLBACK_PROFILE
}

/// Convenience: get pricing tuple in the old (input, output, cache_read, cache_create) format.
pub fn model_pricing(model: &str) -> (f64, f64, f64, f64) {
    let p = &lookup(model).pricing;
    (p.input, p.output, p.cache_read, p.cache_create)
}

/// Convenience: get context window for a model string.
pub fn model_context_window(model: &str) -> u64 {
    lookup(model).context_window
}

/// Convenience: get energy coefficients for a model string.
pub fn model_energy(model: &str) -> &'static EnergyCoefficients {
    &lookup(model).energy
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_models_match() {
        let opus46 = lookup("claude-opus-4-6");
        assert_eq!(opus46.display_name, "Claude Opus 4.6/4.5");
        assert_eq!(opus46.context_window, 1_000_000);
        assert!((opus46.pricing.output - 25.0).abs() < 0.01);

        let opus41 = lookup("claude-opus-4-1-20260301");
        assert_eq!(opus41.display_name, "Claude Opus 4.1/4");
        assert!((opus41.pricing.output - 75.0).abs() < 0.01);

        let sonnet = lookup("claude-sonnet-4-5-20260101");
        assert_eq!(sonnet.display_name, "Claude Sonnet");

        let haiku45 = lookup("claude-haiku-4-5-20251001");
        assert_eq!(haiku45.display_name, "Claude Haiku 4.5");

        let haiku35 = lookup("claude-haiku-3-5-20241022");
        assert_eq!(haiku35.display_name, "Claude Haiku 3.5");
    }

    #[test]
    fn openai_models_match() {
        assert_eq!(lookup("gpt-4o-mini-2024-07-18").display_name, "GPT-4o Mini");
        assert_eq!(lookup("gpt-4o-2024-08-06").display_name, "GPT-4o");
        assert_eq!(lookup("o3-mini-2025-01-31").display_name, "o3-mini");
        assert_eq!(lookup("o3-2025-04-16").display_name, "o3");
    }

    #[test]
    fn deepseek_models_match() {
        assert_eq!(lookup("deepseek-chat").display_name, "DeepSeek V3");
        assert_eq!(lookup("deepseek-reasoner").display_name, "DeepSeek R1");
    }

    #[test]
    fn kimi_models_match() {
        assert_eq!(lookup("kimi-k2.5").display_name, "Kimi K2.5");
        // OpenCode might store as kimi-k2-5
        assert_eq!(lookup("kimi-k2-5").display_name, "Kimi K2.5");
    }

    #[test]
    fn together_glm_match() {
        // OpenCode on this machine uses "zai-org/GLM-5" via together
        assert_eq!(lookup("zai-org/GLM-5").display_name, "GLM-5");
    }

    #[test]
    fn fallback_for_unknown() {
        let p = lookup("some-random-model-nobody-heard-of");
        assert_eq!(p.provider, "unknown");
        assert!((p.energy.j_per_output_tok - 0.9).abs() < 0.01);
    }

    #[test]
    fn pricing_compat() {
        let (pi, po, pcr, pcc) = model_pricing("claude-opus-4-6");
        assert!((pi - 5.0).abs() < 0.01);
        assert!((po - 25.0).abs() < 0.01);
        assert!((pcr - 0.50).abs() < 0.01);
        assert!((pcc - 6.25).abs() < 0.01);
    }

    #[test]
    fn context_window_compat() {
        assert_eq!(model_context_window("claude-opus-4-6"), 1_000_000);
        assert_eq!(model_context_window("claude-sonnet-4-5"), 200_000);
        assert_eq!(model_context_window("gpt-4o"), 128_000);
    }

    #[test]
    fn energy_coefficients() {
        let e = model_energy("claude-opus-4-6");
        assert!((e.j_per_output_tok - 5.0).abs() < 0.01);
        assert!((e.j_per_input_tok() - 0.25).abs() < 0.01);

        let e = model_energy("deepseek-v3");
        assert!((e.j_per_output_tok - 0.5).abs() < 0.01);
    }
}
