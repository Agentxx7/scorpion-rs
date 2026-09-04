use scorpion_app::audit::{audit_error_json, audit_error_status, run_audit, AuditRequest};
use scorpion_app::evidence::{
    content_filename, evidence, evidence_content, evidence_error_json, evidence_error_status,
    export_filename, EvidenceError,
};
use scorpion_app::fetch::{fetch_error_json, fetch_error_status, run_fetch, FetchRequest};
use scorpion_app::iam::{
    callback_received_page, create_trace, iam_error_json, iam_error_status, read_trace,
    receive_callback,
};
use scorpion_app::{
    error_json, error_status, research_availability, research_error_json, research_error_status,
    search, ResearchAvailability, ResearchError, ResearchRequest, ResearchService, SearchError,
    SearchRequest,
};
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_BODY_BYTES: usize = 64 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = env::var("SCORPION_API_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = TcpListener::bind(&bind).await?;
    let research = ResearchService::default();
    eprintln!("scorpion-api listening on {bind}");
    loop {
        let (stream, _) = listener.accept().await?;
        let research = research.clone();
        tokio::spawn(async move {
            if let Err(error) = handle(stream, research).await {
                eprintln!("scorpion-api request error: {error}");
            }
        });
    }
}

async fn handle(
    mut stream: TcpStream,
    research: ResearchService,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    // Single pass over the remaining header lines: Content-Length was the
    // only header any existing route needed; the IAM callback receiver
    // additionally needs Content-Type to distinguish
    // application/x-www-form-urlencoded from application/json (see
    // `scorpion_app::iam`). Purely additive — no existing route reads
    // `content_type`, so this changes nothing for them.
    let mut content_length: usize = 0;
    let mut content_type: Option<String> = None;
    for line in lines {
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line
            .strip_prefix("Content-Type:")
            .or_else(|| line.strip_prefix("content-type:"))
        {
            content_type = Some(value.trim().to_string());
        }
    }
    if content_length > MAX_BODY_BYTES {
        return write_json(
            &mut stream,
            413,
            r#"{"error":{"code":"request_too_large","message":"request body too large"}}"#,
        )
        .await
        .map_err(Into::into);
    }
    while bytes.len() < body_offset + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    if method == "GET" && path == "/" {
        let page = render_index(research_availability());
        return write_html(&mut stream, &page).await.map_err(Into::into);
    }
    if method == "GET" && path == "/health" {
        return write_json(&mut stream, 200, r#"{"status":"ok"}"#)
            .await
            .map_err(Into::into);
    }
    if method == "GET" && path.starts_with("/api/evidence/") {
        let rest = &path["/api/evidence/".len()..];
        if let Some(raw_ref) = rest.strip_suffix("/content") {
            return handle_evidence_content(&mut stream, raw_ref).await;
        }
        if let Some(raw_ref) = rest.strip_suffix("/export") {
            return handle_evidence_export(&mut stream, raw_ref).await;
        }
        let raw_ref = rest;
        return match evidence(raw_ref).await {
            Ok(bundle) => write_json(&mut stream, 200, &serde_json::to_string(&bundle)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                evidence_error_status(&error),
                &evidence_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "GET" && path.starts_with("/api/research/") {
        let raw_id = &path["/api/research/".len()..];
        return match research.status(raw_id).await {
            Ok(status) => write_json(&mut stream, 200, &serde_json::to_string(&status)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                research_error_status(&error),
                &research_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "POST" && path == "/api/research" {
        let body = bytes
            .get(body_offset..body_offset + content_length)
            .unwrap_or_default();
        let input: ResearchRequest = match serde_json::from_slice(body) {
            Ok(input) => input,
            Err(_) => {
                let error = ResearchError::InvalidRequest("invalid JSON body".into());
                return write_json(
                    &mut stream,
                    research_error_status(&error),
                    &research_error_json(&error),
                )
                .await
                .map_err(Into::into);
            }
        };
        return match research.submit(input).await {
            Ok(accepted) => write_json(&mut stream, 202, &serde_json::to_string(&accepted)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                research_error_status(&error),
                &research_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "POST" && path == "/api/audit" {
        let body = bytes
            .get(body_offset..body_offset + content_length)
            .unwrap_or_default();
        let input: AuditRequest = match serde_json::from_slice(body) {
            Ok(input) => input,
            Err(error) => {
                let failure = scorpion_app::audit::AuditError::InvalidRequest(format!(
                    "invalid JSON body: {error}"
                ));
                return write_json(
                    &mut stream,
                    audit_error_status(&failure),
                    &audit_error_json(&failure),
                )
                .await
                .map_err(Into::into);
            }
        };
        return match run_audit(input).await {
            Ok(response) => write_json(&mut stream, 200, &serde_json::to_string(&response)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                audit_error_status(&error),
                &audit_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "POST" && path == "/api/fetch" {
        let body = bytes
            .get(body_offset..body_offset + content_length)
            .unwrap_or_default();
        let input: FetchRequest = match serde_json::from_slice(body) {
            Ok(input) => input,
            Err(error) => {
                let failure = scorpion_app::fetch::FetchError::InvalidRequest(format!(
                    "invalid JSON body: {error}"
                ));
                return write_json(
                    &mut stream,
                    fetch_error_status(&failure),
                    &fetch_error_json(&failure),
                )
                .await
                .map_err(Into::into);
            }
        };
        return match run_fetch(input).await {
            Ok(response) => write_json(&mut stream, 200, &serde_json::to_string(&response)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                fetch_error_status(&error),
                &fetch_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "POST" && path == "/api/iam/traces" {
        return match create_trace().await {
            Ok(response) => write_json(&mut stream, 201, &serde_json::to_string(&response)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                iam_error_status(&error),
                &iam_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if method == "GET" && path.starts_with("/api/iam/traces/") {
        let raw_id = &path["/api/iam/traces/".len()..];
        return match read_trace(raw_id).await {
            Ok(view) => write_json(&mut stream, 200, &serde_json::to_string(&view)?)
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                iam_error_status(&error),
                &iam_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
    }
    if (method == "GET" || method == "POST") && path.starts_with("/iam/callback/") {
        let rest = &path["/iam/callback/".len()..];
        // Passive receiver: split the trace id off any GET query string
        // locally, here, rather than changing how the shared `path`
        // variable above is matched for every other route.
        let (raw_id, query) = rest.split_once('?').unwrap_or((rest, ""));
        let body = bytes
            .get(body_offset..body_offset + content_length)
            .unwrap_or_default();
        return match receive_callback(raw_id, &method, query, content_type.as_deref(), body).await {
            Ok(outcome) => write_html(&mut stream, &callback_received_page(&outcome.trace_id))
                .await
                .map_err(Into::into),
            Err(error) => write_json(
                &mut stream,
                iam_error_status(&error),
                &iam_error_json(&error),
            )
            .await
            .map_err(Into::into),
        };
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

/// `GET /api/evidence/{ref}/content` — "Download captured content".
/// Resolves through the exact same canonical `evidence` seam the plain
/// inspection route uses (no second read implementation, no re-fetch) and
/// returns exactly the stored `EvidenceBundle.content` bytes, unmodified,
/// as a `text/plain` attachment named only from the canonical
/// `EvidenceId` — never from the target-controlled URL
/// (`SCORPION_RESEARCH_EVIDENCE_NAVIGATION_AND_EXPORT_UX_001` Phase 7).
async fn handle_evidence_content(
    stream: &mut TcpStream,
    raw_ref: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match evidence_content(raw_ref).await {
        Ok((id, content)) => write_download(
            stream,
            "text/plain; charset=utf-8",
            &content_filename(id),
            content.as_bytes(),
        )
        .await
        .map_err(Into::into),
        Err(error) => write_json(
            stream,
            evidence_error_status(&error),
            &evidence_error_json(&error),
        )
        .await
        .map_err(Into::into),
    }
}

/// `GET /api/evidence/{ref}/export` — "Download canonical JSON". Resolves
/// through the exact same canonical `evidence` seam and re-serializes the
/// identical `EvidenceBundle` value the plain inspection route returns —
/// same fields, no reconstruction, no recalculated hashes — as an
/// `application/json` attachment named only from the canonical
/// `EvidenceId` (Phase 8).
async fn handle_evidence_export(
    stream: &mut TcpStream,
    raw_ref: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match evidence(raw_ref).await {
        Ok(bundle) => {
            let Some(id) = bundle.id else {
                let error = EvidenceError::ReadFailed;
                return write_json(
                    stream,
                    evidence_error_status(&error),
                    &evidence_error_json(&error),
                )
                .await
                .map_err(Into::into);
            };
            let json = serde_json::to_string(&bundle)?;
            write_download(
                stream,
                "application/json",
                &export_filename(id),
                json.as_bytes(),
            )
            .await
            .map_err(Into::into)
        }
        Err(error) => write_json(
            stream,
            evidence_error_status(&error),
            &evidence_error_json(&error),
        )
        .await
        .map_err(Into::into),
    }
}

/// Write a `200 OK` file-download response. `filename` must already be a
/// safe deterministic value (this crate only ever calls it with
/// `content_filename`/`export_filename`, both derived solely from the
/// canonical `EvidenceId`'s fixed hex charset) — never built from
/// target-controlled input, since it is placed directly into the
/// `Content-Disposition` header.
async fn write_download(
    stream: &mut TcpStream,
    content_type: &str,
    filename: &str,
    body: &[u8],
) -> Result<(), std::io::Error> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Disposition: attachment; filename=\"{filename}\"\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await
}

async fn write_json(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), std::io::Error> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
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

async fn write_html(stream: &mut TcpStream, body: &str) -> Result<(), std::io::Error> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'self'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}

fn render_index(availability: ResearchAvailability) -> String {
    let (disabled, message): (&str, &str) = match availability {
        ResearchAvailability::Available => ("", "Research is configured."),
        ResearchAvailability::NotConfigured => (" disabled", "Research is not configured."),
        ResearchAvailability::UnsupportedProvider(_) => (
            " disabled",
            "Research is not available: the configured search provider is not supported by this build.",
        ),
        ResearchAvailability::InvalidConfiguration(_) => (
            " disabled",
            "Research is not available: the configured search provider selection is invalid.",
        ),
        ResearchAvailability::ConfigurationInvalid => (
            " disabled",
            "Research is not available: the configured research runtime settings are invalid.",
        ),
    };
    INDEX_HTML
        .replace("{{RESEARCH_DISABLED}}", disabled)
        .replace("{{RESEARCH_AVAILABILITY}}", message)
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Scorpion Search</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { margin: 0; background: #f7f8fa; color: #18202a; }
    main { width: min(780px, calc(100% - 2rem)); margin: 12vh auto; }
    h1 { font-size: clamp(2rem, 6vw, 3.5rem); margin: 0 0 .35rem; }
    .tagline { color: #536170; margin: 0 0 2rem; }
    form { display: flex; gap: .6rem; }
    input { flex: 1; min-width: 0; padding: .85rem 1rem; border: 1px solid #aeb8c4; border-radius: .55rem; font-size: 1rem; background: white; color: #18202a; }
    button { padding: .85rem 1.2rem; border: 0; border-radius: .55rem; background: #165dff; color: white; font-weight: 650; cursor: pointer; }
    button:disabled { opacity: .6; cursor: wait; }
    #research-button:disabled { cursor: not-allowed; }
    #status { min-height: 1.5rem; margin: 1rem 0; color: #536170; }
    #status.error { color: #b42318; }
    section { margin-top: 3rem; padding-top: 2rem; border-top: 1px solid #d6dce3; }
    #research-status.error { color: #b42318; }
    #research-result { white-space: pre-wrap; }
    ol { padding-left: 1.4rem; }
    li { margin: 0 0 1.4rem; }
    a { color: #165dff; font-size: 1.1rem; }
    .url { color: #536170; font-size: .85rem; overflow-wrap: anywhere; }
    .snippet { margin: .3rem 0; }
    .meta { color: #536170; font-size: .85rem; }
    .evidence-field { margin: .2rem 0; overflow-wrap: anywhere; }
    #evidence-result pre { background: #eef1f5; border: 1px solid #d6dce3; border-radius: .4rem; padding: .75rem; overflow-x: auto; white-space: pre-wrap; word-break: break-word; }
    #evidence-result details { margin: .9rem 0; }
    #evidence-result summary { cursor: pointer; font-weight: 650; }
    .evidence-actions { display: flex; flex-wrap: wrap; gap: .6rem; margin: .9rem 0; }
    .evidence-actions a { font-size: .95rem; padding: .5rem .85rem; border-radius: .45rem; background: #eef1f5; text-decoration: none; }
    .citation-link { display: inline; padding: 0 .1rem; border: 0; background: none; color: #165dff; font: inherit; font-weight: 650; cursor: pointer; text-decoration: underline; }
    .citation-link:hover, .citation-link:focus-visible { color: #0d3fb8; }
    .audit-field { margin: .2rem 0; overflow-wrap: anywhere; }
    .audit-finding { margin: 0 0 1rem; padding: .6rem 0 .6rem .8rem; border-left: 3px solid #d6dce3; }
    #audit-result pre { background: #eef1f5; border: 1px solid #d6dce3; border-radius: .4rem; padding: .75rem; overflow-x: auto; white-space: pre-wrap; word-break: break-word; }
    @media (prefers-color-scheme: dark) { body { background: #101418; color: #e6edf3; } input { background: #1b222c; color: #e6edf3; border-color: #536170; } #evidence-result pre, #audit-result pre { background: #1b222c; border-color: #536170; } .audit-finding { border-left-color: #536170; } .evidence-actions a { background: #1b222c; } .citation-link { color: #6ea8ff; } .citation-link:hover, .citation-link:focus-visible { color: #9cc4ff; } }
  </style>
</head>
<body>
  <main>
    <h1>Scorpion</h1>
    <p class="tagline">Evidence-first web search and acquisition engine</p>
    <form id="search-form">
      <label for="query" hidden>Search query</label>
      <input id="query" name="query" type="search" placeholder="Search the web" autocomplete="off" required>
      <button id="search-button" type="submit">Search</button>
    </form>
    <div id="status" role="status" aria-live="polite"></div>
    <ol id="results"></ol>
    <section aria-labelledby="research-heading">
      <h2 id="research-heading">Research</h2>
      <p class="tagline">Run durable research with canonical evidence and synthesis.</p>
      <p id="research-availability">{{RESEARCH_AVAILABILITY}}</p>
      <form id="research-form">
        <label for="research-topic" hidden>Research topic</label>
        <input id="research-topic" name="topic" type="text" placeholder="Research a topic" autocomplete="off" required>
        <button id="research-button" type="submit"{{RESEARCH_DISABLED}}>Start Research</button>
      </form>
      <div id="research-status" role="status" aria-live="polite"></div>
      <div id="research-result"></div>
    </section>
    <section aria-labelledby="evidence-heading">
      <h2 id="evidence-heading">Evidence Inspector</h2>
      <p class="tagline">Inspect the exact canonical evidence record an AI resolved through MCP — no re-fetch, no reconstruction.</p>
      <form id="evidence-form">
        <label for="evidence-ref" hidden>Evidence reference</label>
        <input id="evidence-ref" name="evidence_ref" type="text" placeholder="evid_..." autocomplete="off" required>
        <button id="evidence-button" type="submit">Inspect evidence</button>
      </form>
      <div id="evidence-status" role="status" aria-live="polite"></div>
      <div id="evidence-result"></div>
    </section>
    <section aria-labelledby="audit-heading">
      <h2 id="audit-heading">Page Audit</h2>
      <p class="tagline">Run the canonical deterministic page audit — the same rules and technology observations an AI runs through MCP.</p>
      <form id="audit-form">
        <label for="audit-url" hidden>Audit URL</label>
        <input id="audit-url" name="url" type="url" placeholder="https://..." autocomplete="off" required>
        <button id="audit-button" type="submit">Run audit</button>
      </form>
      <div id="audit-status" role="status" aria-live="polite"></div>
      <div id="audit-result"></div>
    </section>
    <section aria-labelledby="fetch-heading">
      <h2 id="fetch-heading">Fetch</h2>
      <p class="tagline">Fetch exactly one resource over HTTP(S) — no crawl, no browser, no link following — and record its retrieval as durable evidence.</p>
      <form id="fetch-form">
        <label for="fetch-url" hidden>Fetch URL</label>
        <input id="fetch-url" name="url" type="url" placeholder="https://..." autocomplete="off" required>
        <button id="fetch-button" type="submit">Fetch</button>
      </form>
      <div id="fetch-status" role="status" aria-live="polite"></div>
      <div id="fetch-result"></div>
    </section>
  </main>
  <script>
    const form = document.getElementById('search-form');
    const input = document.getElementById('query');
    const button = document.getElementById('search-button');
    const status = document.getElementById('status');
    const results = document.getElementById('results');
    const researchForm = document.getElementById('research-form');
    const researchTopic = document.getElementById('research-topic');
    const researchButton = document.getElementById('research-button');
    const researchStatus = document.getElementById('research-status');
    const researchResult = document.getElementById('research-result');
    let researchGeneration = 0;
    let researchTimer = null;
    const text = (value) => document.createTextNode(value ?? '');
    function showError(message) {
      status.className = 'error';
      status.replaceChildren(text(message));
      results.replaceChildren();
    }
    form.addEventListener('submit', async (event) => {
      event.preventDefault();
      const query = input.value.trim();
      if (!query) { showError('Enter a search query.'); return; }
      button.disabled = true;
      status.className = '';
      status.replaceChildren(text('Searching…'));
      results.replaceChildren();
      try {
        const response = await fetch('/api/search', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ query, limit: 10 })
        });
        const payload = await response.json();
        if (!response.ok) {
          showError(payload?.error?.message || 'Search is unavailable.');
          return;
        }
        status.className = '';
        status.replaceChildren(text(payload.result_count ? `${payload.result_count} results` : 'No results found.'));
        for (const result of payload.results || []) {
          const item = document.createElement('li');
          const link = document.createElement('a');
          link.href = result.url;
          link.target = '_blank';
          link.rel = 'noopener noreferrer';
          link.appendChild(text(result.title || result.url));
          const url = document.createElement('div'); url.className = 'url'; url.appendChild(text(result.url));
          const snippet = document.createElement('div'); snippet.className = 'snippet'; snippet.appendChild(text(result.snippet));
          const meta = document.createElement('div'); meta.className = 'meta';
          if (result.date) meta.appendChild(text(result.date));
          item.append(link, url, snippet, meta); results.appendChild(item);
        }
      } catch (_) { showError('Search is unavailable.'); }
      finally { button.disabled = false; }
    });
    const terminalResearchStates = new Set([
      'search_failed', 'completed_no_search_results', 'completed_no_observed_acquisitions',
      'completed_no_extractions', 'completed_without_synthesis_requested',
      'completed_synthesis_insufficient', 'completed_synthesis_failed', 'completed_successfully'
    ]);
    const researchStateLabel = (state) => state.replaceAll('_', ' ');
    function renderResearchError(message) {
      researchStatus.className = 'error';
      researchStatus.replaceChildren(text(message));
      researchResult.replaceChildren();
    }
    // Renders literal "[Source N]" markers inside synthesis text as
    // clickable controls resolved from the canonical citation projection
    // (payload.citations, sourced verbatim from the durable
    // Source-N -> EvidenceRef binding the Research domain already
    // persisted) — never from array position, never by otherwise parsing
    // synthesis prose. A marker whose N has no canonical binding — an
    // out-of-range or malformed citation, or any other synthesis text —
    // remains inert text like everything else here: every appended piece
    // uses the same createTextNode-based `text()` helper already used
    // throughout this file, never a raw-markup DOM-injection primitive,
    // so no synthesis content is ever interpreted as markup.
    const sourceCitationPattern = /\[Source (\d+)\]/g;
    function appendSynthesisWithCitations(container, summary, citationsBySourceNumber) {
      sourceCitationPattern.lastIndex = 0;
      let lastIndex = 0;
      let match;
      while ((match = sourceCitationPattern.exec(summary)) !== null) {
        if (match.index > lastIndex) container.appendChild(text(summary.slice(lastIndex, match.index)));
        const sourceNumber = Number(match[1]);
        const evidenceRef = sourceNumber > 0 ? citationsBySourceNumber.get(sourceNumber) : undefined;
        if (evidenceRef) {
          const citationButton = document.createElement('button');
          citationButton.type = 'button';
          citationButton.className = 'citation-link';
          citationButton.appendChild(text(match[0]));
          citationButton.addEventListener('click', () => goToEvidence(evidenceRef));
          container.appendChild(citationButton);
        } else {
          container.appendChild(text(match[0]));
        }
        lastIndex = sourceCitationPattern.lastIndex;
      }
      if (lastIndex < summary.length) container.appendChild(text(summary.slice(lastIndex)));
    }
    function renderResearch(payload) {
      researchStatus.className = '';
      researchStatus.replaceChildren(text(`Research ${researchStateLabel(payload.state)}`));
      const counts = payload.counts || {};
      const lines = [
        `ResearchId: ${payload.research_id}`,
        `Topic: ${payload.topic}`,
        `State: ${researchStateLabel(payload.state)}`,
        `Search results: ${counts.search_results ?? 0}`,
        `Acquisition attempts: ${counts.acquisition_attempts ?? 0}`,
        `Durable sources: ${counts.durable_sources ?? 0}`,
        `Observed acquisitions: ${counts.observed_acquisitions ?? 0}`,
        `Successful extractions: ${counts.successful_extractions ?? 0}`,
        `Created: ${payload.created_at_unix_ms}`
      ];
      if (payload.completed_at_unix_ms != null) lines.push(`Completed: ${payload.completed_at_unix_ms}`);
      const container = document.createDocumentFragment();
      container.appendChild(text(lines.join('\n')));
      if (payload.synthesis_summary) {
        const citationsBySourceNumber = new Map();
        for (const citation of payload.citations || []) {
          if (citation && Number.isInteger(citation.source_number) && citation.source_number > 0 && citation.evidence_ref) {
            citationsBySourceNumber.set(citation.source_number, citation.evidence_ref);
          }
        }
        container.appendChild(text('\n\nSynthesis:\n'));
        appendSynthesisWithCitations(container, payload.synthesis_summary, citationsBySourceNumber);
      }
      if (payload.evidence_ids?.length) container.appendChild(text(`\n\nEvidenceIds:\n${payload.evidence_ids.join('\n')}`));
      researchResult.replaceChildren(container);
    }
    async function pollResearch(id, generation) {
      if (generation !== researchGeneration) return;
      try {
        const response = await fetch(`/api/research/${encodeURIComponent(id)}`);
        const payload = await response.json();
        if (generation !== researchGeneration) return;
        if (!response.ok) { renderResearchError(payload?.error?.message || 'Research status is unavailable.'); researchButton.disabled = false; return; }
        renderResearch(payload);
        if (!terminalResearchStates.has(payload.state)) {
          researchTimer = window.setTimeout(() => pollResearch(id, generation), 1000);
        } else {
          researchButton.disabled = false;
        }
      } catch (_) {
        if (generation === researchGeneration) renderResearchError('Research status is temporarily unavailable.');
        researchButton.disabled = false;
      }
    }
    researchForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const topic = researchTopic.value.trim();
      if (!topic) { renderResearchError('Enter a research topic.'); return; }
      researchGeneration += 1;
      const generation = researchGeneration;
      if (researchTimer !== null) { window.clearTimeout(researchTimer); researchTimer = null; }
      researchButton.disabled = true;
      researchStatus.className = '';
      researchStatus.replaceChildren(text('Submitting research…'));
      researchResult.replaceChildren();
      try {
        const response = await fetch('/api/research', {
          method: 'POST', headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ topic })
        });
        const payload = await response.json();
        if (generation !== researchGeneration) return;
        if (!response.ok) { renderResearchError(payload?.error?.message || 'Research is unavailable.'); researchButton.disabled = false; return; }
        renderResearch({ research_id: payload.research_id, topic, state: payload.state, counts: {} });
        await pollResearch(payload.research_id, generation);
      } catch (_) {
        if (generation === researchGeneration) renderResearchError('Research is unavailable.');
        researchButton.disabled = false;
      }
    });

    // Evidence Inspector — fact inspection only. Every persisted value is
    // target-controlled and MUST be treated as inert text, never markup:
    // this section uses only the same createTextNode-based `text()`
    // helper already used above, never a raw-markup DOM-injection
    // primitive anywhere in this file. A stored script-tag payload or
    // image event-handler payload must render as literal, inert text —
    // see spider/tests/architecture_guardrails.rs. (Deliberately no
    // literal HTML tag syntax appears in this comment itself — see
    // web_console_inline_script_body_contains_no_script_tag_sentinels.)
    const evidenceForm = document.getElementById('evidence-form');
    const evidenceRefInput = document.getElementById('evidence-ref');
    const evidenceButton = document.getElementById('evidence-button');
    const evidenceStatus = document.getElementById('evidence-status');
    const evidenceResult = document.getElementById('evidence-result');
    function renderEvidenceError(message) {
      evidenceStatus.className = 'error';
      evidenceStatus.replaceChildren(text(message));
      evidenceResult.replaceChildren();
    }
    function evidenceLabel(label) {
      const h = document.createElement('h3');
      h.appendChild(text(label));
      return h;
    }
    function evidenceField(label, value) {
      const row = document.createElement('div');
      row.className = 'evidence-field';
      const term = document.createElement('strong');
      term.appendChild(text(label + ': '));
      row.append(term, text(value === null || value === undefined ? '(absent)' : String(value)));
      return row;
    }
    function evidencePre(contents) {
      const pre = document.createElement('pre');
      pre.appendChild(text(contents));
      return pre;
    }
    // Native, accessible disclosure — collapsed by default. Expanding
    // reveals exactly `contentNode`, already built from inert text
    // (`evidencePre`/`evidenceLabel`) — nothing is truncated, this only
    // defers rendering of large technical payloads until asked for.
    function evidenceDetails(summaryLabel, contentNode) {
      const details = document.createElement('details');
      const summary = document.createElement('summary');
      summary.appendChild(text(summaryLabel));
      details.append(summary, contentNode);
      return details;
    }
    // Only http/https targets are ever returned — javascript:, data:,
    // file:, ftp:, and every other scheme resolve to null and are never
    // made clickable. `URL` performs real scheme parsing rather than a
    // string-prefix check, so a hostile value like
    // "javascript:alert(1)//https://example.test" (a bare prefix check
    // could be fooled by trailing decoys) is rejected: `URL` reports its
    // true `javascript:` protocol, not `https:`.
    function safeHttpUrl(candidate) {
      if (!candidate) return null;
      try {
        const parsed = new URL(candidate);
        if (parsed.protocol === 'http:' || parsed.protocol === 'https:') return parsed.href;
      } catch (_) { /* not a parseable absolute URL */ }
      return null;
    }
    function renderEvidence(bundle) {
      evidenceStatus.className = '';
      evidenceStatus.replaceChildren(text('Evidence found.'));
      const container = document.createElement('div');
      container.append(
        evidenceField('Evidence ID', bundle.id),
        evidenceField('Requested URL', bundle.requested_url),
        evidenceField('Final URL', bundle.final_url),
        evidenceField('Retrieved at', bundle.retrieved_at),
        evidenceField('Effective status', bundle.status_code),
        evidenceField('Observed HTTP status', bundle.observed_status_code),
        evidenceField('Declared content type', bundle.content_type),
        evidenceField('Detected content type', bundle.detected_content_type),
        evidenceField('Transport', bundle.transport),
        evidenceField('DNS mode', bundle.dns),
        evidenceField('Backend provenance', bundle.backend_provenance),
        evidenceField('Response origin', bundle.response_origin),
        evidenceField('Response body hash', bundle.response_body_hash),
        evidenceField('Transformed content hash', bundle.transformed_content_hash),
      );

      // Actions: following any of these performs a new live action
      // (navigation or download) distinct from the immutable Evidence
      // record rendered above — none of them mutate or re-fetch Evidence.
      const actions = document.createElement('div');
      actions.className = 'evidence-actions';
      const liveSourceUrl = safeHttpUrl(bundle.final_url) ?? safeHttpUrl(bundle.requested_url);
      if (liveSourceUrl) {
        const openLiveSource = document.createElement('a');
        openLiveSource.href = liveSourceUrl;
        openLiveSource.target = '_blank';
        openLiveSource.rel = 'noopener noreferrer';
        openLiveSource.appendChild(text('Open live source'));
        actions.appendChild(openLiveSource);
      }
      if (bundle.id) {
        const downloadContent = document.createElement('a');
        downloadContent.href = `/api/evidence/${encodeURIComponent(bundle.id)}/content`;
        downloadContent.appendChild(text('Download captured content'));
        actions.appendChild(downloadContent);
        const downloadJson = document.createElement('a');
        downloadJson.href = `/api/evidence/${encodeURIComponent(bundle.id)}/export`;
        downloadJson.appendChild(text('Download canonical JSON'));
        actions.appendChild(downloadJson);
      }
      if (actions.childNodes.length) container.appendChild(actions);

      if (bundle.response_headers) {
        container.append(evidenceDetails('Response headers', evidencePre(JSON.stringify(bundle.response_headers, null, 2))));
      }
      if (bundle.links) {
        container.append(evidenceLabel('Links'), evidencePre(bundle.links.join('\n')));
      }
      if (bundle.content !== null && bundle.content !== undefined) {
        container.append(evidenceDetails('Captured content (inert text — never executed)', evidencePre(bundle.content)));
      }
      if (bundle.metadata) {
        container.append(evidenceLabel('Metadata'), evidencePre(JSON.stringify(bundle.metadata, null, 2)));
      }
      if (bundle.screenshot) {
        container.append(
          evidenceField('Screenshot hash', bundle.screenshot_hash),
          evidenceLabel('Screenshot (raw base64 — no rendered preview in this frontier)'),
          evidencePre(bundle.screenshot),
        );
      }
      container.append(evidenceDetails('Raw canonical evidence (complete stored value)', evidencePre(JSON.stringify(bundle, null, 2))));
      evidenceResult.replaceChildren(container);
    }
    // Shared by the Evidence Inspector's own form submit below and by
    // Page Audit's "Inspect evidence" action (Phase 14: the audit result
    // populates and triggers this existing flow — it never renders a
    // second evidence view of its own).
    async function inspectEvidenceRef(ref) {
      if (!ref) { renderEvidenceError('Enter an evidence reference.'); return; }
      evidenceButton.disabled = true;
      evidenceStatus.className = '';
      evidenceStatus.replaceChildren(text('Inspecting evidence…'));
      evidenceResult.replaceChildren();
      try {
        const response = await fetch(`/api/evidence/${encodeURIComponent(ref)}`);
        const payload = await response.json();
        if (!response.ok) {
          renderEvidenceError(payload?.error?.message || 'Evidence is unavailable.');
          return;
        }
        renderEvidence(payload);
      } catch (_) {
        renderEvidenceError('Evidence inspection is temporarily unavailable.');
      } finally {
        evidenceButton.disabled = false;
      }
    }
    // Shared navigation target for anything that resolves to a canonical
    // EvidenceRef and wants the Evidence Inspector to show it — Research
    // synthesis citations and Page Audit's own "Inspect evidence" action
    // both funnel through this one function, so there is exactly one
    // place that populates the input, triggers the canonical read, and
    // scrolls the Evidence Inspector into view.
    function goToEvidence(evidenceRef) {
      evidenceRefInput.value = evidenceRef;
      inspectEvidenceRef(evidenceRef);
      const heading = document.getElementById('evidence-heading');
      if (heading) heading.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
    evidenceForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      await inspectEvidenceRef(evidenceRefInput.value.trim());
    });

    // Page Audit — runs the canonical deterministic page audit. Findings
    // and technology markers are deterministic rule evaluations/direct
    // observations only: no score, grade, summary, or interpretation is
    // ever rendered here. Every target-controlled value (URL, Finding
    // target/observed/expected conditions, technology marker values,
    // EvidenceRef, errors) is inserted as inert text via the same
    // createTextNode-based text() helper used throughout this file —
    // never a raw-markup DOM-injection primitive.
    const auditForm = document.getElementById('audit-form');
    const auditUrlInput = document.getElementById('audit-url');
    const auditButton = document.getElementById('audit-button');
    const auditStatus = document.getElementById('audit-status');
    const auditResult = document.getElementById('audit-result');
    function renderAuditError(message) {
      auditStatus.className = 'error';
      auditStatus.replaceChildren(text(message));
      auditResult.replaceChildren();
    }
    function auditLabel(label) {
      const h = document.createElement('h4');
      h.appendChild(text(label));
      return h;
    }
    function auditField(label, value) {
      const row = document.createElement('div');
      row.className = 'audit-field';
      const term = document.createElement('strong');
      term.appendChild(text(label + ': '));
      row.append(term, text(value === null || value === undefined ? '(absent)' : String(value)));
      return row;
    }
    function auditPre(contents) {
      const pre = document.createElement('pre');
      pre.appendChild(text(contents));
      return pre;
    }
    function renderFinding(finding) {
      const item = document.createElement('li');
      item.className = 'audit-finding';
      item.append(
        auditField('Rule ID', finding.rule_id),
        auditField('Rule version', finding.rule_version),
        auditField('Category', finding.category),
        auditField('Severity', finding.severity),
        auditField('Target', finding.target),
        auditField('Observed condition', JSON.stringify(finding.observed_condition)),
        auditField('Expected condition', JSON.stringify(finding.expected_condition)),
        auditField('Evidence references', (finding.evidence || []).map((e) => e.id).join(', ')),
      );
      return item;
    }
    function renderTechnologyMarker(marker) {
      const item = document.createElement('li');
      item.append(
        auditField('Source', JSON.stringify(marker.source)),
        auditField('Value', marker.value),
      );
      return item;
    }
    function renderAuditResult(payload) {
      auditStatus.className = '';
      const container = document.createElement('div');

      // The canonical engine's own execution outcome, projected verbatim
      // on the wire (`outcome`), is the only signal used here — the
      // console never re-derives observed/unobserved from status codes,
      // evidence fields, or findings counts.
      const unobserved = payload.outcome === 'target_unobserved';
      if (unobserved) {
        auditStatus.replaceChildren(
          text('Target was not observed. No audit findings were evaluated.'));
      } else {
        auditStatus.replaceChildren(text('Audit complete.'));
      }

      const refRow = document.createElement('div');
      refRow.className = 'audit-field';
      const refTerm = document.createElement('strong');
      refTerm.appendChild(text('Evidence reference: '));
      const inspectButton = document.createElement('button');
      inspectButton.type = 'button';
      inspectButton.appendChild(text('Inspect evidence'));
      inspectButton.addEventListener('click', () => goToEvidence(payload.evidence_ref));
      refRow.append(refTerm, text(payload.evidence_ref), text(' '), inspectButton);
      container.append(refRow);

      if (unobserved) {
        // No Findings/markers sections: zero findings here means no rule
        // ran — never render it as "Audit complete. Findings (0)".
        container.append(auditLabel('Raw canonical audit result (complete)'), auditPre(JSON.stringify(payload, null, 2)));
        auditResult.replaceChildren(container);
        return;
      }

      container.append(auditLabel(`Findings (${payload.findings.length})`));
      const findingsList = document.createElement('ol');
      for (const finding of payload.findings) {
        findingsList.appendChild(renderFinding(finding));
      }
      container.append(findingsList);

      container.append(auditLabel(`Technology markers (${payload.technology_markers.length})`));
      const markerList = document.createElement('ul');
      for (const marker of payload.technology_markers) {
        markerList.appendChild(renderTechnologyMarker(marker));
      }
      container.append(markerList);

      container.append(auditLabel('Raw canonical audit result (complete)'), auditPre(JSON.stringify(payload, null, 2)));
      auditResult.replaceChildren(container);
    }
    auditForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const url = auditUrlInput.value.trim();
      if (!url) { renderAuditError('Enter a URL to audit.'); return; }
      auditButton.disabled = true;
      auditStatus.className = '';
      auditStatus.replaceChildren(text('Running audit…'));
      auditResult.replaceChildren();
      try {
        const response = await fetch('/api/audit', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ url }),
        });
        const payload = await response.json();
        if (!response.ok) {
          renderAuditError(payload?.error?.message || 'Audit is unavailable.');
          return;
        }
        renderAuditResult(payload);
      } catch (_) {
        renderAuditError('Audit is temporarily unavailable.');
      } finally {
        auditButton.disabled = false;
      }
    });

    // Fetch — runs the canonical one-shot Fetch capability: one caller-
    // supplied URL, no crawl, no browser, no link following. The response
    // carries only the recorded evidence's own fields, verbatim, plus its
    // EvidenceRef — the complete record (captured content, response
    // headers, downloads) is reached through the existing Evidence
    // Inspector via the same shared `goToEvidence` seam Page Audit already
    // uses, never re-rendered independently here.
    const fetchForm = document.getElementById('fetch-form');
    const fetchUrlInput = document.getElementById('fetch-url');
    const fetchButton = document.getElementById('fetch-button');
    const fetchStatus = document.getElementById('fetch-status');
    const fetchResult = document.getElementById('fetch-result');
    function renderFetchError(message) {
      fetchStatus.className = 'error';
      fetchStatus.replaceChildren(text(message));
      fetchResult.replaceChildren();
    }
    function fetchField(label, value) {
      const row = document.createElement('div');
      row.className = 'audit-field';
      const term = document.createElement('strong');
      term.appendChild(text(label + ': '));
      row.append(term, text(value === null || value === undefined ? '(absent)' : String(value)));
      return row;
    }
    function renderFetchResult(payload) {
      fetchStatus.className = '';
      fetchStatus.replaceChildren(text('Fetch complete.'));
      const container = document.createElement('div');
      container.append(
        fetchField('Requested URL', payload.requested_url),
        fetchField('Final URL', payload.final_url),
        fetchField('Status', payload.status_code),
        fetchField('Observed HTTP status', payload.observed_status_code),
        fetchField('Content type', payload.content_type),
      );

      const refRow = document.createElement('div');
      refRow.className = 'audit-field';
      const refTerm = document.createElement('strong');
      refTerm.appendChild(text('Evidence reference: '));
      const inspectButton = document.createElement('button');
      inspectButton.type = 'button';
      inspectButton.appendChild(text('Inspect evidence'));
      inspectButton.addEventListener('click', () => goToEvidence(payload.evidence_ref));
      refRow.append(refTerm, text(payload.evidence_ref), text(' '), inspectButton);
      container.append(refRow);

      fetchResult.replaceChildren(container);
    }
    fetchForm.addEventListener('submit', async (event) => {
      event.preventDefault();
      const url = fetchUrlInput.value.trim();
      if (!url) { renderFetchError('Enter a URL to fetch.'); return; }
      fetchButton.disabled = true;
      fetchStatus.className = '';
      fetchStatus.replaceChildren(text('Fetching…'));
      fetchResult.replaceChildren();
      try {
        const response = await fetch('/api/fetch', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ url }),
        });
        const payload = await response.json();
        if (!response.ok) {
          renderFetchError(payload?.error?.message || 'Fetch is unavailable.');
          return;
        }
        renderFetchResult(payload);
      } catch (_) {
        renderFetchError('Fetch is temporarily unavailable.');
      } finally {
        fetchButton.disabled = false;
      }
    });
  </script>
</body>
</html>"#;
