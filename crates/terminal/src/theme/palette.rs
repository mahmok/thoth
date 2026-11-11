use ratatui::style::Color;

// Backgrounds and surfaces. `Reset` keeps the terminal's own background on 16-colour terminals.
/// The app background.
pub const BACKGROUND: Shade = shade(0x0008_0D12, Color::Reset);
/// A panel on the background.
pub const SURFACE: Shade = shade(0x000E_151D, Color::Reset);
/// A panel above a surface.
pub const ELEVATED: Shade = shade(0x0014_1E28, Color::Reset);
/// A selected row.
pub const SELECTED_SURFACE: Shade = shade(0x0019_2B32, Color::DarkGray);

// Text.
/// Body text.
pub const TEXT: Shade = shade(0x00E6_EDF3, Color::Reset);
/// Less important text.
pub const TEXT_SECONDARY: Shade = shade(0x00A0_ADBA, Color::Gray);
/// The quietest text.
pub const TEXT_MUTED: Shade = shade(0x0068_7786, Color::DarkGray);

// Brand. Teal is the main accent; gold is for rare, meaningful emphasis.
/// The accent colour.
pub const TEAL: Shade = shade(0x0018_C4C4, Color::Cyan);
/// The accent colour, brighter.
pub const TEAL_BRIGHT: Shade = shade(0x0045_D9D0, Color::LightCyan);
/// The highlight colour.
pub const GOLD: Shade = shade(0x00F2_BD59, Color::Yellow);
/// The highlight colour, quieter.
pub const GOLD_MUTED: Shade = shade(0x00B9_924A, Color::Yellow);

// Borders.
/// A pane border.
pub const BORDER: Shade = shade(0x0026_3541, Color::DarkGray);
/// The border of the focused pane.
pub const BORDER_FOCUS: Shade = TEAL;

// Semantic states. Each state has one meaning everywhere it appears.
/// Something in progress.
pub const RUNNING: Shade = shade(0x0045_D9A0, Color::Green);
/// Something waiting.
pub const WAITING: Shade = GOLD;
/// Something idle.
pub const IDLE: Shade = shade(0x0083_93A4, Color::Gray);
/// Something done.
pub const COMPLETED: Shade = RUNNING;
/// Something that failed.
pub const ERROR: Shade = shade(0x00FF_6B70, Color::Red);

/// A palette entry: its exact value on a truecolor terminal and the nearest of the 16 named
/// colours for everything else, so the theme still reads on terminals without RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shade {
    pub ansi: Color,
    pub rgb: Color,
}

#[expect(clippy::big_endian_bytes, reason = "a colour is written as big-endian hex: 0xRRGGBB")]
const fn hex(value: u32) -> Color {
    let [_pad, red_part, green_part, blue_part] = value.to_be_bytes();
    Color::Rgb(red_part, green_part, blue_part)
}

const fn shade(value: u32, ansi: Color) -> Shade {
    Shade { rgb: hex(value), ansi }
}
