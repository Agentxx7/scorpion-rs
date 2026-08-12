use std::{fs, path::PathBuf};

fn engine_source() -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/automation/engine.rs"))
        .unwrap()
}

fn browser_source() -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/automation/browser.rs"))
        .unwrap()
}

fn proxy_resolution_violations(source: &str) -> Vec<&'static str> {
    let mut violations = Vec::new();
    if source.contains("if let Ok(proxy) = reqwest::Proxy::all") {
        violations.push("ignored proxy parse error");
    }
    if source.contains("builder.build().ok()") {
        violations.push("ignored proxy client build error");
    }
    if source.contains("with_proxies(cfgs.proxies.as_deref());") {
        violations.push("unpropagated proxy resolution result");
    }
    violations
}

#[test]
fn proxy_resolution_is_fallible_and_propagated_before_execution() {
    let engine = engine_source();
    let browser = browser_source();
    assert!(engine.contains("pub fn with_proxies(") && engine.contains("EngineResult<&mut Self>"));
    assert!(browser.contains("engine.with_proxies(cfgs.proxies.as_deref())?;"));
    assert!(proxy_resolution_violations(&(engine + &browser)).is_empty());
}

#[test]
fn guardrail_detects_every_silent_direct_fallback_shape() {
    let synthetic = r#"
        if let Ok(proxy) = reqwest::Proxy::all(url) { builder = builder.proxy(proxy); }
        let client = builder.build().ok();
        engine.with_proxies(cfgs.proxies.as_deref());
    "#;
    assert_eq!(proxy_resolution_violations(synthetic).len(), 3);
}

#[test]
fn unrelated_execution_policies_are_unchanged_structurally() {
    let engine = engine_source();
    assert!(engine.contains("self.use_chrome_ai || (self.api_url.is_empty()"));
    assert!(engine.contains("pick_fallback_model"));
    assert!(engine.contains("self.client.as_ref().unwrap_or(&CLIENT)"));
}
