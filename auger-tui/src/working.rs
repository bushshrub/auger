//! The "Working" row shown while a turn runs, after Codex v0.157
//! (`status_indicator_widget.rs`, `shimmer.rs`; Apache-2.0): a pulsing
//! bullet, a light band sweeping across the label every two seconds, and the
//! elapsed time with the interrupt hint.

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

const LABEL: &str = "Working";
const SWEEP: Duration = Duration::from_secs(2);
/// Dim and bright ends of the sweep, used when the terminal has true color.
const BASE: (u8, u8, u8) = (128, 128, 128);
const HIGHLIGHT: (u8, u8, u8) = (255, 255, 255);

/// The row for a turn that started at `started`.
pub fn line(started: Instant) -> Line<'static> {
    let now = Instant::now();
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut spans = vec![bullet(started.elapsed()), Span::raw(" ")];
    spans.extend(shimmer(LABEL, now));
    spans.push(Span::styled(
        format!(" ({} \u{2022} ", fmt_elapsed(started.elapsed().as_secs())),
        dim,
    ));
    spans.push(Span::styled("esc", Style::default().add_modifier(Modifier::BOLD)));
    spans.push(Span::styled(" to interrupt)", dim));
    Line::from(spans)
}

fn true_color() -> bool {
    static TRUE_COLOR: OnceLock<bool> = OnceLock::new();
    *TRUE_COLOR.get_or_init(|| {
        std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
    })
}

/// Seconds into the shared animation clock, so every animated element moves
/// in step.
fn clock(now: Instant) -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    now.duration_since(*START.get_or_init(Instant::now)).as_secs_f32()
}

/// The pulsing bullet: the first cell of the shimmer with true color, a
/// 600ms blink otherwise.
fn bullet(elapsed: Duration) -> Span<'static> {
    if true_color() {
        return shimmer("\u{2022}", Instant::now()).remove(0);
    }
    match (elapsed.as_millis() / 600) % 2 == 0 {
        true => Span::raw("\u{2022}"),
        false => Span::styled("\u{25e6}", Style::default().add_modifier(Modifier::DIM)),
    }
}

/// `text` with a cosine-shaped bright band sweeping left to right.
fn shimmer(text: &str, now: Instant) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let pos = (clock(now) % SWEEP.as_secs_f32()) / SWEEP.as_secs_f32() * period as f32;
    let band_half_width = 5.0;

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let dist = ((i + padding) as f32 - pos.floor()).abs();
            let t = if dist <= band_half_width {
                0.5 * (1.0 + (std::f32::consts::PI * dist / band_half_width).cos())
            } else {
                0.0
            };
            let style = if true_color() {
                let (r, g, b) = blend(HIGHLIGHT, BASE, t * 0.9);
                Style::default().fg(Color::Rgb(r, g, b)).add_modifier(Modifier::BOLD)
            } else if t < 0.2 {
                Style::default().add_modifier(Modifier::DIM)
            } else if t < 0.6 {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

/// Mix `alpha` of `fg` into `bg`.
fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let mix = |f: u8, b: u8| (f as f32 * alpha + b as f32 * (1.0 - alpha)) as u8;
    (mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// 0s, 59s, 1m 01s, 1h 01m 01s.
fn fmt_elapsed(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m {:02}s", secs / 3600, (secs % 3600) / 60, secs % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_matches_codex() {
        assert_eq!(fmt_elapsed(0), "0s");
        assert_eq!(fmt_elapsed(61), "1m 01s");
        assert_eq!(fmt_elapsed(3600 + 61), "1h 01m 01s");
    }

    #[test]
    fn the_row_reads_working_with_the_interrupt_hint() {
        let text: String = line(Instant::now())
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(text.ends_with(" Working (0s \u{2022} esc to interrupt)"), "{text}");
    }
}
