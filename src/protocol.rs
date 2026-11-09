use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl PlaybackStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlayerInfo {
    /// Full D-Bus name, e.g. `org.mpris.MediaPlayer2.spotify`.
    pub bus_name: String,
    /// Human readable name (the MPRIS `Identity`), e.g. `Spotify`.
    pub name: String,
    pub status: PlaybackStatus,
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct State {
    /// The currently selected player, if any.
    pub selected: Option<PlayerInfo>,
    /// All known players, ordered by bus name.
    pub players: Vec<PlayerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
    /// Select the next player in the list (wraps around).
    NextPlayer,
    /// Select the previous player in the list (wraps around).
    PreviousPlayer,
    /// Select a player by bus name, bus name suffix (`spotify`) or identity.
    SelectPlayer { name: String },
    GetState,
    /// Keep the connection open and receive a `state` response on every change.
    Subscribe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error { message: String },
    State { state: State },
}
