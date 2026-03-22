/// Energy, carbon, and local cost projection from API token usage.
///
/// Pure computation module. No UI dependencies. Takes token counts and model tier,
/// returns physical estimates (joules, kWh, gCO2, USD, seconds-of-solar).
///
/// Research basis (March 2026):
/// - Per-token energy: TokenPowerBench (arxiv 2512.03024), "From Words to Watts" (2310.03003),
///   "Where Do the Joules Go?" (2601.22076), Epoch AI estimates
/// - Carbon intensity: EPA eGRID 2023, Electricity Maps, CO2.js/Ember annual averages
/// - Solar: NREL ATB 2024, EIA capacity factors by state
/// - Local GPU: TokenPowerBench 4090 benchmarks, llm-tracker.info
/// - Electricity pricing: EIA retail sales data 2025-2026

// ---------------------------------------------------------------------------
// Model tiers
// ---------------------------------------------------------------------------

/// Coarse model tier for energy estimation. Exact parameter counts are unknown;
/// tiers are inferred from pricing ratios and third-party benchmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTier {
    /// ~8B parameters (Haiku class)
    Haiku,
    /// ~70B parameters (Sonnet class)
    Sonnet,
    /// ~200B+ parameters (Opus class)
    Opus,
}

impl ModelTier {
    pub fn from_model_str(model: &str) -> Self {
        if model.contains("opus") {
            ModelTier::Opus
        } else if model.contains("haiku") {
            ModelTier::Haiku
        } else {
            // Sonnet is the default/fallback, same as pricing
            ModelTier::Sonnet
        }
    }
}

// ---------------------------------------------------------------------------
// Energy coefficients (server-side, H100 FP8)
// ---------------------------------------------------------------------------

/// Joules per output token (decode phase) by model tier.
/// Midpoints of research ranges. Uncertainty is ±3-5x on absolutes,
/// but inter-tier ratios are more stable.
///
/// Sources:
/// - Haiku (8B): 0.05-0.3 J range, midpoint 0.15 J (TokenPowerBench Llama-3-8B)
/// - Sonnet (70B): 0.4-1.5 J range, midpoint 0.9 J (TokenPowerBench Llama-3-70B FP8)
/// - Opus (200B+): 2-10 J range, midpoint 5.0 J (extrapolated, ~1.5x power scaling from 70B)
pub fn output_joules_per_token(tier: ModelTier) -> f64 {
    match tier {
        ModelTier::Haiku => 0.15,
        ModelTier::Sonnet => 0.9,
        ModelTier::Opus => 5.0,
    }
}

/// Joules per fresh input token (prefill phase). Prefill is compute-bound with high
/// GPU utilization, making it ~5% of decode cost per token.
/// Cache-hit tokens skip prefill entirely and cost effectively zero.
///
/// Source: "Where Do the Joules Go?" (arxiv 2601.22076) -- prefill <= 3.4% of total energy.
/// Using 5% as conservative multiplier.
const INPUT_TO_OUTPUT_ENERGY_RATIO: f64 = 0.05;

pub fn input_joules_per_token(tier: ModelTier) -> f64 {
    output_joules_per_token(tier) * INPUT_TO_OUTPUT_ENERGY_RATIO
}

/// Cache-hit tokens skip the forward pass. Energy cost is a memory read
/// of cached KV states -- negligible relative to compute.
/// Anthropic bills cache reads at 10% of input price, but even that overstates
/// the energy: the pricing covers infrastructure amortization, not just watts.
pub const CACHE_HIT_ENERGY_FACTOR: f64 = 0.0;

// ---------------------------------------------------------------------------
// Datacenter overhead
// ---------------------------------------------------------------------------

/// Power Usage Effectiveness. Ratio of total facility power to IT equipment power.
/// AWS does not publish fleet PUE; 1.2 is a reasonable hyperscale estimate.
/// Google Cloud reports 1.09. Industry average is ~1.8.
pub const DEFAULT_PUE: f64 = 1.2;

// ---------------------------------------------------------------------------
// Carbon intensity (gCO2eq/kWh)
// ---------------------------------------------------------------------------

/// Grid carbon intensity for major cloud regions.
/// Sources: EPA eGRID 2023, Electricity Maps annual averages, lowcarbonpower.org
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridRegion {
    /// us-east-1 (N. Virginia) -- eGRID RFCE ~318-347, conservative 350
    UsEast1,
    /// us-east-2 (Ohio) -- eGRID RFCW ~449
    UsEast2,
    /// us-west-2 (Oregon) -- lowcarbonpower ~200 (cleaner than NWPP aggregate)
    UsWest2,
    /// eu-west-1 (Ireland) -- ~298
    EuWest1,
    /// eu-west-3 (Paris, France) -- ~85, heavy nuclear
    EuWest3,
    /// eu-central-1 (Frankfurt, Germany) -- ~350
    EuCentral1,
    /// US national average
    UsAverage,
    /// World average (CO2.js/Ember)
    WorldAverage,
    /// User-provided value
    Custom(u32),
}

impl GridRegion {
    /// gCO2eq per kWh
    pub fn intensity(&self) -> f64 {
        match self {
            GridRegion::UsEast1 => 350.0,
            GridRegion::UsEast2 => 449.0,
            GridRegion::UsWest2 => 200.0,
            GridRegion::EuWest1 => 298.0,
            GridRegion::EuWest3 => 85.0,
            GridRegion::EuCentral1 => 350.0,
            GridRegion::UsAverage => 367.0,
            GridRegion::WorldAverage => 442.0,
            GridRegion::Custom(v) => *v as f64,
        }
    }
}

/// Default: us-east-1, where most Anthropic Bedrock inference runs.
pub const DEFAULT_GRID_REGION: GridRegion = GridRegion::UsEast1;

// ---------------------------------------------------------------------------
// Local GPU reference (consumer hardware equivalence)
// ---------------------------------------------------------------------------

/// RTX 4090 running a quantized 70B model (Q4/Q5).
/// Source: TokenPowerBench -- 125W GPU draw, ~14 tok/s decode.
/// Full system draw ~275-325W; using 300W as reference.
pub struct GpuProfile {
    pub name: &'static str,
    /// Watts, full system (GPU + CPU + RAM + cooling)
    pub system_watts: f64,
    /// Output tokens per second (decode, 70B Q5)
    pub tokens_per_sec: f64,
}

pub const GPU_4090: GpuProfile = GpuProfile {
    name: "RTX 4090",
    system_watts: 300.0,
    tokens_per_sec: 14.0,
};

/// Joules per token for local GPU inference.
pub fn local_joules_per_token(gpu: &GpuProfile) -> f64 {
    gpu.system_watts / gpu.tokens_per_sec
}

/// kWh per million tokens on local hardware.
pub fn local_kwh_per_million_tokens(gpu: &GpuProfile) -> f64 {
    local_joules_per_token(gpu) * 1_000_000.0 / 3_600_000.0
}

// ---------------------------------------------------------------------------
// Electricity pricing (USD/kWh)
// ---------------------------------------------------------------------------

/// US national average residential rate, 2025-2026.
/// Source: EIA -- 17.29 cents/kWh (2025), forecast 18.02 (2026).
pub const US_AVG_ELECTRICITY_RATE: f64 = 0.17;

/// Notable state rates for reference/presets.
pub const RATE_HAWAII: f64 = 0.42;
pub const RATE_CALIFORNIA: f64 = 0.33;
pub const RATE_NEW_ENGLAND: f64 = 0.30;
pub const RATE_TEXAS: f64 = 0.16;
pub const RATE_NORTH_DAKOTA: f64 = 0.11;

// ---------------------------------------------------------------------------
// Solar offset
// ---------------------------------------------------------------------------

/// 400W rated panel, US average residential capacity factor 17%.
/// Average continuous output: 400 * 0.17 = 68W.
/// Source: NREL ATB 2024, EIA state capacity factors.
pub struct SolarProfile {
    pub rated_watts: f64,
    pub capacity_factor: f64,
}

pub const SOLAR_US_AVG: SolarProfile = SolarProfile {
    rated_watts: 400.0,
    capacity_factor: 0.17,
};

pub const SOLAR_SOUTHWEST: SolarProfile = SolarProfile {
    rated_watts: 400.0,
    capacity_factor: 0.25,
};

pub const SOLAR_NORTHEAST: SolarProfile = SolarProfile {
    rated_watts: 400.0,
    capacity_factor: 0.14,
};

impl SolarProfile {
    /// Average continuous output in watts.
    pub fn avg_watts(&self) -> f64 {
        self.rated_watts * self.capacity_factor
    }

    /// kWh produced per second of operation.
    pub fn kwh_per_second(&self) -> f64 {
        self.avg_watts() / 3_600_000.0
    }
}

// ---------------------------------------------------------------------------
// Config (user overrides with defaults)
// ---------------------------------------------------------------------------

pub struct EnergyConfig {
    pub electricity_rate: f64,
    pub grid_region: GridRegion,
    pub pue: f64,
    pub solar: SolarProfile,
    pub local_gpu: GpuProfile,
}

impl Default for EnergyConfig {
    fn default() -> Self {
        Self {
            electricity_rate: US_AVG_ELECTRICITY_RATE,
            grid_region: DEFAULT_GRID_REGION,
            pue: DEFAULT_PUE,
            solar: SOLAR_US_AVG,
            local_gpu: GPU_4090,
        }
    }
}

// ---------------------------------------------------------------------------
// Core computation
// ---------------------------------------------------------------------------

/// Token counts for a single API call or aggregated session.
#[derive(Debug, Clone, Default)]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_create_tokens: u64,
}

/// Low/high confidence bounds. The J/token coefficients have ±3-5x uncertainty
/// on absolutes. We use 3x as the band: low = midpoint/3, high = midpoint*3.
/// Everything downstream (kWh, carbon, cost) scales linearly from joules,
/// so the band propagates without compounding.
const CONFIDENCE_LOW_FACTOR: f64 = 1.0 / 3.0;
const CONFIDENCE_HIGH_FACTOR: f64 = 3.0;

/// A value with low/mid/high confidence range.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ranged {
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

impl Ranged {
    fn from_mid(mid: f64) -> Self {
        Self {
            low: mid * CONFIDENCE_LOW_FACTOR,
            mid,
            high: mid * CONFIDENCE_HIGH_FACTOR,
        }
    }

    /// Scale all three bands by a constant factor.
    fn scale(self, factor: f64) -> Self {
        Self { low: self.low * factor, mid: self.mid * factor, high: self.high * factor }
    }
}

impl std::ops::Add for Ranged {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self { low: self.low + rhs.low, mid: self.mid + rhs.mid, high: self.high + rhs.high }
    }
}

impl std::ops::AddAssign for Ranged {
    fn add_assign(&mut self, rhs: Self) {
        self.low += rhs.low;
        self.mid += rhs.mid;
        self.high += rhs.high;
    }
}

/// All computed energy/carbon/cost projections for a token count.
#[derive(Debug, Clone, Default)]
pub struct EnergyEstimate {
    /// Server-side energy in joules (before PUE)
    pub server_joules: Ranged,
    /// Server-side energy including datacenter overhead (PUE)
    pub facility_joules: Ranged,
    /// Server-side energy in kWh (facility, post-PUE)
    pub facility_kwh: Ranged,

    /// grams CO2 equivalent
    pub carbon_grams: Ranged,

    /// Equivalent local GPU runtime in seconds (no confidence range -- local GPU is known hardware)
    pub local_gpu_seconds: f64,
    /// Local GPU energy in kWh
    pub local_kwh: f64,
    /// Local electricity cost in USD
    pub local_cost_usd: f64,

    /// Seconds of solar panel output to offset server energy
    pub solar_offset_seconds: Ranged,

    /// API cost for reference (passed through, not computed here)
    pub api_cost_usd: f64,
    /// Ratio of API cost to local electricity cost
    pub api_markup_ratio: f64,
}

/// Compute energy estimates from token counts.
pub fn estimate(tokens: &TokenCounts, tier: ModelTier, api_cost_usd: f64, config: &EnergyConfig) -> EnergyEstimate {
    let j_out = output_joules_per_token(tier);
    let j_in = input_joules_per_token(tier);

    // Fresh input = total input minus cache reads (cache creates are fresh computation)
    let fresh_input = tokens.input_tokens + tokens.cache_create_tokens;
    let cache_hits = tokens.cache_read_tokens;

    let server_joules_mid =
        tokens.output_tokens as f64 * j_out
        + fresh_input as f64 * j_in
        + cache_hits as f64 * j_out * CACHE_HIT_ENERGY_FACTOR;

    let server_joules = Ranged::from_mid(server_joules_mid);
    let facility_joules = server_joules.scale(config.pue);
    let facility_kwh = facility_joules.scale(1.0 / 3_600_000.0);

    // Carbon
    let carbon_grams = facility_kwh.scale(config.grid_region.intensity());

    // Local GPU equivalence (known hardware, no confidence range)
    let j_local = local_joules_per_token(&config.local_gpu);
    let total_tokens = tokens.output_tokens + tokens.input_tokens
        + tokens.cache_read_tokens + tokens.cache_create_tokens;
    let local_gpu_seconds = (total_tokens as f64 * j_local) / config.local_gpu.system_watts;
    let local_kwh = total_tokens as f64 * j_local / 3_600_000.0;
    let local_cost_usd = local_kwh * config.electricity_rate;

    // Solar offset
    let solar_kwh_per_sec = config.solar.kwh_per_second();
    let solar_offset_seconds = if solar_kwh_per_sec > 0.0 {
        facility_kwh.scale(1.0 / solar_kwh_per_sec)
    } else {
        Ranged { low: f64::INFINITY, mid: f64::INFINITY, high: f64::INFINITY }
    };

    // API markup
    let api_markup_ratio = if local_cost_usd > 0.0 {
        api_cost_usd / local_cost_usd
    } else {
        f64::INFINITY
    };

    EnergyEstimate {
        server_joules,
        facility_joules,
        facility_kwh,
        carbon_grams,
        local_gpu_seconds,
        local_kwh,
        local_cost_usd,
        solar_offset_seconds,
        api_cost_usd,
        api_markup_ratio,
    }
}

// ---------------------------------------------------------------------------
// Session-level integration
// ---------------------------------------------------------------------------

/// Accumulator for computing energy across multiple API calls in a session.
/// Tracks per-call estimates so cumulative totals can be queried at any point.
#[derive(Debug, Clone, Default)]
pub struct SessionEnergy {
    pub cumulative: EnergyEstimate,
    pub call_count: u32,
}

impl SessionEnergy {
    /// Add an API call's energy contribution. Call this once per ApiCall event.
    pub fn add_call(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_create_tokens: u64,
        model: &str,
        api_cost_usd: f64,
        config: &EnergyConfig,
    ) {
        let tokens = TokenCounts {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_create_tokens,
        };
        let tier = ModelTier::from_model_str(model);
        let call_est = estimate(&tokens, tier, api_cost_usd, config);

        self.cumulative.server_joules += call_est.server_joules;
        self.cumulative.facility_joules += call_est.facility_joules;
        self.cumulative.facility_kwh += call_est.facility_kwh;
        self.cumulative.carbon_grams += call_est.carbon_grams;
        self.cumulative.local_gpu_seconds += call_est.local_gpu_seconds;
        self.cumulative.local_kwh += call_est.local_kwh;
        self.cumulative.local_cost_usd += call_est.local_cost_usd;
        self.cumulative.solar_offset_seconds += call_est.solar_offset_seconds;
        self.cumulative.api_cost_usd += call_est.api_cost_usd;
        // Recompute markup from cumulative totals
        self.cumulative.api_markup_ratio = if self.cumulative.local_cost_usd > 0.0 {
            self.cumulative.api_cost_usd / self.cumulative.local_cost_usd
        } else {
            f64::INFINITY
        };
        self.call_count += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn fmt_estimate(e: &EnergyEstimate) -> String {
        format!(
            "\
server_joules:        {:.4} [{:.4} .. {:.4}]
facility_joules:      {:.4} [{:.4} .. {:.4}]
facility_kwh:         {:.8} [{:.8} .. {:.8}]
carbon_grams:         {:.6} [{:.6} .. {:.6}]
local_gpu_seconds:    {:.4}
local_kwh:            {:.8}
local_cost_usd:       {:.8}
solar_offset_seconds: {:.2} [{:.2} .. {:.2}]
api_cost_usd:         {:.4}
api_markup_ratio:     {:.2}",
            e.server_joules.mid, e.server_joules.low, e.server_joules.high,
            e.facility_joules.mid, e.facility_joules.low, e.facility_joules.high,
            e.facility_kwh.mid, e.facility_kwh.low, e.facility_kwh.high,
            e.carbon_grams.mid, e.carbon_grams.low, e.carbon_grams.high,
            e.local_gpu_seconds,
            e.local_kwh,
            e.local_cost_usd,
            e.solar_offset_seconds.mid, e.solar_offset_seconds.low, e.solar_offset_seconds.high,
            e.api_cost_usd,
            e.api_markup_ratio,
        )
    }

    /// Validate coefficient flow: 1M output tokens through Opus tier.
    /// Expected server energy: 1_000_000 * 5.0 J = 5_000_000 J = 5 MJ
    /// At PUE 1.2: 6 MJ = 1.667 kWh
    /// Carbon at 350 gCO2/kWh: 583.3 g
    #[test]
    fn opus_1m_output_tokens() {
        let tokens = TokenCounts {
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let config = EnergyConfig::default();
        let e = estimate(&tokens, ModelTier::Opus, 75.0, &config);

        assert!((e.server_joules.mid - 5_000_000.0).abs() < 0.01, "5 MJ for 1M Opus output tokens");
        assert!((e.facility_joules.mid - 6_000_000.0).abs() < 0.01, "6 MJ with PUE 1.2");
        assert!((e.facility_kwh.mid - 1.6667).abs() < 0.001, "~1.667 kWh");
        assert!((e.carbon_grams.mid - 583.33).abs() < 1.0, "~583 gCO2 at 350 gCO2/kWh");

        // Confidence band: low = mid/3, high = mid*3
        assert!((e.server_joules.low - 5_000_000.0 / 3.0).abs() < 1.0);
        assert!((e.server_joules.high - 5_000_000.0 * 3.0).abs() < 1.0);

        assert_snapshot!("opus_1m_output", fmt_estimate(&e));
    }

    /// Validate tier scaling: Opus output should be ~5.56x Sonnet, ~33.3x Haiku.
    #[test]
    fn tier_scaling_ratios() {
        let tokens = TokenCounts {
            output_tokens: 100_000,
            ..Default::default()
        };
        let config = EnergyConfig::default();
        let opus = estimate(&tokens, ModelTier::Opus, 0.0, &config);
        let sonnet = estimate(&tokens, ModelTier::Sonnet, 0.0, &config);
        let haiku = estimate(&tokens, ModelTier::Haiku, 0.0, &config);

        let opus_sonnet = opus.server_joules.mid / sonnet.server_joules.mid;
        let opus_haiku = opus.server_joules.mid / haiku.server_joules.mid;

        assert!((opus_sonnet - 5.556).abs() < 0.01, "Opus/Sonnet ratio: {opus_sonnet}");
        assert!((opus_haiku - 33.333).abs() < 0.01, "Opus/Haiku ratio: {opus_haiku}");

        assert_snapshot!("tier_scaling", format!(
            "opus_sonnet_ratio: {opus_sonnet:.3}\nopus_haiku_ratio: {opus_haiku:.3}"
        ));
    }

    /// Validate input vs output energy asymmetry.
    /// 100k input tokens should cost 5% of 100k output tokens (same tier).
    #[test]
    fn input_output_asymmetry() {
        let config = EnergyConfig::default();

        let output_only = estimate(
            &TokenCounts { output_tokens: 100_000, ..Default::default() },
            ModelTier::Sonnet, 0.0, &config,
        );
        let input_only = estimate(
            &TokenCounts { input_tokens: 100_000, ..Default::default() },
            ModelTier::Sonnet, 0.0, &config,
        );

        let ratio = input_only.server_joules.mid / output_only.server_joules.mid;
        assert!((ratio - 0.05).abs() < 0.001, "Input/output ratio: {ratio}");
    }

    /// Cache reads should contribute zero energy.
    #[test]
    fn cache_reads_are_free() {
        let config = EnergyConfig::default();

        let with_cache = estimate(
            &TokenCounts { cache_read_tokens: 500_000, ..Default::default() },
            ModelTier::Sonnet, 0.0, &config,
        );

        assert_eq!(with_cache.server_joules.mid, 0.0, "Cache-only tokens produce zero server energy");
    }

    /// Cache creates count as fresh input computation.
    #[test]
    fn cache_creates_cost_as_input() {
        let config = EnergyConfig::default();

        let fresh = estimate(
            &TokenCounts { input_tokens: 100_000, ..Default::default() },
            ModelTier::Sonnet, 0.0, &config,
        );
        let cached = estimate(
            &TokenCounts { cache_create_tokens: 100_000, ..Default::default() },
            ModelTier::Sonnet, 0.0, &config,
        );

        assert!((fresh.server_joules.mid - cached.server_joules.mid).abs() < 0.001,
            "Cache create energy should equal fresh input energy");
    }

    /// Realistic session: mixed tokens, Opus, typical API cost.
    /// ~5000 output, ~20000 input, ~15000 cache read, ~5000 cache create.
    #[test]
    fn realistic_opus_session() {
        let tokens = TokenCounts {
            input_tokens: 20_000,
            output_tokens: 5_000,
            cache_read_tokens: 15_000,
            cache_create_tokens: 5_000,
        };
        let config = EnergyConfig::default();
        // Rough API cost: (20k+5k_create)*$15/M_in + 15k*$1.50/M_cache + 5k*$75/M_out
        let api_cost = (25_000.0 * 15.0 + 15_000.0 * 1.50 + 5_000.0 * 75.0) / 1_000_000.0;
        let e = estimate(&tokens, ModelTier::Opus, api_cost, &config);

        assert_snapshot!("realistic_opus_session", fmt_estimate(&e));
    }

    /// Validate local GPU equivalence numbers.
    /// 4090: 300W system, 14 tok/s => 21.43 J/tok, 5.95 kWh/M tokens.
    #[test]
    fn local_gpu_reference() {
        let j = local_joules_per_token(&GPU_4090);
        assert!((j - 21.4286).abs() < 0.001, "4090 J/tok: {j}");

        let kwh = local_kwh_per_million_tokens(&GPU_4090);
        assert!((kwh - 5.952).abs() < 0.01, "4090 kWh/M: {kwh}");
    }

    /// Validate solar offset calculation.
    /// 1 kWh at US avg solar (68W continuous) = 1/0.0000189 ~= 52,941 seconds.
    #[test]
    fn solar_offset_1kwh() {
        let s = &SOLAR_US_AVG;
        assert!((s.avg_watts() - 68.0).abs() < 0.01);

        let secs_per_kwh = 1.0 / s.kwh_per_second();
        assert!((secs_per_kwh - 52_941.0).abs() < 100.0,
            "Seconds of solar per kWh: {secs_per_kwh}");
    }

    /// Grid region carbon intensities should span the expected range.
    #[test]
    fn grid_region_range() {
        let regions = [
            (GridRegion::EuWest3, 85.0, "France (nuclear)"),
            (GridRegion::UsWest2, 200.0, "Oregon"),
            (GridRegion::EuWest1, 298.0, "Ireland"),
            (GridRegion::UsEast1, 350.0, "Virginia"),
            (GridRegion::UsAverage, 367.0, "US avg"),
            (GridRegion::WorldAverage, 442.0, "World avg"),
            (GridRegion::UsEast2, 449.0, "Ohio"),
        ];
        let mut lines = Vec::new();
        for (region, expected, label) in &regions {
            let actual = region.intensity();
            assert_eq!(actual, *expected, "{label}");
            lines.push(format!("{label}: {actual} gCO2eq/kWh"));
        }
        assert_snapshot!("grid_regions", lines.join("\n"));
    }

    /// API markup: Opus at $75/M output tokens vs 4090 electricity.
    /// 4090: 5.95 kWh/M * $0.17/kWh = $1.01/M tokens.
    /// Markup: $75 / $1.01 ~= 74x (output-only comparison).
    #[test]
    fn api_markup_opus_output() {
        let tokens = TokenCounts {
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let config = EnergyConfig::default();
        let e = estimate(&tokens, ModelTier::Opus, 75.0, &config);

        assert!(e.api_markup_ratio > 50.0, "Opus output markup should be >50x, got {:.1}", e.api_markup_ratio);
        assert!(e.api_markup_ratio < 100.0, "Opus output markup should be <100x, got {:.1}", e.api_markup_ratio);

        assert_snapshot!("api_markup_opus", format!(
            "api_cost: ${:.2}\nlocal_electricity: ${:.4}\nmarkup: {:.1}x",
            e.api_cost_usd, e.local_cost_usd, e.api_markup_ratio
        ));
    }

    /// Custom grid region and electricity rate.
    #[test]
    fn custom_config() {
        let tokens = TokenCounts {
            output_tokens: 10_000,
            ..Default::default()
        };
        let config = EnergyConfig {
            electricity_rate: RATE_HAWAII,
            grid_region: GridRegion::Custom(600),
            ..Default::default()
        };
        let e = estimate(&tokens, ModelTier::Sonnet, 0.15, &config);

        assert_eq!(config.grid_region.intensity(), 600.0);
        assert_eq!(config.electricity_rate, 0.42);
        assert_snapshot!("custom_config", fmt_estimate(&e));
    }

    /// SessionEnergy accumulates across multiple API calls correctly.
    #[test]
    fn session_accumulator() {
        let config = EnergyConfig::default();
        let mut session = SessionEnergy::default();

        // Two Opus calls
        session.add_call(20_000, 5_000, 15_000, 5_000, "claude-opus-4-6", 0.50, &config);
        session.add_call(10_000, 3_000, 8_000, 2_000, "claude-opus-4-6", 0.30, &config);

        assert_eq!(session.call_count, 2);
        assert!((session.cumulative.api_cost_usd - 0.80).abs() < 0.001);

        // Energy should be sum of both calls
        let call1 = estimate(
            &TokenCounts { input_tokens: 20_000, output_tokens: 5_000, cache_read_tokens: 15_000, cache_create_tokens: 5_000 },
            ModelTier::Opus, 0.50, &config,
        );
        let call2 = estimate(
            &TokenCounts { input_tokens: 10_000, output_tokens: 3_000, cache_read_tokens: 8_000, cache_create_tokens: 2_000 },
            ModelTier::Opus, 0.30, &config,
        );

        let expected_joules = call1.server_joules.mid + call2.server_joules.mid;
        assert!((session.cumulative.server_joules.mid - expected_joules).abs() < 0.01,
            "Accumulated joules: {} vs expected {}", session.cumulative.server_joules.mid, expected_joules);

        assert_snapshot!("session_accumulator", fmt_estimate(&session.cumulative));
    }

    /// Mixed-model session: Opus + Sonnet subagent calls.
    #[test]
    fn mixed_model_session() {
        let config = EnergyConfig::default();
        let mut session = SessionEnergy::default();

        session.add_call(20_000, 5_000, 15_000, 5_000, "claude-opus-4-6", 0.50, &config);
        session.add_call(10_000, 3_000, 8_000, 0, "claude-sonnet-4-6", 0.05, &config);
        session.add_call(5_000, 1_000, 3_000, 0, "claude-haiku-4-5", 0.01, &config);

        assert_eq!(session.call_count, 3);

        // Opus call should dominate energy (5 J/output vs 0.9 vs 0.15)
        let opus_only = estimate(
            &TokenCounts { input_tokens: 20_000, output_tokens: 5_000, cache_read_tokens: 15_000, cache_create_tokens: 5_000 },
            ModelTier::Opus, 0.50, &config,
        );
        let opus_fraction = opus_only.server_joules.mid / session.cumulative.server_joules.mid;
        assert!(opus_fraction > 0.85, "Opus should dominate: {:.1}%", opus_fraction * 100.0);

        assert_snapshot!("mixed_model_session", fmt_estimate(&session.cumulative));
    }

    /// Confidence band properties: low < mid < high, ratios are 1/3 and 3x.
    #[test]
    fn confidence_band_properties() {
        let tokens = TokenCounts { output_tokens: 10_000, ..Default::default() };
        let config = EnergyConfig::default();
        let e = estimate(&tokens, ModelTier::Sonnet, 1.0, &config);

        // All ranged values: low < mid < high
        for (name, r) in [
            ("server_joules", e.server_joules),
            ("facility_kwh", e.facility_kwh),
            ("carbon_grams", e.carbon_grams),
            ("solar_offset_seconds", e.solar_offset_seconds),
        ] {
            assert!(r.low < r.mid, "{name}: low ({}) < mid ({})", r.low, r.mid);
            assert!(r.mid < r.high, "{name}: mid ({}) < high ({})", r.mid, r.high);
            assert!((r.low / r.mid - CONFIDENCE_LOW_FACTOR).abs() < 0.001, "{name}: low/mid ratio");
            assert!((r.high / r.mid - CONFIDENCE_HIGH_FACTOR).abs() < 0.001, "{name}: high/mid ratio");
        }
    }
}
