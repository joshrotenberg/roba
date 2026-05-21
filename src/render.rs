//! Presentation / "eye candy" subsystem.
//!
//! Centralizes the decisions about when to render markdown, when to
//! colorize, and how to format the various surfaces (body, footer,
//! refusal warnings, errors, tool-call lines). Everything keys off a
//! single [`Style`] computed once at startup from CLI args + env +
//! TTY detection.

use std::io::IsTerminal;

use crate::cli::AskArgs;

/// Resolved presentation policy for one cwr invocation. Built once
/// and threaded through wherever output happens.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    /// Render markdown bodies through termimad.
    pub render_markdown: bool,
    /// Use ANSI color anywhere we emit decoration.
    pub color: bool,
}

impl Style {
    /// Resolve from CLI args + environment + TTY detection.
    pub fn detect(args: &AskArgs) -> Self {
        let stdout_tty = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let plain = args.plain;

        // Markdown rendering is for human consumption. Skip when the
        // user explicitly opted out, when stdout isn't a TTY, when
        // they asked for a structured / extracted form, or in stream
        // mode (we'd have to buffer to render markdown, defeating the
        // purpose of streaming).
        let render_markdown = !plain
            && stdout_tty
            && !args.json
            && !args.stream
            && args.code.is_none();

        // Color governs the footer, refusal warning, error prefixes,
        // and tool call lines. Off when piping, when --plain, or when
        // NO_COLOR is set.
        let color = !plain && stdout_tty && !no_color;

        Self {
            render_markdown,
            color,
        }
    }

    /// Build a style with all visual features off. Useful when the
    /// caller has already decided rendering is inappropriate (e.g.
    /// inside `--save` writing).
    pub fn plain() -> Self {
        Self {
            render_markdown: false,
            color: false,
        }
    }
}

/// Print the answer body to stdout, optionally with markdown
/// rendering applied. The cargo-style 3-space body indent is
/// imposed by this function; plain mode prints with no indent so
/// pipe consumers get raw text.
pub fn print_body(text: &str, style: &Style) {
    if !style.render_markdown {
        println!("{text}");
        return;
    }
    let skin = build_skin(style.color);
    let rendered = skin.text(text, None);
    let rendered_string = format!("{rendered}");
    let trimmed = rendered_string.trim_end_matches('\n');
    for line in trimmed.split('\n') {
        if line.is_empty() {
            println!();
        } else {
            println!("   {line}");
        }
    }
}

fn build_skin(color: bool) -> termimad::MadSkin {
    if color {
        termimad::MadSkin::default()
    } else {
        termimad::MadSkin::no_style()
    }
}

/// Print a metadata line on stderr (cost footer, tool rollup, etc.).
/// Dim gray when color is on; otherwise just the raw text.
pub fn print_meta(line: &str, style: &Style) {
    if style.color {
        eprintln!("\x1b[2m{line}\x1b[0m");
    } else {
        eprintln!("{line}");
    }
}

/// Print a blank line on stderr -- used as a separator before the
/// footer block.
pub fn print_meta_blank() {
    eprintln!();
}
