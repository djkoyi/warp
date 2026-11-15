use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PaletteMode {
    Command,
    Navigation,
    LaunchConfig,
    WarpDrive,
    Files,
    Conversations,
    /// The user's recent Claude Code conversations, to resume one in a new tab.
    ClaudeSessions,
}
