use super::stats::Stats;
use crate::agent::Settings;
use crate::transcript::Change as TranscriptChange;

#[derive(Debug, Clone)]
/// A change to a Session's view.
pub enum Event {
    /// The Agent started or stopped working.
    Busy(bool),
    /// The model or thinking level changed.
    Settings(Settings),
    /// The token statistics changed.
    Stats(Stats),
    /// The transcript changed.
    Transcript(TranscriptChange),
}
