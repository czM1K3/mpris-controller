use crate::protocol::{Request, Response, State};
use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

pub struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Connection {
    pub async fn connect() -> Result<Self> {
        let path = crate::socket_path();
        let stream = UnixStream::connect(&path)
            .await
            .with_context(|| format!("cannot connect to daemon at {} (is it running?)", path.display()))?;
        let (r, writer) = stream.into_split();
        Ok(Self { reader: BufReader::new(r), writer })
    }

    pub async fn send(&mut self, req: &Request) -> Result<()> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Reads the next response; `None` when the daemon closed the connection.
    pub async fn recv(&mut self) -> Result<Option<Response>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
}

/// Sends one request and returns its response.
pub async fn request(req: &Request) -> Result<Response> {
    let mut conn = Connection::connect().await?;
    conn.send(req).await?;
    match conn.recv().await? {
        Some(Response::Error { message }) => bail!(message),
        Some(r) => Ok(r),
        None => bail!("daemon closed the connection"),
    }
}

pub async fn get_state() -> Result<State> {
    match request(&Request::GetState).await? {
        Response::State { state } => Ok(state),
        other => bail!("unexpected response: {other:?}"),
    }
}
