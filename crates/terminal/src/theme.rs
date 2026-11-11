//! The THOTH visual identity as Ratatui styles.
//!
//! `palette` names the colours. `Theme` turns them into styles for the terminal the app runs
//! in: full RGB when truecolor is advertised, the named 16 colours otherwise, and modifiers only
//! under `NO_COLOR`. Render code asks for a semantic style (`theme.muted()`, `theme.status(..)`)
//! or passes a palette shade to `fg` and `on`, and never names a colour itself.

pub mod palette;

use std::env;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Padding};

pub use palette::Shade;

use palette::{
    BACKGROUND, BORDER, BORDER_FOCUS, COMPLETED, ERROR, GOLD, GOLD_MUTED, IDLE, RUNNING, SELECTED_SURFACE, TEAL, TEXT,
    TEXT_MUTED, TEXT_SECONDARY, WAITING,
};

/// How much colour the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// The 16 named colours, which every terminal supports.
    Ansi,
    /// The 256-colour palette, as advertised by `TERM`. Palette colours are approximated.
    Indexed,
    /// No colour at all (`NO_COLOR`): only modifiers such as bold and italic remain.
    Mono,
    /// 24-bit RGB, as advertised by `COLORTERM`.
    Truecolor,
}

impl ColorDepth {
    /// Reads the terminal's capabilities. `THOTH_COLOR` (`truecolor`, `256`, `16` or `none`)
    /// overrides detection; otherwise `NO_COLOR` wins, then `COLORTERM`, then `TERM`.
    /// Emitting more colour than a terminal can show garbles the screen, so unknown means less.
    pub fn from_env(force: Option<&str>, no_color: bool, colorterm: Option<&str>, term: Option<&str>) -> Self {
        match force {
            Some("truecolor" | "24bit") => return Self::Truecolor,
            Some("256") => return Self::Indexed,
            Some("16" | "ansi") => return Self::Ansi,
            Some("none" | "mono") => return Self::Mono,
            _ => {},
        }
        if no_color {
            Self::Mono
        } else if matches!(colorterm, Some("truecolor" | "24bit")) {
            Self::Truecolor
        } else if term.is_some_and(|t| t.contains("256color") || t.ends_with("-direct")) {
            Self::Indexed
        } else {
            Self::Ansi
        }
    }
}

/// The state a Tool card, a sub-agent or a turn can be in. Each maps to one palette shade, so
/// the same state looks the same wherever it appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Completed,
    Error,
    Idle,
    Running,
    Waiting,
}

impl Status {
    pub const fn shade(self) -> Shade {
        match self {
            Self::Running => RUNNING,
            Self::Waiting => WAITING,
            Self::Idle => IDLE,
            Self::Completed => COMPLETED,
            Self::Error => ERROR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Colours and styles.
pub struct Theme {
    depth: ColorDepth,
}

impl Theme {
    /// Teal: the main accent, for names and interactive hints.
    pub fn accent(self) -> Style {
        self.fg(TEAL)
    }

    /// Makes a style from outside the theme, such as a syntax highlighter's, fit the terminal
    /// and the palette: colours are converted for the terminal's depth, and backgrounds and
    /// underline colours are dropped so that text sits on the app background.
    pub fn adapt(self, style: Style) -> Style {
        Style { fg: style.fg.and_then(|c| self.adapt_color(c)), bg: None, underline_color: None, ..style }
    }

    fn adapt_color(self, color: Color) -> Option<Color> {
        match self.depth {
            ColorDepth::Mono => None,
            ColorDepth::Truecolor | ColorDepth::Indexed | ColorDepth::Ansi => Some(self.downsample(color)),
        }
    }

    // Borders

    pub fn border(self) -> Style {
        self.fg(BORDER)
    }

    pub fn border_focused(self) -> Style {
        self.fg(BORDER_FOCUS)
    }

    /// The chat's pane: padded on the sides and given a row of air under the header.
    pub const fn chat_pane() -> Block<'static> {
        Block::new().padding(Padding::new(1, 1, 1, 0))
    }

    /// Inline and fenced code.
    pub fn code(self) -> Style {
        self.fg(GOLD_MUTED)
    }

    /// The colour `shade` takes on this terminal, or `None` when colour is off.
    pub fn color(self, shade: Shade) -> Option<Color> {
        match self.depth {
            ColorDepth::Truecolor => Some(shade.rgb),
            ColorDepth::Indexed => Some(self.downsample(shade.rgb)),
            ColorDepth::Ansi => Some(shade.ansi),
            ColorDepth::Mono => None,
        }
    }

    /// `color` as this terminal can show it: RGB is approximated on 256-colour terminals and
    /// dropped to the nearest of the 16 on others. Named and indexed colours pass through.
    fn downsample(self, color: Color) -> Color {
        match (self.depth, color) {
            (ColorDepth::Indexed, Color::Rgb(r, g, b)) => Color::Indexed(nearest_indexed(r, g, b)),
            (ColorDepth::Ansi, Color::Rgb(r, g, b)) => nearest_ansi(r, g, b),
            _ => color,
        }
    }

    /// A foreground in `shade`. Use it for any palette shade the named helpers do not cover.
    pub fn fg(self, shade: Shade) -> Style {
        Style { fg: self.color(shade), ..Style::new() }
    }

    /// Paints `area` with the app background. Call once per frame, before anything else.
    pub fn fill(self, frame: &mut Frame<'_>, area: Rect) {
        self.fill_on(frame, area, BACKGROUND);
    }

    /// Paints `area` with `surface`, for a bar that sits slightly above the background.
    pub fn fill_on(self, frame: &mut Frame<'_>, area: Rect, surface: Shade) {
        frame.buffer_mut().set_style(area, self.on(surface));
    }

    /// Antique gold: rare emphasis, such as a notice that needs attention.
    pub fn gold(self) -> Style {
        self.fg(GOLD)
    }

    /// Markdown headings: the first level is bold teal, the second teal, the rest italic text.
    pub fn heading(self, level: u8) -> Style {
        match level {
            1 => self.accent().add_modifier(Modifier::BOLD),
            2 => self.accent(),
            _ => self.secondary().add_modifier(Modifier::ITALIC),
        }
    }

    /// Secondary facts about a log entry, such as durations and sources.
    pub fn log_meta(self) -> Style {
        self.muted()
    }

    pub fn muted(self) -> Style {
        self.fg(TEXT_MUTED)
    }

    pub const fn new(depth: ColorDepth) -> Self {
        Self { depth }
    }

    /// A background in `shade`.
    pub fn on(self, shade: Shade) -> Style {
        Style { bg: self.color(shade), ..Style::new() }
    }

    // Layout blocks. The design prefers whitespace to boxes: content sits in a padded pane, and
    // a single rule marks the top of the areas that need a boundary.

    /// Borderless, padded area for content such as the chat.
    pub const fn pane() -> Block<'static> {
        Block::new().padding(Padding::horizontal(1))
    }

    /// A pane under a one-row rule, on the app background. The rule is teal when `focused`.
    pub fn rule(self, focused: bool) -> Block<'static> {
        self.rule_on(focused, BACKGROUND)
    }

    /// A pane under a one-row rule, on `surface`, for floating areas such as pickers.
    pub fn rule_on(self, focused: bool, surface: Shade) -> Block<'static> {
        let (border, title) =
            if focused { (self.border_focused(), self.secondary()) } else { (self.border(), self.muted()) };
        Self::pane()
            .borders(Borders::TOP)
            .border_type(BorderType::Plain)
            .border_style(border)
            .title_style(title)
            .style(self.on(surface))
    }

    pub fn secondary(self) -> Style {
        self.fg(TEXT_SECONDARY)
    }

    /// Section headings. Teal, bold, and the only heading style.
    pub fn section_title(self) -> Style {
        self.accent().add_modifier(Modifier::BOLD)
    }

    /// The highlight for the selected row of a `List` or `Table`: teal text on a raised
    /// surface. Under `NO_COLOR` it falls back to reverse video.
    pub fn selected(self) -> Style {
        match self.depth {
            ColorDepth::Mono => Style::new().add_modifier(Modifier::REVERSED),
            ColorDepth::Truecolor | ColorDepth::Indexed | ColorDepth::Ansi => {
                self.fg(TEAL).patch(self.on(SELECTED_SURFACE))
            },
        }
    }

    // States

    pub fn status(self, status: Status) -> Style {
        self.fg(status.shade())
    }

    // Text

    pub fn text(self) -> Style {
        self.fg(TEXT)
    }

    /// Clock times in the log. The log does not show times yet.
    #[expect(dead_code, reason = "the log does not show times yet")]
    pub fn timestamp(self) -> Style {
        self.muted()
    }
}

/// The nearest of the 16 named colours to an RGB value, by distance to their usual rendering.
fn nearest_ansi(r: u8, g: u8, b: u8) -> Color {
    const NAMED: [(Color, [i32; 3]); 16] = [
        (Color::Black, [0, 0, 0]),
        (Color::Red, [205, 49, 49]),
        (Color::Green, [13, 188, 121]),
        (Color::Yellow, [229, 229, 16]),
        (Color::Blue, [36, 114, 200]),
        (Color::Magenta, [188, 63, 188]),
        (Color::Cyan, [17, 168, 205]),
        (Color::Gray, [229, 229, 229]),
        (Color::DarkGray, [102, 102, 102]),
        (Color::LightRed, [241, 76, 76]),
        (Color::LightGreen, [35, 209, 139]),
        (Color::LightYellow, [245, 245, 67]),
        (Color::LightBlue, [59, 142, 234]),
        (Color::LightMagenta, [214, 112, 214]),
        (Color::LightCyan, [41, 184, 219]),
        (Color::White, [255, 255, 255]),
    ];
    let target = [i32::from(r), i32::from(g), i32::from(b)];
    NAMED
        .iter()
        .min_by_key(|&&(_, rgb)| {
            rgb.iter().zip(&target).map(|(named, wanted)| named.saturating_sub(*wanted).saturating_pow(2)).sum::<i32>()
        })
        .map_or(Color::Reset, |&(color, _)| color)
}

/// The nearest xterm-256 colour to an RGB value: the closer of the 6x6x6 cube and the
/// 24-step grey ramp.
fn nearest_indexed(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let step = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|&(_, &level)| i32::from(level).saturating_sub(i32::from(v)).abs())
            .map_or(0, |(index, _)| index)
    };
    let level = |index: usize| LEVELS.get(index).copied().unwrap_or(0);
    let (ri, gi, bi) = (step(r), step(g), step(b));
    let distance = |c: (u8, u8, u8)| {
        let d = |x: u8, y: u8| i32::from(x).saturating_sub(i32::from(y)).saturating_pow(2);
        d(c.0, r).saturating_add(d(c.1, g)).saturating_add(d(c.2, b))
    };
    let cube = (level(ri), level(gi), level(bi));
    let average = u32::from(r).saturating_add(u32::from(g)).saturating_add(u32::from(b)).div_euclid(3);
    let raw_step = average.saturating_sub(8).saturating_add(5).div_euclid(10);
    let grey_step = u8::try_from(raw_step.min(23)).unwrap_or(23);
    let grey_level = 8u8.saturating_add(10u8.saturating_mul(grey_step));
    let grey = (grey_level, grey_level, grey_level);
    let cube_index = u8::try_from(
        16usize.saturating_add(36usize.saturating_mul(ri)).saturating_add(6usize.saturating_mul(gi)).saturating_add(bi),
    )
    .unwrap_or(u8::MAX);
    if distance(grey) < distance(cube) { 232u8.saturating_add(grey_step) } else { cube_index }
}

/// The theme for this process, read once from the environment.
pub fn current() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let no_color = env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let var = |name| env::var(name).ok();
        Theme::new(ColorDepth::from_env(
            var("THOTH_COLOR").as_deref(),
            no_color,
            var("COLORTERM").as_deref(),
            var("TERM").as_deref(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::palette::ELEVATED;
    use super::*;

    fn truecolor() -> Theme {
        Theme::new(ColorDepth::Truecolor)
    }

    #[test]
    fn colour_depth_follows_the_environment() {
        let depth = |force, no_color, colorterm, term| ColorDepth::from_env(force, no_color, colorterm, term);
        assert_eq!(depth(None, false, Some("truecolor"), Some("xterm")), ColorDepth::Truecolor);
        assert_eq!(depth(None, false, Some("24bit"), None), ColorDepth::Truecolor);
        // Terminal.app: 256 colours but no COLORTERM.
        assert_eq!(depth(None, false, None, Some("xterm-256color")), ColorDepth::Indexed);
        assert_eq!(depth(None, false, None, Some("xterm")), ColorDepth::Ansi);
        assert_eq!(depth(None, false, None, None), ColorDepth::Ansi);
        assert_eq!(depth(None, true, Some("truecolor"), None), ColorDepth::Mono);
    }

    #[test]
    fn thoth_color_overrides_detection() {
        let force = |v| ColorDepth::from_env(Some(v), true, None, None);
        assert_eq!(force("truecolor"), ColorDepth::Truecolor);
        assert_eq!(force("256"), ColorDepth::Indexed);
        assert_eq!(force("16"), ColorDepth::Ansi);
        assert_eq!(force("none"), ColorDepth::Mono);
    }

    #[test]
    fn rgb_is_approximated_in_the_256_colour_palette() {
        // Pure cube corners and greys land on their exact entries.
        assert_eq!(nearest_indexed(0, 0, 0), 16);
        assert_eq!(nearest_indexed(255, 255, 255), 231);
        assert_eq!(nearest_indexed(255, 0, 0), 196);
        assert_eq!(nearest_indexed(128, 128, 128), 244);
        // The background is a near-black grey and teal is a cyan-ish cube colour.
        assert!((232..=235).contains(&nearest_indexed(0x08, 0x0D, 0x12)));
        assert_eq!(nearest_indexed(0x18, 0xC4, 0xC4), 44);
    }

    #[test]
    fn indexed_depth_uses_approximations_for_the_palette() {
        let theme = Theme::new(ColorDepth::Indexed);
        assert_eq!(theme.accent().fg, Some(Color::Indexed(44)));
        assert!(matches!(theme.on(BACKGROUND).bg, Some(Color::Indexed(_))));
    }

    #[test]
    fn foreign_styles_are_converted_and_lose_their_background() {
        let foreign =
            Style::new().fg(Color::Rgb(0xFF, 0, 0)).bg(Color::Rgb(255, 255, 255)).add_modifier(Modifier::ITALIC);
        let on_truecolor = Theme::new(ColorDepth::Truecolor).adapt(foreign);
        assert_eq!(on_truecolor.fg, Some(Color::Rgb(0xFF, 0, 0)));
        assert_eq!(on_truecolor.bg, None);
        assert!(on_truecolor.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(Theme::new(ColorDepth::Indexed).adapt(foreign).fg, Some(Color::Indexed(196)));
        assert_eq!(Theme::new(ColorDepth::Ansi).adapt(foreign).fg, Some(Color::Red));
        assert_eq!(Theme::new(ColorDepth::Mono).adapt(foreign).fg, None);
    }

    #[test]
    fn truecolor_uses_the_exact_palette_values() {
        let theme = truecolor();
        assert_eq!(theme.accent().fg, Some(Color::Rgb(0x18, 0xC4, 0xC4)));
        assert_eq!(theme.gold().fg, Some(Color::Rgb(0xF2, 0xBD, 0x59)));
        assert_eq!(theme.muted().fg, Some(Color::Rgb(0x68, 0x77, 0x86)));
        assert_eq!(theme.border().fg, Some(Color::Rgb(0x26, 0x35, 0x41)));
        assert_eq!(theme.status(Status::Error).fg, Some(Color::Rgb(0xFF, 0x6B, 0x70)));
    }

    #[test]
    fn ansi_falls_back_to_named_colours() {
        let theme = Theme::new(ColorDepth::Ansi);
        assert_eq!(theme.accent().fg, Some(Color::Cyan));
        assert_eq!(theme.status(Status::Running).fg, Some(Color::Green));
        assert_eq!(theme.status(Status::Error).fg, Some(Color::Red));
        assert_eq!(theme.on(BACKGROUND).bg, Some(Color::Reset));
    }

    #[test]
    fn no_color_drops_colours_but_keeps_emphasis() {
        let theme = Theme::new(ColorDepth::Mono);
        assert_eq!(theme.accent().fg, None);
        assert_eq!(theme.muted().fg, None);
        assert_eq!(theme.status(Status::Error).fg, None);
        assert!(theme.section_title().add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.selected().bg, None);
        assert!(theme.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn selection_is_teal_text_on_a_raised_surface() {
        let selected = truecolor().selected();
        assert_eq!(selected.fg, Some(Color::Rgb(0x18, 0xC4, 0xC4)));
        assert_eq!(selected.bg, Some(Color::Rgb(0x19, 0x2B, 0x32)));
        assert!(!selected.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn each_status_has_its_own_shade() {
        let theme = truecolor();
        assert_eq!(theme.status(Status::Running), theme.status(Status::Completed));
        assert_ne!(theme.status(Status::Running), theme.status(Status::Waiting));
        assert_ne!(theme.status(Status::Idle), theme.status(Status::Error));
    }

    #[test]
    fn headings_get_weaker_with_depth_and_avoid_underlines() {
        let theme = truecolor();
        assert!(theme.heading(1).add_modifier.contains(Modifier::BOLD));
        assert!(!theme.heading(1).add_modifier.contains(Modifier::UNDERLINED));
        assert!(!theme.heading(2).add_modifier.contains(Modifier::BOLD));
        assert!(theme.heading(3).add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn code_is_gold_only_when_colour_is_on() {
        assert_eq!(truecolor().code().fg, Some(Color::Rgb(0xB9, 0x92, 0x4A)));
        assert_eq!(Theme::new(ColorDepth::Mono).code().fg, None);
    }

    /// Draws `block` over a 6x3 area and returns the style of its top-left and centre cells.
    fn drawn(block: Block<'static>) -> (Style, Style) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(6, 3)).unwrap();
        terminal.draw(|frame| frame.render_widget(block, frame.area())).unwrap();
        let buffer = terminal.backend().buffer();
        (buffer[(0, 0)].style(), buffer[(2, 1)].style())
    }

    #[test]
    fn a_rule_is_teal_when_focused_and_sits_on_its_surface() {
        let theme = truecolor();
        let (rule, inside) = drawn(theme.rule(true));
        assert_eq!(rule.fg, theme.border_focused().fg);
        assert_eq!(inside.bg, Some(Color::Rgb(0x08, 0x0D, 0x12)));

        let (unfocused, _) = drawn(theme.rule(false));
        assert_eq!(unfocused.fg, theme.border().fg);

        let (_, elevated) = drawn(theme.rule_on(true, ELEVATED));
        assert_eq!(elevated.bg, Some(Color::Rgb(0x14, 0x1E, 0x28)));
    }

    #[test]
    fn a_pane_has_side_padding_and_no_border() {
        let area = Rect::new(0, 0, 10, 4);
        assert_eq!(Theme::pane().inner(area), Rect::new(1, 0, 8, 4));
        assert_eq!(truecolor().rule(true).inner(area), Rect::new(1, 1, 8, 3));
    }
}
