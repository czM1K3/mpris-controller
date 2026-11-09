# mpris-controller

A small set of tools for managing MPRIS media players (Spotify, browsers, etc.) from a Wayland compositor such as Hyprland or sway.

It consists of three binaries that share one Cargo package:

| Binary | Role |
| --- | --- |
| `mpris-daemon` | Server. Tracks all MPRIS players over D-Bus, keeps the selected player, and serves state and commands over a unix socket. Started by the compositor. |
| `mpris-ctl` | Control client. Switches the selected player and controls it (play/pause, next, previous). Meant to be bound to keys. |
| `mpris-status` | Status client. Prints the selected player, track, artist and playing state, and keeps printing on every change. Meant for a bar widget (e.g. ags). |

> This project was built by Claude (Anthropic's AI model, via Claude Code).

## Installation

```sh
cargo install --path .
```

This installs `mpris-daemon`, `mpris-ctl` and `mpris-status` into `~/.cargo/bin`, so make sure that directory is on your `PATH`.

## Usage

### Daemon

```sh
mpris-daemon
```

- Connects to the D-Bus session bus and discovers existing and future players (`org.mpris.MediaPlayer2.*`).
- Listens on `$XDG_RUNTIME_DIR/mpris-controller.sock`. It refuses to start if another daemon is already listening there, and removes a stale socket left by a crash.
- Shuts down cleanly on SIGINT/SIGTERM and removes the socket.

#### Selection rules

- If nothing is selected (or the selected player disappears), the first playing player is chosen, otherwise the first player in bus-name order.
- If the selected player is not playing and another player starts playing, selection moves to the one that started.
- A manual selection is kept as long as the selected player is playing or no other player starts.

### `mpris-ctl`

```sh
mpris-ctl play-pause      # toggle play/pause on the selected player
mpris-ctl play
mpris-ctl pause
mpris-ctl next            # next track
mpris-ctl previous        # previous track
mpris-ctl next-player     # select next player (wraps around)
mpris-ctl prev-player     # select previous player (wraps around)
mpris-ctl select spotify  # select by name
mpris-ctl list            # list players, selected one marked with *
```

`select` matches, case-insensitively, the full bus name, the bus name without the `org.mpris.MediaPlayer2.` prefix (`spotify`, `firefox`), the part before the first dot of that suffix, or the player's identity (`Spotify`, `Mozilla zen`).

Exit code is non-zero and an error is printed if the daemon isn't running or the command fails (e.g. no player selected).

### `mpris-status`

```sh
mpris-status                 # follow changes, one JSON object per line
mpris-status --once          # print current state once and exit
mpris-status --format text   # human-readable: "Spotify: Artist - Title (playing)"
```

JSON output:

```json
{
  "selected": {
    "bus_name": "org.mpris.MediaPlayer2.spotify",
    "name": "Spotify",
    "status": "playing",
    "title": "Song",
    "artist": "Artist",
    "album": "Album"
  },
  "players": [ { "...": "same shape as selected" } ]
}
```

- `status` is one of `playing`, `paused`, `stopped`.
- `selected` is `null` when there are no players.
- In follow mode, if the daemon goes away an empty state is printed and the client reconnects every second, so a bar widget keeps working across daemon restarts.

## Compositor setup

### Hyprland (`hyprland.conf`)

```ini
exec-once = mpris-daemon

bind = , XF86AudioPlay, exec, mpris-ctl play-pause
bind = , XF86AudioNext, exec, mpris-ctl next
bind = , XF86AudioPrev, exec, mpris-ctl previous
bind = SUPER, bracketright, exec, mpris-ctl next-player
bind = SUPER, bracketleft,  exec, mpris-ctl prev-player
```

Remove any other handler bound to the same media keys (e.g. `playerctl`/`playerctld` bindings) to avoid double actions. `exec-once` only runs at compositor start, so start the daemon manually the first time.

### sway

```
exec mpris-daemon
bindsym XF86AudioPlay exec mpris-ctl play-pause
bindsym XF86AudioNext exec mpris-ctl next
bindsym XF86AudioPrev exec mpris-ctl previous
```

### ags widget

Run `mpris-status` as a long-lived subprocess and parse each line as JSON. With ags v2 (Astal):

```ts
const media = Variable({ selected: null, players: [] }).watch(
  ["mpris-status"], (line, prev) => JSON.parse(line))
```

Use `selected.name`, `selected.title`, `selected.artist` and `selected.status` in the widget.

## IPC protocol

Newline-delimited JSON over the unix socket. The client sends one request per line and gets one response per line.

Requests (`cmd` field):

```json
{"cmd":"play_pause"}
{"cmd":"play"}
{"cmd":"pause"}
{"cmd":"next"}
{"cmd":"previous"}
{"cmd":"next_player"}
{"cmd":"previous_player"}
{"cmd":"select_player","name":"spotify"}
{"cmd":"get_state"}
{"cmd":"subscribe"}
```

Responses (`type` field):

```json
{"type":"ok"}
{"type":"error","message":"no player selected"}
{"type":"state","state":{"selected":null,"players":[]}}
```

After `subscribe`, the daemon sends a `state` response immediately and again on every change until the client disconnects. You can talk to it directly, e.g.:

```sh
echo '{"cmd":"get_state"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/mpris-controller.sock
```

## Project layout

```
src/lib.rs               socket path, module exports
src/protocol.rs          request/response/state types
src/client.rs            client helpers shared by mpris-ctl and mpris-status
src/bin/mpris-daemon.rs  D-Bus tracking + socket server
src/bin/mpris-ctl.rs     key-bound control client
src/bin/mpris-status.rs  status/follow client
```

The daemon uses [zbus](https://crates.io/crates/zbus) for D-Bus and [tokio](https://tokio.rs) for async I/O.

## Notes

- Only D-Bus changes that players announce through `PropertiesChanged` are tracked; playback position is not.
- Requires a running D-Bus session bus (standard on desktop Linux).
