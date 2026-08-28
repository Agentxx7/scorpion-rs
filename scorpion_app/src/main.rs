use scorpion_app::{
    error_json, error_status, research_error_json, research_error_status, search, ResearchError,
    ResearchRequest, ResearchService, SearchError, SearchRequest,
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
    let content_length = lines
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
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
        return write_html(&mut stream, INDEX_HTML)
            .await
            .map_err(Into::into);
    }
    if method == "GET" && path == "/health" {
        return write_json(&mut stream, 200, r#"{"status":"ok"}"#)
            .await
            .map_err(Into::into);
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
    @media (prefers-color-scheme: dark) { body { background: #101418; color: #e6edf3; } input { background: #1b222c; color: #e6edf3; border-color: #536170; } }
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
      <form id="research-form">
        <label for="research-topic" hidden>Research topic</label>
        <input id="research-topic" name="topic" type="text" placeholder="Research a topic" autocomplete="off" required>
        <button id="research-button" type="submit">Start Research</button>
      </form>
      <div id="research-status" role="status" aria-live="polite"></div>
      <div id="research-result"></div>
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
          if (result.score != null) { if (result.date) meta.appendChild(text(' · ')); meta.appendChild(text(`score ${result.score}`)); }
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
      if (payload.synthesis_summary) lines.push(`\nSynthesis:\n${payload.synthesis_summary}`);
      if (payload.evidence_ids?.length) lines.push(`\nEvidenceIds:\n${payload.evidence_ids.join('\n')}`);
      researchResult.replaceChildren(text(lines.join('\n')));
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
  </script>
</body>
</html>"#;
