use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
/// How hard a model thinks.
pub enum Thinking {
    High,
    Low,
    Max,
    #[default]
    Medium,
    Minimal,
    Off,
}

impl Thinking {
    /// Every level, lowest first.
    pub const ALL: [Self; 6] = [Self::Off, Self::Minimal, Self::Low, Self::Medium, Self::High, Self::Max];

    #[must_use]
    #[inline]
    /// What the level means, in a few words.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Off => "no extra thinking",
            Self::Minimal => "a few tokens of reasoning",
            Self::Low => "quick, little reasoning",
            Self::Medium => "balanced",
            Self::High => "thorough, slower",
            Self::Max => "as much as the model allows",
        }
    }

    #[must_use]
    #[inline]
    /// The level's short name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}
