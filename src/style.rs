//! Shared ANSI palette + color gating for roba's read-only report verbs.
//!
//! `doctor`, `config lint`, and `config explain` all colorize a human report
//! and gate it on the same rule: color on only when `--plain` was not passed,
//! stdout is a TTY, and `NO_COLOR` is unset. Before this module each verb
//! carried its own copy of the codes, the `paint` helper, and the gate, which
//! drifted apart as surfaces were added. This is the single home for that
//! palette + gate so the three verbs render the same colors and the same
//! byte-plain output everywhere.
//!
//! `render.rs`'s `Style` is a separate, broader concern (it is bound to
//! `AskArgs` and also owns markdown rendering and the spinner), so it is left
//! as-is and not unified here.
//!
//! The palette (green = good, yellow = advisory, red = broken, cyan = key,
//! dim = secondary detail, blue = cross-reference):

use std::io::IsTerminal;

/// Section header / good status: bold green. Matches clap's own `--help`
/// headers and `doctor`'s `[ok]` marker.
pub const HEADER: &str = "\x1b[1;32m";
/// A config knob / alias name / file path: cyan (clap's literal style).
pub const KEY: &str = "\x1b[36m";
/// Secondary detail (descriptions, sources, hints): dim.
pub const DIM: &str = "\x1b[2m";
/// An advisory / warning marker: bold yellow.
pub const WARN: &str = "\x1b[1;33m";
/// An error / broken marker: bold red.
pub const ERROR: &str = "\x1b[1;31m";
/// A source-number cross-reference marker (`[N]`): bright blue (a
/// reference/link color, not error-red), so it stands out from the dim
/// secondary text it sits among.
pub const NUM: &str = "\x1b[94m";

/// Wrap `s` in an ANSI `code` (with reset) when `on`, else return it
/// untouched. The one place the report verbs decide color, so a non-TTY,
/// `NO_COLOR`, or `--plain` run stays byte-plain.
pub fn paint(s: &str, code: &str, on: bool) -> String {
    if on {
        format!("{code}{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Whether a report verb should colorize: off under `--plain`, `NO_COLOR`, or
/// a non-TTY stdout. The shared gate for `doctor`, `config lint`, and `config
/// explain` (mirrors the `color` predicate in [`crate::render`]).
pub fn color_enabled(plain: bool) -> bool {
    !plain && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// A color-or-not palette object. When `color` is false every method returns
/// the text untouched, so one render function produces both the colored TTY
/// view and the byte-plain `--plain` / piped view (and is testable without a
/// real terminal). Used by `config explain`; `doctor` and `config lint` use
/// the free [`paint`] function with the palette constants directly.
pub struct Painter {
    pub color: bool,
}

impl Painter {
    /// Section header: green-bold, matching clap's own `--help` headers.
    pub fn header(&self, s: &str) -> String {
        paint(s, HEADER, self.color)
    }
    /// A config knob / alias name: cyan (clap's literal style).
    pub fn key(&self, s: &str) -> String {
        paint(s, KEY, self.color)
    }
    /// Secondary detail (descriptions, sources): dim.
    pub fn dim(&self, s: &str) -> String {
        paint(s, DIM, self.color)
    }
    /// An unsafe-setting warning marker: bold yellow.
    pub fn warn(&self, s: &str) -> String {
        paint(s, WARN, self.color)
    }
    /// A source-number cross-reference marker (`[N]`): blue.
    pub fn num(&self, s: &str) -> String {
        paint(s, NUM, self.color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_on_wraps_with_reset() {
        assert_eq!(paint("x", KEY, true), "\x1b[36mx\x1b[0m");
    }

    #[test]
    fn paint_off_is_byte_plain() {
        let s = paint("x", KEY, false);
        assert_eq!(s, "x");
        assert!(!s.contains('\x1b'));
    }

    #[test]
    fn painter_off_leaks_no_ansi() {
        let p = Painter { color: false };
        for s in [
            p.header("h"),
            p.key("k"),
            p.dim("d"),
            p.warn("w"),
            p.num("[1]"),
        ] {
            assert!(!s.contains('\x1b'), "leaked ANSI: {s}");
        }
    }

    #[test]
    fn painter_on_uses_the_standard_codes() {
        let p = Painter { color: true };
        assert_eq!(p.header("h"), "\x1b[1;32mh\x1b[0m");
        assert_eq!(p.key("k"), "\x1b[36mk\x1b[0m");
        assert_eq!(p.dim("d"), "\x1b[2md\x1b[0m");
        assert_eq!(p.warn("w"), "\x1b[1;33mw\x1b[0m");
        assert_eq!(p.num("[1]"), "\x1b[94m[1]\x1b[0m");
    }
}
