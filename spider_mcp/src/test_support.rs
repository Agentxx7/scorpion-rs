//! Shared, `#[cfg(test)]`-only fixtures for the Tor transport surface's
//! tool-level tests (`tools::scrape`, `tools::crawl`, `tools::links`, and
//! the source-reader tools) — a single blocking SOCKS5 fixture matching
//! the blocking `std::net` HTTP-fixture convention already established in
//! this crate's own test modules (see `tools::feed::tests::localhost`).
//! No public Tor dependency.

#![cfg(test)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SocksBehavior {
    Splice,
    Fail,
}

/// A minimal blocking SOCKS5 fixture: handles the greeting/CONNECT,
/// records connect count, and either splices every connection to a fixed
/// target or always fails (for "SOCKS failure -> zero direct fallback"
/// proofs).
pub(crate) struct SocksFixture {
    pub addr: std::net::SocketAddr,
    connect_count: Arc<AtomicUsize>,
}

impl SocksFixture {
    pub fn start(splice_to: Option<std::net::SocketAddr>, behavior: SocksBehavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connect_count = Arc::new(AtomicUsize::new(0));
        let connect_count_thread = connect_count.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let connect_count = connect_count_thread.clone();
                    std::thread::spawn(move || {
                        let _ = serve_one(stream, splice_to, behavior, connect_count);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });
        Self {
            addr,
            connect_count,
        }
    }

    pub fn connect_count(&self) -> usize {
        self.connect_count.load(Ordering::SeqCst)
    }
}

fn serve_one(
    mut stream: TcpStream,
    splice_to: Option<std::net::SocketAddr>,
    behavior: SocksBehavior,
    connect_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    let nmethods = header[1] as usize;
    let mut methods = vec![0_u8; nmethods];
    stream.read_exact(&mut methods)?;
    stream.write_all(&[0x05, 0x00])?;

    let mut req_head = [0_u8; 4];
    stream.read_exact(&mut req_head)?;
    match req_head[3] {
        0x01 => {
            let mut addr = [0_u8; 4];
            stream.read_exact(&mut addr)?;
        }
        0x03 => {
            let mut len_buf = [0_u8; 1];
            stream.read_exact(&mut len_buf)?;
            let mut name = vec![0_u8; len_buf[0] as usize];
            stream.read_exact(&mut name)?;
        }
        0x04 => {
            let mut addr = [0_u8; 16];
            stream.read_exact(&mut addr)?;
        }
        _ => return Ok(()),
    }
    let mut port_buf = [0_u8; 2];
    stream.read_exact(&mut port_buf)?;

    connect_count.fetch_add(1, Ordering::SeqCst);

    if behavior == SocksBehavior::Fail {
        stream.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
        return Ok(());
    }

    let Some(splice_to) = splice_to else {
        stream.write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;
        return Ok(());
    };

    stream.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])?;

    let mut upstream = TcpStream::connect(splice_to)?;
    let mut client_reader = stream.try_clone()?;
    let mut upstream_writer = upstream.try_clone()?;
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_reader, &mut upstream_writer);
    });
    let _ = std::io::copy(&mut upstream, &mut stream);
    let _ = up.join();
    Ok(())
}

/// A tiny blocking local HTTP fixture: serves one fixed 200 response body
/// for every request, records every path hit.
pub(crate) struct HttpFixture {
    pub addr: std::net::SocketAddr,
    hits: Arc<std::sync::Mutex<Vec<String>>>,
}

impl HttpFixture {
    pub fn start(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let hits_thread = hits.clone();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0_u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    hits_thread.lock().unwrap().push(path);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });
        Self { addr, hits }
    }

    pub fn hit_count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }

    pub fn hit_paths(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}
