/// A scripted login worker.
pub mod scripted;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use std::collections::HashMap;
use std::sync::Mutex;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use super::github::Endpoints;
use crate::sync::MutexExt as _;

#[derive(Debug, Clone)]
/// A request the mock server saw.
pub struct Recorded {
    pub body: String,
    pub headers: Vec<(String, String)>,
    pub method: String,
    pub path: String,
}

impl Recorded {
    #[must_use]
    #[inline]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|entry| entry.0 == name).map(|entry| entry.1.as_str())
    }
}

/// Queued responses by path.
type Routes = HashMap<String, VecDeque<(u16, String)>>;
/// The responses to give for each path, in order.
pub type RouteList<'route> = Vec<(&'route str, Vec<(u16, String)>)>;

#[derive(Debug)]
/// A local HTTP server for tests.
#[expect(clippy::partial_pub_fields, reason = "the private fields keep the invariants of the type")]
pub struct MockServer {
    requests: Arc<Mutex<Vec<Recorded>>>,
    pub url: String,
}

impl MockServer {
    #[must_use]
    #[inline]
    /// Endpoints that point at this server.
    pub fn endpoints(&self) -> Endpoints {
        Endpoints {
            device_code: format!("{}/login/device/code", self.url),
            access_token: format!("{}/login/oauth/access_token", self.url),
            copilot_token: format!("{}/copilot_internal/v2/token", self.url),
        }
    }

    #[must_use]
    #[inline]
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock_unpoisoned().clone()
    }

    /// Serves each route's responses in order, repeating the last one.
    #[inline]
    /// Starts a server that answers `routes` in order.
    ///
    /// # Panics
    ///
    /// Panics if the local port cannot be bound.
    pub async fn start(routes: RouteList<'_>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared_routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(
            routes.into_iter().map(|(path, responses)| (path.to_owned(), responses.into())).collect(),
        ));
        let requests = Arc::new(Mutex::new(Vec::new()));

        let (served_routes, served_requests) = (shared_routes, Arc::clone(&requests));
        let _server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let _connection = tokio::spawn(serve(stream, Arc::clone(&served_routes), Arc::clone(&served_requests)));
            }
        });
        Self { requests, url }
    }
}

/// Answers one request from the queued responses.
async fn serve(mut stream: TcpStream, routes: Arc<Mutex<Routes>>, requests: Arc<Mutex<Vec<Recorded>>>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let path = request.path.split('?').next().unwrap_or_default().to_owned();
    requests.lock_unpoisoned().push(request);

    let (status, body) = {
        let mut locked = routes.lock_unpoisoned();
        match locked.get_mut(&path) {
            Some(queue) if queue.len() > 1 => queue.pop_front().unwrap(),
            Some(queue) => queue.front().cloned().unwrap_or((404, String::new())),
            None => (404, "{}".to_owned()),
        }
    };
    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _written = stream.write_all(response.as_bytes()).await;
    let _closed = stream.shutdown().await;
}

/// Reads one HTTP request, headers and body.
async fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break position.saturating_add(4);
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut start = lines.next()?.split_whitespace();
    let (method, path) = (start.next()?.to_owned(), start.next()?.to_owned());
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize =
        headers.iter().find(|entry| entry.0 == "content-length").and_then(|entry| entry.1.parse().ok()).unwrap_or(0);

    while buffer.len() < header_end.saturating_add(length) {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[header_end..]).into_owned();
    Some(Recorded { body, headers, method, path })
}
