//! Control client: bind these subcommands to keys.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use mpris_controller::client;
use mpris_controller::protocol::{PlaybackStatus, Request};

#[derive(Parser)]
#[command(about = "Control the player selected in mpris-daemon")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Toggle play/pause on the selected player
    PlayPause,
    Play,
    Pause,
    /// Next track
    Next,
    /// Previous track
    Previous,
    /// Select the next player
    NextPlayer,
    /// Select the previous player
    PrevPlayer,
    /// Select a player by name (e.g. `spotify`, `firefox`, or its identity)
    Select { name: String },
    /// List players; the selected one is marked with `*`
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let req = match Cli::parse().command {
        Cmd::PlayPause => Request::PlayPause,
        Cmd::Play => Request::Play,
        Cmd::Pause => Request::Pause,
        Cmd::Next => Request::Next,
        Cmd::Previous => Request::Previous,
        Cmd::NextPlayer => Request::NextPlayer,
        Cmd::PrevPlayer => Request::PreviousPlayer,
        Cmd::Select { name } => Request::SelectPlayer { name },
        Cmd::List => {
            let state = client::get_state().await?;
            let sel = state.selected.as_ref().map(|p| p.bus_name.as_str());
            for p in &state.players {
                let mark = if Some(p.bus_name.as_str()) == sel { '*' } else { ' ' };
                let status = match p.status {
                    PlaybackStatus::Playing => "playing",
                    PlaybackStatus::Paused => "paused",
                    PlaybackStatus::Stopped => "stopped",
                };
                println!("{mark} {} ({}) [{status}] {} - {}", p.name, p.bus_name, p.artist, p.title);
            }
            return Ok(());
        }
    };
    if let Err(e) = client::request(&req).await {
        bail!("{e:#}");
    }
    Ok(())
}
