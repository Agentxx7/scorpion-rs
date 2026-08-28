use scorpion_app::{error_json, error_status, search, SearchError, SearchRequest};
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_BODY_BYTES: usize = 64 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = env::var("SCORPION_API_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = TcpListener::bind(&bind).await?;
    eprintln!("scorpion-api listening on {bind}");
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = handle(stream).await {
                eprintln!("scorpion-api request error: {error}");
            }
        });
    }
}

async fn handle(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > MAX_BODY_BYTES {
            write_json(
                &mut stream,
                413,
                r#"{"error":{"code":"request_too_large","message":"request body too large"}}"#,
            )
            .await?;
            return Ok(());
        }
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let delimiter = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("malformed HTTP request")?;
    let body_offset = delimiter + 4;
    let head = String::from_utf8_lossy(&bytes[..delimiter]);
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let content_length = lines
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < body_offset + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if method == "GET" && path == "/health" {
        return write_json(&mut stream, 200, r#"{"status":"ok"}"#)
            .await
            .map_err(Into::into);
    }
    if method != "POST" || path != "/api/search" {
        return write_json(
            &mut stream,
            404,
            r#"{"error":{"code":"not_found","message":"route not found"}}"#,
        )
        .await
        .map_err(Into::into);
    }
    let body = bytes
        .get(body_offset..body_offset + content_length)
        .unwrap_or_default();
    let input: SearchRequest = match serde_json::from_slice(body) {
        Ok(input) => input,
        Err(error) => {
            let failure = SearchError::InvalidRequest(format!("invalid JSON body: {error}"));
            return write_json(&mut stream, error_status(&failure), &error_json(&failure))
                .await
                .map_err(Into::into);
        }
    };
    let base_url = env::var("SEARXNG_BASE_URL").ok();
    match search(input, base_url.as_deref()).await {
        Ok(response) => write_json(&mut stream, 200, &serde_json::to_string(&response)?)
            .await
            .map_err(Into::into),
        Err(error) => write_json(&mut stream, error_status(&error), &error_json(&error))
            .await
            .map_err(Into::into),
    }
}

async fn write_json(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), std::io::Error> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let response = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(response.as_bytes()).await
}
