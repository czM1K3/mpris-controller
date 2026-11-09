//! Main server: tracks MPRIS players over D-Bus and serves state/commands over a unix socket.

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use mpris_controller::protocol::{PlaybackStatus, PlayerInfo, Request, Response, State};
use std::collections::{BTreeMap, HashMap};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, proxy};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";

#[proxy(interface = "org.mpris.MediaPlayer2", default_path = "/org/mpris/MediaPlayer2")]
trait MediaPlayer2 {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

#[proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn play(&self) -> zbus::Result<()>;
    fn pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

enum Event {
    Added(String),
    Removed(String),
    Updated(PlayerInfo),
    Command(Request, oneshot::Sender<Response>),
}

/// Reads the full state of one player. `None` if it can't be queried (e.g. it just vanished).
async fn read_player(conn: &Connection, bus_name: &str) -> Option<PlayerInfo> {
    let root = MediaPlayer2Proxy::builder(conn)
        .destination(bus_name.to_owned())
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;
    let player = PlayerProxy::builder(conn)
        .destination(bus_name.to_owned())
        .ok()?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;

    let status = player.playback_status().await.ok()?;
    let meta = player.metadata().await.unwrap_or_default();
    let name = match root.identity().await {
        Ok(n) if !n.is_empty() => n,
        _ => bus_name.strip_prefix(PREFIX).unwrap_or(bus_name).to_owned(),
    };

    let text = |key: &str| -> String {
        let Some(v) = meta.get(key) else { return String::new() };
        let Ok(v) = v.try_clone() else { return String::new() };
        // `xesam:artist` is a string list; some players send a plain string.
        match String::try_from(v.try_clone().unwrap()) {
            Ok(s) => s,
            Err(_) => Vec::<String>::try_from(v).map(|v| v.join(", ")).unwrap_or_default(),
        }
    };

    Some(PlayerInfo {
        bus_name: bus_name.to_owned(),
        name,
        status: PlaybackStatus::parse(&status),
        title: text("xesam:title"),
        artist: text("xesam:artist"),
        album: text("xesam:album"),
    })
}

/// Follows one player and emits `Updated` on every property change.
async fn watch_player(conn: Connection, bus_name: String, tx: mpsc::UnboundedSender<Event>) {
    let result: Result<()> = async {
        let props = PropertiesProxy::builder(&conn)
            .destination(bus_name.clone())?
            .path(MPRIS_PATH)?
            .build()
            .await?;
        // Subscribe before the initial read so no change is missed.
        let mut changes = props.receive_properties_changed().await?;
        if let Some(info) = read_player(&conn, &bus_name).await {
            let _ = tx.send(Event::Updated(info));
        }
        while changes.next().await.is_some() {
            if let Some(info) = read_player(&conn, &bus_name).await {
                if tx.send(Event::Updated(info)).is_err() {
                    break;
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        eprintln!("watching {bus_name} failed: {e:#}");
    }
}

/// Finds players that exist now and reports ones that appear/disappear later.
async fn discover(conn: Connection, tx: mpsc::UnboundedSender<Event>) -> Result<()> {
    let dbus = DBusProxy::new(&conn).await?;
    // Subscribe first, then list, so nothing is lost in between.
    let mut owner_changes = dbus.receive_name_owner_changed().await?;
    for name in dbus.list_names().await? {
        if name.starts_with(PREFIX) {
            let _ = tx.send(Event::Added(name.to_string()));
        }
    }
    while let Some(sig) = owner_changes.next().await {
        let args = sig.args()?;
        let name = args.name().to_string();
        if !name.starts_with(PREFIX) {
            continue;
        }
        if args.new_owner().is_none() {
            let _ = tx.send(Event::Removed(name));
        } else {
            // New owner (or owner replaced): (re)start tracking.
            let _ = tx.send(Event::Removed(name.clone()));
            let _ = tx.send(Event::Added(name));
        }
    }
    bail!("D-Bus NameOwnerChanged stream ended")
}

struct Actor {
    conn: Connection,
    events: mpsc::UnboundedSender<Event>,
    players: BTreeMap<String, PlayerInfo>,
    tasks: HashMap<String, JoinHandle<()>>,
    selected: Option<String>,
    state: watch::Sender<State>,
}

impl Actor {
    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Added(name) => {
                if !self.tasks.contains_key(&name) {
                    let task = tokio::spawn(watch_player(
                        self.conn.clone(),
                        name.clone(),
                        self.events.clone(),
                    ));
                    self.tasks.insert(name, task);
                }
            }
            Event::Removed(name) => {
                if let Some(t) = self.tasks.remove(&name) {
                    t.abort();
                }
                self.players.remove(&name);
            }
            Event::Updated(info) => {
                // Ignore late updates from players we've already dropped.
                if !self.tasks.contains_key(&info.bus_name) {
                    return;
                }
                let started = info.status == PlaybackStatus::Playing
                    && self
                        .players
                        .get(&info.bus_name)
                        .is_none_or(|old| old.status != PlaybackStatus::Playing);
                let name = info.bus_name.clone();
                self.players.insert(name.clone(), info);
                // A player that starts playing takes over if the selected one isn't playing.
                if started && !self.selected_is_playing() {
                    self.selected = Some(name);
                }
            }
            Event::Command(req, reply) => {
                let resp = self.command(req);
                let _ = reply.send(resp);
            }
        }
        self.publish();
    }

    fn selected_is_playing(&self) -> bool {
        self.selected
            .as_ref()
            .and_then(|s| self.players.get(s))
            .is_some_and(|p| p.status == PlaybackStatus::Playing)
    }

    /// Keeps the selection valid and publishes the new state.
    fn publish(&mut self) {
        if self.selected.as_ref().is_none_or(|s| !self.players.contains_key(s)) {
            self.selected = self
                .players
                .values()
                .find(|p| p.status == PlaybackStatus::Playing)
                .or_else(|| self.players.values().next())
                .map(|p| p.bus_name.clone());
        }
        let state = State {
            selected: self.selected.as_ref().and_then(|s| self.players.get(s)).cloned(),
            players: self.players.values().cloned().collect(),
        };
        self.state.send_if_modified(|cur| {
            if *cur == state {
                false
            } else {
                *cur = state;
                true
            }
        });
    }

    fn cycle(&mut self, step: isize) -> Response {
        let names: Vec<&String> = self.players.keys().collect();
        if names.is_empty() {
            return err("no players available");
        }
        let cur = self
            .selected
            .as_ref()
            .and_then(|s| names.iter().position(|n| *n == s))
            .unwrap_or(0) as isize;
        let next = (cur + step).rem_euclid(names.len() as isize) as usize;
        self.selected = Some(names[next].clone());
        Response::Ok
    }

    fn select(&mut self, query: &str) -> Response {
        let q = query.to_lowercase();
        let found = self.players.values().find(|p| {
            let short = p.bus_name.strip_prefix(PREFIX).unwrap_or(&p.bus_name).to_lowercase();
            p.bus_name.to_lowercase() == q
                || short == q
                || short.split('.').next() == Some(q.as_str())
                || p.name.to_lowercase() == q
        });
        match found {
            Some(p) => {
                self.selected = Some(p.bus_name.clone());
                Response::Ok
            }
            None => err(format!("no player matching '{query}'")),
        }
    }

    fn command(&mut self, req: Request) -> Response {
        match req {
            Request::GetState => Response::State { state: self.state.borrow().clone() },
            // Handled by the connection task, which never forwards it here.
            Request::Subscribe => err("subscribe is handled per connection"),
            Request::NextPlayer => self.cycle(1),
            Request::PreviousPlayer => self.cycle(-1),
            Request::SelectPlayer { name } => self.select(&name),
            Request::PlayPause | Request::Play | Request::Pause | Request::Next | Request::Previous => {
                let Some(bus_name) = self.selected.clone() else {
                    return err("no player selected");
                };
                // Run the D-Bus call off the actor so a hung player can't block everything.
                let conn = self.conn.clone();
                tokio::spawn(async move {
                    let res = async {
                        let p = PlayerProxy::builder(&conn)
                            .destination(bus_name.clone())?
                            .cache_properties(CacheProperties::No)
                            .build()
                            .await?;
                        match req {
                            Request::PlayPause => p.play_pause().await,
                            Request::Play => p.play().await,
                            Request::Pause => p.pause().await,
                            Request::Next => p.next().await,
                            _ => p.previous().await,
                        }
                    }
                    .await;
                    if let Err(e) = res {
                        eprintln!("command failed: {e}");
                    }
                });
                Response::Ok
            }
        }
    }
}

fn err(message: impl Into<String>) -> Response {
    Response::Error { message: message.into() }
}

async fn send_line(w: &mut (impl AsyncWriteExt + Unpin), resp: &Response) -> std::io::Result<()> {
    let mut line = serde_json::to_string(resp).expect("serializable");
    line.push('\n');
    w.write_all(line.as_bytes()).await
}

async fn serve_client(
    stream: UnixStream,
    events: mpsc::UnboundedSender<Event>,
    mut state: watch::Receiver<State>,
) -> std::io::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                send_line(&mut w, &err(format!("bad request: {e}"))).await?;
                continue;
            }
        };
        if matches!(req, Request::Subscribe) {
            let current = state.borrow_and_update().clone();
            send_line(&mut w, &Response::State { state: current }).await?;
            loop {
                tokio::select! {
                    changed = state.changed() => {
                        if changed.is_err() { return Ok(()); }
                        let current = state.borrow_and_update().clone();
                        send_line(&mut w, &Response::State { state: current }).await?;
                    }
                    // Detect the client going away.
                    next = lines.next_line() => {
                        if next?.is_none() { return Ok(()); }
                    }
                }
            }
        }
        let (tx, rx) = oneshot::channel();
        if events.send(Event::Command(req, tx)).is_err() {
            return Ok(());
        }
        let resp = rx.await.unwrap_or_else(|_| err("daemon shutting down"));
        send_line(&mut w, &resp).await?;
    }
    Ok(())
}

async fn bind_socket() -> Result<UnixListener> {
    let path = mpris_controller::socket_path();
    if path.exists() {
        if UnixStream::connect(&path).await.is_ok() {
            bail!("another daemon is already running on {}", path.display());
        }
        std::fs::remove_file(&path).context("removing stale socket")?;
    }
    UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let listener = bind_socket().await?;
    let conn = Connection::session().await.context("connecting to the session bus")?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let (state_tx, state_rx) = watch::channel(State::default());

    tokio::spawn({
        let (conn, tx) = (conn.clone(), tx.clone());
        async move {
            if let Err(e) = discover(conn, tx).await {
                eprintln!("player discovery stopped: {e:#}");
                std::process::exit(1);
            }
        }
    });

    tokio::spawn({
        let tx = tx.clone();
        async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { continue };
                let (tx, state) = (tx.clone(), state_rx.clone());
                tokio::spawn(async move {
                    let _ = serve_client(stream, tx, state).await;
                });
            }
        }
    });

    let mut actor = Actor {
        conn,
        events: tx,
        players: BTreeMap::new(),
        tasks: HashMap::new(),
        selected: None,
        state: state_tx,
    };

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            Some(ev) = rx.recv() => actor.handle(ev),
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
        }
    }
    let _ = std::fs::remove_file(mpris_controller::socket_path());
    Ok(())
}
