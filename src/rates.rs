//! Per-model dollar-rate table for cost reporting.
//!
//! Claude Code persists token counts to the session JSONL but never a
//! per-session dollar figure (the `total_cost_usd` field is written as
//! `null`). To report dollars, roba bundles a rate table
//! (`rates.toml`) and multiplies recorded tokens by the per-model
//! USD-per-million-token prices.
//!
//! The bundled rates carry an `as_of` date and a `source` URL so the
//! report can be honest about staleness. A user can override the table
//! with `--rates-file PATH` (or `ROBA_RATES_FILE`) in the same schema,
//! or suppress dollars entirely with `--no-dollars`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The table compiled into the binary. Editing `rates.toml` and
/// rebuilding is all it takes to add a model or adjust a price -- no
/// code change required.
const BUNDLED_RATES: &str = include_str!("rates.toml");

/// A parsed rate table: provenance metadata plus per-model prices.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Rates {
    pub meta: RatesMeta,
    pub models: HashMap<String, ModelRates>,
}

/// Where the table came from and when, for the staleness disclaimer.
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RatesMeta {
    /// ISO date the rates were last verified.
    pub as_of: String,
    /// Human-facing source the rates were taken from.
    pub source: String,
}

/// USD-per-million-token prices for one model. `cache_write` is the
/// 5-minute cache-write price; `cache_read` is the cache-hit price.
#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
pub struct ModelRates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Rates {
    /// Load the table bundled into the binary.
    pub fn bundled() -> Result<Self> {
        toml::from_str(BUNDLED_RATES).context("parsing bundled rates.toml")
    }

    /// Load a user-supplied rates file in the same schema.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading rates file {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing rates file {}", path.display()))
    }

    /// Resolve the effective table: an explicit CLI path wins, then
    /// `ROBA_RATES_FILE`, then the bundled default. A leading `~/` in
    /// either path source is expanded.
    pub fn resolve(cli_file: Option<&Path>) -> Result<Self> {
        if let Some(p) = cli_file {
            return Self::from_file(&expand_tilde(p));
        }
        if let Ok(env) = std::env::var("ROBA_RATES_FILE")
            && !env.is_empty()
        {
            return Self::from_file(&expand_tilde(Path::new(&env)));
        }
        Self::bundled()
    }

    /// Look up the rate row for `model`. An exact key match wins;
    /// otherwise the longest table key that is a prefix of `model`
    /// wins, so a dated id (`claude-sonnet-4-6-20260101`) resolves to
    /// the undated `claude-sonnet-4-6` entry. Returns `None` when no
    /// key matches.
    pub fn model_rates(&self, model: &str) -> Option<&ModelRates> {
        if let Some(r) = self.models.get(model) {
            return Some(r);
        }
        self.models
            .iter()
            .filter(|(k, _)| model.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, r)| r)
    }

    /// Dollar cost for a token breakdown under `model`'s rates.
    /// Returns `None` when the model isn't in the table (the caller
    /// reports "rates unknown" rather than a misleading $0).
    pub fn cost_usd(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> Option<f64> {
        let r = self.model_rates(model)?;
        let per_mtok = |tokens: u64, rate: f64| (tokens as f64) * rate / 1_000_000.0;
        Some(
            per_mtok(input_tokens, r.input)
                + per_mtok(output_tokens, r.output)
                + per_mtok(cache_read_tokens, r.cache_read)
                + per_mtok(cache_write_tokens, r.cache_write),
        )
    }
}

/// Expand a leading `~/` against `$HOME`. Leaves everything else
/// untouched. Kept local so the rates path resolution doesn't depend
/// on the profile module's private helper.
fn expand_tilde(path: &Path) -> std::path::PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = s.strip_prefix("~/") else {
        return path.to_path_buf();
    };
    match std::env::var_os("HOME") {
        Some(home) => Path::new(&home).join(rest),
        None => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_rates_loads() {
        let rates = Rates::bundled().expect("bundled rates parse");
        assert!(!rates.meta.as_of.is_empty());
        assert!(rates.meta.source.starts_with("http"));
        // A representative model is present.
        assert!(rates.models.contains_key("claude-sonnet-4-6"));
    }

    #[test]
    fn from_file_loads_valid_toml() {
        let dir = std::env::temp_dir().join("roba-rates-valid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rates.toml");
        std::fs::write(
            &path,
            r#"
[meta]
as_of = "2026-01-01"
source = "test"

[models."test-model"]
input = 1.0
output = 2.0
cache_read = 0.5
cache_write = 1.5
"#,
        )
        .unwrap();
        let rates = Rates::from_file(&path).expect("loads");
        assert_eq!(rates.meta.as_of, "2026-01-01");
        let r = rates.models.get("test-model").unwrap();
        assert_eq!(r.input, 1.0);
        assert_eq!(r.output, 2.0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn from_file_errors_on_bad_toml() {
        let dir = std::env::temp_dir().join("roba-rates-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rates.toml");
        std::fs::write(&path, "this is { not valid toml ::::").unwrap();
        assert!(Rates::from_file(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn from_file_errors_on_missing_file() {
        let path = std::env::temp_dir().join("roba-rates-does-not-exist-xyz.toml");
        assert!(Rates::from_file(&path).is_err());
    }

    #[test]
    fn cost_usd_known_model() {
        let rates = Rates::bundled().unwrap();
        // Sonnet 4.6: input $3, output $15, cache_read $0.30, cache_write $3.75 per MTok.
        // 1M input + 1M output = $3 + $15 = $18.
        let c = rates
            .cost_usd("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 0)
            .unwrap();
        assert!((c - 18.0).abs() < 1e-9, "got {c}");
        // Add 1M cache-read ($0.30) + 1M cache-write ($3.75) = $22.05.
        let c = rates
            .cost_usd(
                "claude-sonnet-4-6",
                1_000_000,
                1_000_000,
                1_000_000,
                1_000_000,
            )
            .unwrap();
        assert!((c - 22.05).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn cost_usd_unknown_model_is_none() {
        let rates = Rates::bundled().unwrap();
        assert!(rates.cost_usd("gpt-9", 1000, 1000, 0, 0).is_none());
    }

    #[test]
    fn model_rates_matches_dated_id_by_prefix() {
        let rates = Rates::bundled().unwrap();
        // A dated full id resolves to its undated base entry.
        let r = rates
            .model_rates("claude-sonnet-4-6-20260101")
            .expect("prefix match");
        assert_eq!(r.input, 3.0);
    }

    #[test]
    fn model_rates_prefers_longest_prefix() {
        let rates = Rates::bundled().unwrap();
        // opus-4-1 must not be shadowed by a shorter opus key.
        let r = rates
            .model_rates("claude-opus-4-1-20250101")
            .expect("match");
        assert_eq!(r.input, 15.0);
        let r = rates
            .model_rates("claude-opus-4-5-20251101")
            .expect("match");
        assert_eq!(r.input, 5.0);
    }
}
