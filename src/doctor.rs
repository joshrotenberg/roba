//! `roba doctor` -- a health check for the claude boundary.
//!
//! roba shells out to the `claude` binary. The common first-run
//! failures all live at that boundary: claude not on PATH, not
//! authenticated, an unparseable `roba.toml`, or a stale bundled rates
//! table. `doctor` runs one check per failure mode and prints a
//! `[ok]` / `[warn]` / `[fail]` line for each.
//!
//! Exit code: 0 if no check FAILs, 1 if any check FAILs. Warnings do
//! not fail. The same code is returned in both human and `--json`
//! modes. The command never calls claude with a prompt -- the only
//! claude invocation is `claude --version`.
//!
//! `--json` emits the uniform `{ version: 1, result: { checks, overall } }`
//! envelope (via the crate's `VersionedResult` wrapper); the human form
//! prints one `[ok]`/`[warn]`/`[fail]` line per check, unchanged.

use anyhow::Result;
use serde::Serialize;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::DoctorArgs;
use crate::profile;
use crate::rates::Rates;

/// Warn when the bundled rate table's `as_of` date is older than this.
const RATES_STALE_DAYS: i64 = 90;

/// The pass/warn/fail outcome of one check. Serializes as a lowercase
/// string (`"ok"` / `"warn"` / `"fail"`) for the `--json` envelope.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
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

/// The structured result of one check: a stable `name`, a `status`, and
/// a human-readable `message`. Collected (not printed) so both the
/// human and `--json` forms render from the same data.
#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    status: Status,
    message: String,
}

/// The full doctor report: every check plus the worst status across
/// them. The `--json` envelope's `result` payload.
#[derive(Debug, Serialize)]
struct Report {
    checks: Vec<Check>,
    /// The worst status across all checks: `fail` if any failed, else
    /// `warn` if any warned, else `ok`. The exit code is `1` exactly
    /// when this is `fail`.
    overall: Status,
}

/// Run all checks, then render either one line each (human) or the
/// uniform `{ version, result }` JSON envelope. Returns the process
/// exit code (0 = no FAILs, 1 = at least one FAIL); the same code is
/// returned in both modes.
pub fn run(args: DoctorArgs) -> Result<i32> {
    let checks = vec![check_claude(), check_auth(), check_config(), check_rates()];
    let overall = overall_status(&checks);
    let exit = if overall == Status::Fail { 1 } else { 0 };
    let report = Report { checks, overall };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::VersionedResult::new(&report))?
        );
    } else {
        for c in &report.checks {
            println!("{} {}: {}", c.status.marker(), c.name, c.message);
        }
    }
    Ok(exit)
}

/// The worst status across the checks, used as the report's `overall`
/// and to derive the exit code.
fn overall_status(checks: &[Check]) -> Status {
    if checks.iter().any(|c| c.status == Status::Fail) {
        Status::Fail
    } else if checks.iter().any(|c| c.status == Status::Warn) {
        Status::Warn
    } else {
        Status::Ok
    }
}

/// Is `claude` on PATH and runnable? PASS with the reported version,
/// FAIL if the binary can't be found or run.
fn check_claude() -> Check {
    match Command::new("claude").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let message = if version.is_empty() {
                "found on PATH".to_string()
            } else {
                version
            };
            Check {
                name: "claude",
                status: Status::Ok,
                message,
            }
        }
        Ok(_) | Err(_) => Check {
            name: "claude",
            status: Status::Fail,
            message:
                "not found on PATH -- install claude-code (https://github.com/anthropics/claude-code)"
                    .to_string(),
        },
    }
}

/// Is `ANTHROPIC_API_KEY` set? PASS if so. Otherwise WARN -- but the
/// wording is deliberately *informational*, not alarming: a missing key
/// is the normal, expected state for OAuth/subscription auth (roba's
/// default), which can't be verified here without a real call. Only
/// `--bare` strictly needs the key. A nested operator reading this line
/// must be able to tell "fine, OAuth" from "actually broken".
fn check_auth() -> Check {
    let key = std::env::var("ANTHROPIC_API_KEY").ok();
    let (status, message) = auth_status(key.as_deref());
    Check {
        name: "auth",
        status,
        message,
    }
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
fn check_config() -> Check {
    let (status, message) = match profile::load_pool() {
        Ok(pool) if pool.sources.is_empty() => (Status::Ok, "no roba.toml found".to_string()),
        Ok(pool) => {
            let files = pool
                .sources
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            (
                Status::Ok,
                format!("{} file(s): {files}", pool.sources.len()),
            )
        }
        Err(e) => (Status::Fail, format!("{e:#}")),
    };
    Check {
        name: "config",
        status,
        message,
    }
}

/// Report the bundled rate table's `as_of` date. WARN if it's older
/// than [`RATES_STALE_DAYS`], else PASS. FAIL only if the bundled
/// table can't be parsed (a build-time invariant, but checked anyway).
fn check_rates() -> Check {
    let rates = match Rates::bundled() {
        Ok(r) => r,
        Err(e) => {
            return Check {
                name: "rates",
                status: Status::Fail,
                message: format!("{e:#}"),
            };
        }
    };
    let as_of = &rates.meta.as_of;
    let (status, message) = match days_since(as_of) {
        Some(days) if days > RATES_STALE_DAYS => (
            Status::Warn,
            format!("as_of {as_of} ({days} days old; prices may be stale)"),
        ),
        Some(days) => (Status::Ok, format!("as_of {as_of} ({days} days old)")),
        None => (
            Status::Warn,
            format!("as_of {as_of} (could not parse date)"),
        ),
    };
    Check {
        name: "rates",
        status,
        message,
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

    fn check(status: Status) -> Check {
        Check {
            name: "x",
            status,
            message: String::new(),
        }
    }

    #[test]
    fn overall_status_is_worst() {
        // fail dominates everything.
        assert_eq!(
            overall_status(&[check(Status::Ok), check(Status::Warn), check(Status::Fail)]),
            Status::Fail
        );
        // warn beats ok when there's no fail.
        assert_eq!(
            overall_status(&[check(Status::Ok), check(Status::Warn)]),
            Status::Warn
        );
        // all-ok stays ok.
        assert_eq!(
            overall_status(&[check(Status::Ok), check(Status::Ok)]),
            Status::Ok
        );
    }

    #[test]
    fn status_serializes_as_lowercase_string() {
        assert_eq!(serde_json::to_value(Status::Ok).unwrap(), "ok");
        assert_eq!(serde_json::to_value(Status::Warn).unwrap(), "warn");
        assert_eq!(serde_json::to_value(Status::Fail).unwrap(), "fail");
    }
}
