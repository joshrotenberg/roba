//! `roba doctor` -- a health check for the claude boundary.
//!
//! roba shells out to the `claude` binary. The common first-run
//! failures all live at that boundary: claude not on PATH, not
//! authenticated, an unparseable `roba.toml`, or a stale bundled rates
//! table. `doctor` runs one check per failure mode and prints a
//! `[ok]` / `[warn]` / `[fail]` line for each.
//!
//! Exit code: 0 if no check FAILs, 1 if any check FAILs. Warnings do
//! not fail. The command never calls claude with a prompt -- the only
//! claude invocation is `claude --version`.

use anyhow::Result;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::profile;
use crate::rates::Rates;

/// Warn when the bundled rate table's `as_of` date is older than this.
const RATES_STALE_DAYS: i64 = 90;

/// The pass/warn/fail outcome of one check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn marker(self) -> &'static str {
        match self {
            Status::Ok => "[ok]",
            Status::Warn => "[warn]",
            Status::Fail => "[fail]",
        }
    }
}

/// Print one check line to stdout: `[ok] name: detail`.
fn report(status: Status, name: &str, detail: &str) {
    println!("{} {name}: {detail}", status.marker());
}

/// Run all checks, printing one line each, and return the process exit
/// code (0 = no failures, 1 = at least one FAIL).
pub fn run() -> Result<i32> {
    let mut any_fail = false;

    any_fail |= check_claude() == Status::Fail;
    let _ = check_auth();
    any_fail |= check_config() == Status::Fail;
    any_fail |= check_rates() == Status::Fail;

    Ok(if any_fail { 1 } else { 0 })
}

/// Is `claude` on PATH and runnable? PASS with the reported version,
/// FAIL if the binary can't be found or run.
fn check_claude() -> Status {
    match Command::new("claude").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let detail = if version.is_empty() {
                "found on PATH".to_string()
            } else {
                version
            };
            report(Status::Ok, "claude", &detail);
            Status::Ok
        }
        Ok(_) | Err(_) => {
            report(
                Status::Fail,
                "claude",
                "not found on PATH -- install claude-code (https://github.com/anthropics/claude-code)",
            );
            Status::Fail
        }
    }
}

/// Is `ANTHROPIC_API_KEY` set? PASS if so. Otherwise WARN -- but the
/// wording is deliberately *informational*, not alarming: a missing key
/// is the normal, expected state for OAuth/subscription auth (roba's
/// default), which can't be verified here without a real call. Only
/// `--bare` strictly needs the key. A nested operator reading this line
/// must be able to tell "fine, OAuth" from "actually broken".
fn check_auth() -> Status {
    let key = std::env::var("ANTHROPIC_API_KEY").ok();
    let (status, detail) = auth_status(key.as_deref());
    report(status, "auth", &detail);
    status
}

/// Decide the `auth` check's status and wording from the API-key value.
/// Pure (no env read) so the wording/status can be unit-tested without
/// touching process-global state. `key` is `Some` only when the env var
/// is set to a non-empty value.
fn auth_status(key: Option<&str>) -> (Status, String) {
    match key {
        Some(v) if !v.is_empty() => (Status::Ok, "ANTHROPIC_API_KEY set".to_string()),
        _ => (
            Status::Warn,
            "ANTHROPIC_API_KEY not set -- normal for OAuth/subscription auth (roba's default), \
             which can't be verified here; only --bare needs the key. If a real run fails with \
             an auth error, run: claude /login"
                .to_string(),
        ),
    }
}

/// Does the roba.toml pool load and parse? PASS listing the source
/// files (or "no roba.toml found"), FAIL with the parse error.
fn check_config() -> Status {
    match profile::load_pool() {
        Ok(pool) if pool.sources.is_empty() => {
            report(Status::Ok, "config", "no roba.toml found");
            Status::Ok
        }
        Ok(pool) => {
            let files = pool
                .sources
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            report(
                Status::Ok,
                "config",
                &format!("{} file(s): {files}", pool.sources.len()),
            );
            Status::Ok
        }
        Err(e) => {
            report(Status::Fail, "config", &format!("{e:#}"));
            Status::Fail
        }
    }
}

/// Report the bundled rate table's `as_of` date. WARN if it's older
/// than [`RATES_STALE_DAYS`], else PASS. FAIL only if the bundled
/// table can't be parsed (a build-time invariant, but checked anyway).
fn check_rates() -> Status {
    let rates = match Rates::bundled() {
        Ok(r) => r,
        Err(e) => {
            report(Status::Fail, "rates", &format!("{e:#}"));
            return Status::Fail;
        }
    };
    let as_of = &rates.meta.as_of;
    match days_since(as_of) {
        Some(days) if days > RATES_STALE_DAYS => {
            report(
                Status::Warn,
                "rates",
                &format!("as_of {as_of} ({days} days old; prices may be stale)"),
            );
            Status::Warn
        }
        Some(days) => {
            report(
                Status::Ok,
                "rates",
                &format!("as_of {as_of} ({days} days old)"),
            );
            Status::Ok
        }
        None => {
            report(
                Status::Warn,
                "rates",
                &format!("as_of {as_of} (could not parse date)"),
            );
            Status::Warn
        }
    }
}

/// Days elapsed between an ISO `YYYY-MM-DD` date and today (UTC).
/// `None` if the string doesn't parse or the system clock is before
/// the epoch. Negative when the date is in the future.
fn days_since(as_of: &str) -> Option<i64> {
    let (y, m, d) = parse_iso_date(as_of)?;
    let now = today_days()?;
    Some(now - days_from_civil(y, m, d))
}

/// Whole days from the Unix epoch to now (UTC), via the system clock.
fn today_days() -> Option<i64> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((secs / 86_400) as i64)
}

/// Parse an ISO `YYYY-MM-DD` date into `(year, month, day)`. Rejects
/// anything with the wrong number of dash-separated parts.
fn parse_iso_date(s: &str) -> Option<(i64, i64, i64)> {
    let mut parts = s.split('-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((y, m, d))
}

/// Days from 1970-01-01 to the given civil date, via Howard Hinnant's
/// `days_from_civil` algorithm. Valid for the proleptic Gregorian
/// calendar; we only feed it real dates from the rate table.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_epoch_is_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn days_from_civil_known_offsets() {
        // 2000-01-01 is 10957 days after the epoch.
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
        // One day later.
        assert_eq!(days_from_civil(2000, 1, 2), 10_958);
        // A full common year later.
        assert_eq!(days_from_civil(2001, 1, 1), 11_323);
    }

    #[test]
    fn days_between_two_dates() {
        let a = days_from_civil(2026, 6, 2);
        let b = days_from_civil(2026, 9, 1);
        assert_eq!(b - a, 91);
    }

    #[test]
    fn parse_iso_date_valid() {
        assert_eq!(parse_iso_date("2026-06-02"), Some((2026, 6, 2)));
    }

    #[test]
    fn parse_iso_date_rejects_malformed() {
        assert_eq!(parse_iso_date("2026/06/02"), None);
        assert_eq!(parse_iso_date("2026-06"), None);
        assert_eq!(parse_iso_date("2026-06-02-1"), None);
        assert_eq!(parse_iso_date("not-a-date-x"), None);
    }

    #[test]
    fn days_since_future_date_is_negative() {
        // A date far in the future relative to any plausible clock.
        let d = days_since("2999-01-01").expect("parses");
        assert!(d < 0, "future date should be negative, got {d}");
    }

    #[test]
    fn auth_status_key_set_is_ok() {
        let (status, detail) = auth_status(Some("sk-ant-xxx"));
        assert_eq!(status, Status::Ok);
        assert_eq!(detail, "ANTHROPIC_API_KEY set");
    }

    #[test]
    fn auth_status_no_key_warns_informationally() {
        // No key (None) and an empty key both take the OAuth-normal path.
        for key in [None, Some("")] {
            let (status, detail) = auth_status(key);
            assert_eq!(status, Status::Warn, "missing key is a warn, not a fail");
            // The wording must read as normal-for-OAuth, not broken, and
            // point at the recovery path for a real auth failure.
            assert!(detail.contains("normal for OAuth"), "detail: {detail}");
            assert!(detail.contains("--bare"), "detail: {detail}");
            assert!(detail.contains("claude /login"), "detail: {detail}");
            // It must NOT read as a hard failure.
            assert!(!detail.contains("[fail]"), "detail: {detail}");
        }
    }

    #[test]
    fn status_markers() {
        assert_eq!(Status::Ok.marker(), "[ok]");
        assert_eq!(Status::Warn.marker(), "[warn]");
        assert_eq!(Status::Fail.marker(), "[fail]");
    }
}
