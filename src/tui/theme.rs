//! Tracker colors.
//!
//! Milestone 5 can swap this for an Omarchy palette. Layout and input do not
//! read colors except through [`Theme`].

use ratatui::style::{Color, Modifier, Style};

/// Palette for the read-only tracker view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Window background.
    pub background: Color,
    /// Primary text.
    pub text: Color,
    /// Empty cells and secondary labels.
    pub dim: Color,
    /// Titles and the focused border.
    pub title: Color,
    /// Note names.
    pub note: Color,
    /// Effect digits.
    pub effect: Color,
    /// Row numbers and column headers.
    pub accent: Color,
    /// Foreground on the cursor.
    pub cursor_fg: Color,
    /// Background on the cursor cell or selected sample.
    pub cursor_bg: Color,
    /// Background on the cursor row.
    pub row_bg: Color,
    /// Background on the selected order entry.
    pub order_bg: Color,
    /// Unfocused pane border.
    pub border: Color,
    /// Focused pane border.
    pub border_focus: Color,
}

impl Theme {
    /// Deep blue, in the neighborhood of the ProTracker screen.
    pub const fn protracker() -> Self {
        Self {
            background: Color::Rgb(0, 0, 90),
            text: Color::White,
            dim: Color::Rgb(140, 150, 180),
            title: Color::Yellow,
            note: Color::Rgb(170, 255, 170),
            effect: Color::Rgb(255, 214, 102),
            accent: Color::Cyan,
            cursor_fg: Color::Black,
            cursor_bg: Color::Yellow,
            row_bg: Color::Rgb(24, 36, 150),
            order_bg: Color::Rgb(210, 170, 40),
            border: Color::Rgb(80, 100, 170),
            border_focus: Color::Yellow,
        }
    }

    /// Default text on the window background.
    pub fn text(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }

    /// Secondary text on the window background.
    pub fn dim(self) -> Style {
        Style::default().fg(self.dim).bg(self.background)
    }

    /// Bold title text on the window background.
    pub fn title(self) -> Style {
        Style::default()
            .fg(self.title)
            .bg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// Fill style for pane interiors.
    pub fn fill(self) -> Style {
        self.text()
    }
}

pub(crate) fn paint(fg: Color, bg: Color, bold: bool) -> Style {
    let style = Style::default().fg(fg).bg(bg);
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}
