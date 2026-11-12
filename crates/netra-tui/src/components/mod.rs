//! Rendering helpers. Each submodule draws one region of the screen.

pub mod confirm;
pub mod filter_panel;
pub mod header;
pub mod help;
pub mod inspector;
pub mod list;
pub mod status;
pub mod too_small;

use ratatui::layout::Rect;

/// Fits `text` into exactly `width` characters, truncating with no panic.
pub(crate) fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(width);
    let mut used = 0;
    for character in text.chars() {
        if used >= width {
            break;
        }
        out.push(character);
        used += 1;
    }
    while used < width {
        out.push(' ');
        used += 1;
    }
    out
}

/// Truncates `text` to at most `width` characters.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

/// Returns a centered rectangle no larger than `width` x `height`.
pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncates_and_pads_by_characters() {
        assert_eq!(fit("abcdef", 3), "abc");
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(fit("áéíóú", 2), "áé");
    }

    #[test]
    fn centered_never_exceeds_the_area() {
        let area = Rect::new(0, 0, 20, 10);
        let rect = centered(area, 60, 40);
        assert_eq!(rect, area);
        let rect = centered(area, 10, 4);
        assert_eq!(rect.width, 10);
        assert_eq!(rect.height, 4);
    }
}
