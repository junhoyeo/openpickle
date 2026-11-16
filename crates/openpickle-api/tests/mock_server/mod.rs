//! A small HTTP/1.1 server that records each request and answers with a reply chosen by a
//! handler. Shared by the integration tests and the `verify_capture` example.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// A request as the server received it. Header names are lowercase, in arrival order.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// The raw query string, without `?`.
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// Waits this long before answering.
    pub delay: Duration,
}

impl Reply {
    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: "application/json".into(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

pub struct MockServer {
    pub addr: SocketAddr,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, handler.clone(), recorded.clone()));
            }
        });
        Self { addr, requests }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn take(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

async fn serve(stream: TcpStream, handler: Arc<Handler>, requests: Arc<Mutex<Vec<Recorded>>>) {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let mut buffer = Vec::new();
    loop {
        let Some(request) = read_request(&mut read, &mut buffer).await else {
            return;
        };
        let reply = handler(&request);
        requests.lock().unwrap().push(request);
        if !reply.delay.is_zero() {
            tokio::time::sleep(reply.delay).await;
        }
        let head = format!(
            "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\n\r\n",
            reply.status,
            reply.content_type,
            reply.body.len()
        );
        if write.write_all(head.as_bytes()).await.is_err()
            || write.write_all(&reply.body).await.is_err()
        {
            return;
        }
    }
}

async fn read_request(
    read: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    buffer: &mut Vec<u8>,
) -> Option<Recorded> {
    let head_end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0u8; 8192];
        let n = read.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8(buffer[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .map_or(0, |(_, v)| v.parse().unwrap_or(0));
    let body_start = head_end + 4;
    while buffer.len() < body_start + length {
        let mut chunk = [0u8; 8192];
        let n = read.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    let body = buffer[body_start..body_start + length].to_vec();
    buffer.drain(..body_start + length);
    Some(Recorded {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body,
    })
}
