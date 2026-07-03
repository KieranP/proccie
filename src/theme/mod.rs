//! The terminal's background polarity, so colors adapt to it; detection and
//! color choices live in submodules.

mod detect;
mod palette;
mod parse;

pub use parse::parse_color;

/// Whether the terminal has a dark or light background.
#[derive(Clone, Copy, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}
