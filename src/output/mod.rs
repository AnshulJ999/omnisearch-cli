pub mod json;
pub mod style;


use std::io::IsTerminal;

/// `NO_COLOR` set to anything but the empty string turns colour off
/// (no-color.org), as does `TERM=dumb`.
pub fn no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
        || std::env::var_os("TERM").is_some_and(|v| v == "dumb")
}

/// Colour for stdout: a terminal, and no `NO_COLOR`.
pub fn use_color() -> bool {
    std::io::stdout().is_terminal() && !no_color_env()
}

/// Indented JSON on a terminal; one compact line when piped.
pub fn pretty() -> bool {
    std::io::stdout().is_terminal()
}
