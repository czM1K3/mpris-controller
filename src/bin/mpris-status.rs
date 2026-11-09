//! Status client: prints the current state, and keeps printing on every change.
//! Meant to be consumed by a bar widget (e.g. ags `subprocess`).

use anyhow::Result;
use clap::{Parser, ValueEnum};
use mpris_controller::client::Connection;
use mpris_controller::protocol::{PlaybackStatus, Request, Response, State};
use std::io::Write;
use std::time::Duration;

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// One JSON object per line (full state)
    Json,
    /// `Player: Artist - Title (playing)`
    Text,
}

#[derive(Parser)]
#[command(about = "Show the state of mpris-daemon")]
struct Cli {
    #[arg(long, value_enum, default_value = "json")]
    format: Format,
    /// Print the state once and exit instead of following changes
    #[arg(long)]
    once: bool,
}

fn render(state: &State, format: Format) -> String {
    match format {
        Format::Json => serde_json::to_string(state).expect("serializable"),
        Format::Text => match &state.selected {
            None => String::new(),
            Some(p) => {
                let status = match p.status {
                    PlaybackStatus::Playing => "playing",
                    PlaybackStatus::Paused => "paused",
                    PlaybackStatus::Stopped => "stopped",
                };
                format!("{}: {} - {} ({status})", p.name, p.artist, p.title)
            }
        },
    }
}

fn emit(state: &State, format: Format) {
    let mut out = std::io::stdout().lock();
    // Bar widgets close the pipe when they go away; stop quietly.
    if writeln!(out, "{}", render(state, format)).and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

async fn follow(format: Format) -> Result<()> {
    let mut conn = Connection::connect().await?;
    conn.send(&Request::Subscribe).await?;
    while let Some(resp) = conn.recv().await? {
        if let Response::State { state } = resp {
            emit(&state, format);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.once {
        let state = mpris_controller::client::get_state().await?;
        emit(&state, cli.format);
        return Ok(());
    }
    // Survive daemon restarts: show an empty state while it's down and reconnect.
    loop {
        if let Err(e) = follow(cli.format).await {
            eprintln!("{e:#}");
        }
        emit(&State::default(), cli.format);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
