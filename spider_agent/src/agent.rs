//! Core Agent struct and builder for spider_agent.

use crate::config::{AgentConfig, UsageSnapshot, UsageStats};
#[cfg(feature = "search")]
use crate::config::{ResearchOptions, SearchOptions};
use crate::error::{AgentError, AgentResult};
#[cfg(feature = "search")]
use crate::llm::TokenUsage;
use crate::llm::{CompletionOptions, CompletionResponse, LLMProvider, Message};
use crate::memory::AgentMemory;
use crate::tools::{
    CustomTool, CustomToolRegistry, CustomToolResult, SpiderBrowserToolConfig,
    SpiderCloudToolConfig,
};
use std::sync::Arc;

/// Neutral page acquisition contract used by [`Agent::research`].
///
/// Implementations own acquisition. Spider-specific page, evidence, and
/// transport types deliberately do not cross this boundary.
#[async_trait::async_trait]
pub trait PageAcquirer: Send + Sync {
    /// Acquire one URL for research without performing extraction.
    async fn acquire(&self, url: &str) -> AgentResult<AcquiredSource>;
}

/// Page content and admission metadata supplied to the research loop.
#[derive(Debug, Clone)]
pub struct AcquiredSource {
    /// URL requested by the search result.
    pub requested_url: String,
    /// Final URL after redirects.
    pub final_url: String,
    /// Effective HTTP status presented by the acquirer.
    pub status: u16,
    /// Declared response content type, or an empty string when absent.
    pub content_type: String,
    /// Acquired textual content.
    pub content: String,
    /// Opaque acquisition identity supplied by the acquirer.
    pub acquisition_id: Option<String>,
}

/// Check if an API key is a placeholder or empty.
fn is_placeholder_api_key(key: &str) -> bool {
    let trimmed = key.trim();
    trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("YOUR_API_KEY")
        || trimmed.eq_ignore_ascii_case("YOUR-API-KEY")
        || trimmed.eq_ignore_ascii_case("API_KEY")
        || trimmed.eq_ignore_ascii_case("API-KEY")
}
use tokio::sync::Semaphore;

#[cfg(feature = "search")]
use crate::search::{SearchProvider, SearchResults};

#[cfg(feature = "chrome")]
use crate::browser::BrowserContext;

#[cfg(feature = "webdriver")]
use crate::webdriver::WebDriverContext;

#[cfg(feature = "fs")]
use crate::temp::TempStorage;

/// Multimodal agent for web automation and research.
///
/// Designed to be wrapped in `Arc` for concurrent access.
///
/// # Example
/// ```ignore
/// use spider_agent::{Agent, AgentConfig};
/// use std::sync::Arc;
///
/// let agent = Arc::new(Agent::builder()
///     .with_openai("sk-...", "gpt-4o")
///     .with_search_serper("serper-key")
///     .build()?);
///
/// // Spawn concurrent tasks
/// let agent_clone = agent.clone();
/// tokio::spawn(async move {
///     agent_clone.search("rust web frameworks").await
/// });
/// ```
pub struct Agent {
    /// LLM provider for inference.
    llm: Option<Box<dyn LLMProvider>>,

    /// HTTP client for requests.
    client: reqwest::Client,

    /// Optional injected acquisition authority for research pages.
    page_acquirer: Option<Box<dyn PageAcquirer>>,

    /// Search provider (if configured).
    #[cfg(feature = "search")]
    search_provider: Option<Box<dyn SearchProvider>>,

    /// Browser context for Chrome automation.
    #[cfg(feature = "chrome")]
    browser: Option<BrowserContext>,

    /// WebDriver context for browser automation.
    #[cfg(feature = "webdriver")]
    webdriver: Option<WebDriverContext>,

    /// Temporary storage for large operations.
    #[cfg(feature = "fs")]
    temp_storage: Option<TempStorage>,

    /// Session memory (lock-free via DashMap).
    memory: AgentMemory,

    /// Concurrency semaphore for LLM calls.
    llm_semaphore: Arc<Semaphore>,

    /// Configuration.
    config: AgentConfig,

    /// Usage statistics (atomic counters for lock-free updates).
    usage: Arc<UsageStats>,

    /// Custom tool registry.
    custom_tools: CustomToolRegistry,
}

impl Agent {
    /// Create a new agent builder.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Returns a reference to the underlying HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    // ==================== Search Methods ====================

    /// Search the web and return results.
    #[cfg(feature = "search")]
    pub async fn search(&self, query: &str) -> AgentResult<SearchResults> {
        self.search_with_options(query, SearchOptions::default())
            .await
    }

    /// Search with custom options.
    #[cfg(feature = "search")]
    pub async fn search_with_options(
        &self,
        query: &str,
        options: SearchOptions,
    ) -> AgentResult<SearchResults> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_search_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let provider = self
            .search_provider
            .as_ref()
            .ok_or(AgentError::NotConfigured("search provider"))?;

        self.usage.increment_search_calls();

        provider
            .search(query, &options)
            .await
            .map_err(AgentError::Search)
    }

    // ==================== LLM Methods ====================

    /// Send a prompt to the LLM and get a response.
    pub async fn prompt(&self, messages: Vec<Message>) -> AgentResult<String> {
        let response = self.complete(messages).await?;
        Ok(response.content)
    }

    /// Send a completion request with full options.
    pub async fn complete(&self, messages: Vec<Message>) -> AgentResult<CompletionResponse> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_llm_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }
        if let Some(limit) = self.usage.check_token_limits(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let llm = self
            .llm
            .as_ref()
            .ok_or(AgentError::NotConfigured("LLM provider"))?;

        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AgentError::Llm("Failed to acquire semaphore".to_string()))?;

        let options = CompletionOptions {
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            json_mode: self.config.json_mode,
        };

        self.usage.increment_llm_calls();

        let response = llm.complete(messages, &options, &self.client).await?;

        // Track token usage
        self.usage.add_tokens(
            response.usage.prompt_tokens as u64,
            response.usage.completion_tokens as u64,
        );

        Ok(response)
    }

    // ==================== Extraction Methods ====================

    /// Extract structured data from HTML using the LLM.
    pub async fn extract(&self, html: &str, prompt: &str) -> AgentResult<serde_json::Value> {
        let cleaned_html = self.clean_html(html);
        let truncated = self.truncate_html(&cleaned_html);

        let messages = vec![
            Message::system(
                "You are a data extraction assistant. Extract the requested information from the HTML and return it as JSON.",
            ),
            Message::user(format!(
                "Extract the following from this HTML:\n\n{}\n\nHTML:\n{}",
                prompt, truncated
            )),
        ];

        let response = self.complete(messages).await?;
        self.parse_json(&response.content)
    }

    /// Extract data with a JSON schema for structured output.
    pub async fn extract_structured(
        &self,
        html: &str,
        schema: &serde_json::Value,
    ) -> AgentResult<serde_json::Value> {
        let cleaned_html = self.clean_html(html);
        let truncated = self.truncate_html(&cleaned_html);

        let messages = vec![
            Message::system(
                "You are a data extraction assistant. Extract data matching the provided JSON schema.",
            ),
            Message::user(format!(
                "Extract data matching this schema:\n{}\n\nFrom this HTML:\n{}",
                serde_json::to_string_pretty(schema).unwrap_or_default(),
                truncated
            )),
        ];

        let response = self.complete(messages).await?;
        self.parse_json(&response.content)
    }

    // ==================== HTTP Methods ====================

    /// Fetch a URL and return the HTML content.
    pub async fn fetch(&self, url: &str) -> AgentResult<FetchResult> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_fetch_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        self.usage.increment_fetch_calls();

        let response = self.client.get(url).send().await?;

        let status = response.status();
        let headers = response.headers().clone();

        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let html = response.text().await?;

        Ok(FetchResult {
            url: url.to_string(),
            status: status.as_u16(),
            content_type,
            html,
        })
    }

    // ==================== Research Methods ====================

    /// Research a topic using search and extraction.
    #[cfg(feature = "search")]
    pub async fn research(
        &self,
        topic: &str,
        options: ResearchOptions,
    ) -> AgentResult<ResearchResult> {
        // Search for the topic
        let search_opts = options
            .search_options
            .clone()
            .unwrap_or_else(|| SearchOptions::new().with_limit(options.max_pages.max(5)));

        let search_results = self.search_with_options(topic, search_opts).await?;

        if search_results.is_empty() {
            return Ok(ResearchResult {
                topic: topic.to_string(),
                search_results,
                extractions: Vec::new(),
                summary: None,
                usage: TokenUsage::default(),
            });
        }

        // Extract from each result
        let extraction_prompt = options.extraction_prompt.clone().unwrap_or_else(|| {
            format!(
                "Extract key information relevant to: {}. Include facts, data points, and insights.",
                topic
            )
        });

        let mut extractions = Vec::new();
        let mut total_usage = TokenUsage::default();

        let max_pages = options.max_pages.min(search_results.results.len());

        #[derive(Clone, Copy)]
        enum AcquisitionMode<'a> {
            Injected(&'a dyn PageAcquirer),
            Compatibility,
        }

        let acquisition_mode = match self.page_acquirer.as_deref() {
            Some(acquirer) => AcquisitionMode::Injected(acquirer),
            None => AcquisitionMode::Compatibility,
        };

        for result in search_results.results.iter().take(max_pages) {
            let acquisition = match acquisition_mode {
                AcquisitionMode::Injected(acquirer) => {
                    self.usage.increment_fetch_calls();
                    acquirer.acquire(&result.url).await
                }
                AcquisitionMode::Compatibility => self.fetch(&result.url).await.map(Into::into),
            };

            match acquisition {
                Ok(source) => {
                    if !(200..300).contains(&source.status) {
                        log::warn!(
                            "Skipping {}: HTTP status {} is not successful",
                            result.url,
                            source.status
                        );
                        continue;
                    }
                    if !is_supported_research_content_type(&source.content_type) {
                        log::warn!(
                            "Skipping {}: unsupported content type {:?}",
                            result.url,
                            source.content_type
                        );
                        continue;
                    }
                    if is_obvious_block_document(&source.content) {
                        log::warn!(
                            "Skipping {}: response appears to be a block or challenge document",
                            result.url
                        );
                        continue;
                    }

                    // Extract
                    match self.extract(&source.content, &extraction_prompt).await {
                        Ok(extracted) => {
                            extractions.push(PageExtraction {
                                url: source.final_url,
                                title: result.title.clone(),
                                extracted,
                                acquisition_id: source.acquisition_id,
                            });
                        }
                        Err(e) => {
                            log::warn!("Extraction failed for {}: {}", result.url, e);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Fetch failed for {}: {}", result.url, e);
                }
            }
        }

        // Synthesize if requested
        let summary = if options.synthesize && !extractions.is_empty() {
            match self.synthesize_research(topic, &extractions).await {
                Ok((summary, usage)) => {
                    total_usage.accumulate(&usage);
                    Some(summary)
                }
                Err(e) => {
                    log::warn!("Synthesis failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(ResearchResult {
            topic: topic.to_string(),
            search_results,
            extractions,
            summary,
            usage: total_usage,
        })
    }

    /// Synthesize research findings into a summary.
    #[cfg(feature = "search")]
    async fn synthesize_research(
        &self,
        topic: &str,
        extractions: &[PageExtraction],
    ) -> AgentResult<(String, TokenUsage)> {
        let mut context = String::new();
        for (i, extraction) in extractions.iter().enumerate() {
            context.push_str(&format!(
                "\n\nSource {} ({}): {}\n{}",
                i + 1,
                extraction.url,
                extraction.title,
                serde_json::to_string_pretty(&extraction.extracted).unwrap_or_default()
            ));
        }

        let messages = vec![
            Message::system(
                "You are a research synthesis assistant. Summarize the findings from multiple sources into a coherent response.",
            ),
            Message::user(format!(
                "Topic: {}\n\nSources:{}\n\nProvide a comprehensive summary of the findings, citing sources where appropriate. Return as JSON with a 'summary' field.",
                topic, context
            )),
        ];

        let response = self.complete(messages).await?;
        let json = self.parse_json(&response.content)?;
        let summary = json
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or(&response.content)
            .to_string();

        Ok((summary, response.usage))
    }

    // ==================== Memory Methods ====================

    /// Get a value from memory (lock-free).
    pub fn memory_get(&self, key: &str) -> Option<serde_json::Value> {
        self.memory.get(key)
    }

    /// Set a value in memory (lock-free).
    pub fn memory_set(&self, key: &str, value: serde_json::Value) {
        self.memory.set(key, value);
    }

    /// Clear all memory (lock-free).
    pub fn memory_clear(&self) {
        self.memory.clear();
    }

    /// Get the memory instance for direct access.
    pub fn memory(&self) -> &AgentMemory {
        &self.memory
    }

    // ==================== Usage Methods ====================

    /// Get a snapshot of usage statistics.
    pub fn usage(&self) -> UsageSnapshot {
        self.usage.snapshot()
    }

    /// Get the raw usage stats for direct access.
    pub fn usage_stats(&self) -> &Arc<UsageStats> {
        &self.usage
    }

    /// Reset usage statistics.
    pub fn reset_usage(&self) {
        self.usage.reset();
    }

    // ==================== Custom Tool Methods ====================

    /// Register a custom tool.
    pub fn register_custom_tool(&self, tool: CustomTool) {
        self.custom_tools.register(tool);
    }

    /// Remove a custom tool.
    pub fn remove_custom_tool(&self, name: &str) -> bool {
        self.custom_tools.remove(name).is_some()
    }

    /// List all registered custom tools.
    pub fn list_custom_tools(&self) -> Vec<String> {
        self.custom_tools.list()
    }

    /// Check if a custom tool is registered.
    pub fn has_custom_tool(&self, name: &str) -> bool {
        self.custom_tools.contains(name)
    }

    /// Execute a custom tool by name.
    ///
    /// # Arguments
    /// * `name` - The registered tool name
    /// * `path` - Optional path to append to the base URL
    /// * `query` - Optional query parameters
    /// * `body` - Optional request body
    pub async fn execute_custom_tool(
        &self,
        name: &str,
        path: Option<&str>,
        query: Option<&[(&str, &str)]>,
        body: Option<&str>,
    ) -> AgentResult<CustomToolResult> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_custom_tool_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        // Track the call
        self.usage.increment_custom_tool_calls(name);

        // Execute the tool
        self.custom_tools
            .execute(name, &self.client, path, query, body)
            .await
    }

    /// Execute a custom tool and parse the JSON response.
    pub async fn execute_custom_tool_json(
        &self,
        name: &str,
        path: Option<&str>,
        query: Option<&[(&str, &str)]>,
        body: Option<&str>,
    ) -> AgentResult<serde_json::Value> {
        let result = self.execute_custom_tool(name, path, query, body).await?;
        serde_json::from_str(&result.body).map_err(AgentError::Json)
    }

    /// Get the custom tool registry for direct access.
    pub fn custom_tool_registry(&self) -> &CustomToolRegistry {
        &self.custom_tools
    }

    /// Register Spider Cloud routes as custom tools.
    ///
    /// Core routes (`/crawl`, `/scrape`, `/search`, `/links`, `/transform`,
    /// `/unblocker`) are enabled by default. AI routes are gated and disabled
    /// by default.
    ///
    /// Returns the number of tools registered.
    pub fn register_spider_cloud(&self, config: SpiderCloudToolConfig) -> usize {
        self.custom_tools.register_spider_cloud(&config)
    }

    /// Register Spider Browser Cloud tools.
    ///
    /// Returns the number of tools registered.
    pub fn register_spider_browser(&self, config: SpiderBrowserToolConfig) -> usize {
        self.custom_tools.register_spider_browser(&config)
    }

    // ==================== Browser Methods ====================

    /// Get the browser context if configured.
    #[cfg(feature = "chrome")]
    pub fn browser(&self) -> Option<&BrowserContext> {
        self.browser.as_ref()
    }

    /// Navigate to a URL using the browser.
    #[cfg(feature = "chrome")]
    pub async fn navigate(&self, url: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .navigate(url)
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Get HTML from the current browser page.
    #[cfg(feature = "chrome")]
    pub async fn browser_html(&self) -> AgentResult<String> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .html()
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Take a screenshot of the current browser page.
    #[cfg(feature = "chrome")]
    pub async fn screenshot(&self) -> AgentResult<Vec<u8>> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .screenshot()
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Open a new page/tab in the browser.
    ///
    /// The returned [`BrowserContext`] **owns** the new tab: the CDP target is
    /// closed once the last clone of that context drops. Code that keeps only
    /// `ctx.page().clone()` and drops the context will find its tab closed —
    /// call [`BrowserContext::defuse_page`] to let the page outlive the context.
    #[cfg(feature = "chrome")]
    pub async fn new_page(&self) -> AgentResult<crate::browser::BrowserContext> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .clone_page()
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Open a new page and navigate to URL.
    ///
    /// The bare page handle carries no ownership, so the tab is only released
    /// when the agent's browser context drops. Prefer
    /// [`Agent::new_page_with_url_owned`], which scopes the tab to the returned
    /// context.
    #[cfg(feature = "chrome")]
    #[deprecated(
        since = "2.52.14",
        note = "leaks the CDP tab until the agent's browser context drops; use new_page_with_url_owned()"
    )]
    pub async fn new_page_with_url(
        &self,
        url: &str,
    ) -> AgentResult<std::sync::Arc<crate::browser::Page>> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        #[allow(deprecated)]
        browser
            .new_page_with_url(url)
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Open a new page at `url` and return it as an owning context.
    ///
    /// The tab is closed when the last clone of the returned
    /// [`BrowserContext`] drops. Leak-free replacement for
    /// [`Agent::new_page_with_url`].
    #[cfg(feature = "chrome")]
    pub async fn new_page_with_url_owned(
        &self,
        url: &str,
    ) -> AgentResult<crate::browser::BrowserContext> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .new_page_with_url_owned(url)
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Click an element in the browser.
    #[cfg(feature = "chrome")]
    pub async fn click(&self, selector: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .click(selector)
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Type text into an element in the browser.
    #[cfg(feature = "chrome")]
    pub async fn type_text(&self, selector: &str, text: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let browser = self
            .browser
            .as_ref()
            .ok_or(AgentError::NotConfigured("browser"))?;

        self.usage.increment_webbrowser_calls();

        browser
            .type_text(selector, text)
            .await
            .map_err(|e| AgentError::Browser(e.to_string()))
    }

    /// Extract from the current browser page using the LLM.
    #[cfg(feature = "chrome")]
    pub async fn extract_page(&self, prompt: &str) -> AgentResult<serde_json::Value> {
        let html = self.browser_html().await?;
        self.extract(&html, prompt).await
    }

    // ==================== WebDriver Methods ====================

    /// Get the WebDriver context if configured.
    #[cfg(feature = "webdriver")]
    pub fn webdriver(&self) -> Option<&WebDriverContext> {
        self.webdriver.as_ref()
    }

    /// Navigate using WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_navigate(&self, url: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .navigate(url)
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    /// Get HTML from WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_html(&self) -> AgentResult<String> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .html()
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    /// Take a screenshot using WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_screenshot(&self) -> AgentResult<Vec<u8>> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .screenshot()
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    /// Click an element using WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_click(&self, selector: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .click(selector)
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    /// Type text into an element using WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_type_text(&self, selector: &str, text: &str) -> AgentResult<()> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .type_text(selector, text)
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    /// Extract from the current WebDriver page using the LLM.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_extract_page(&self, prompt: &str) -> AgentResult<serde_json::Value> {
        // Note: webdriver_html already tracks the webbrowser call
        let html = self.webdriver_html().await?;
        self.extract(&html, prompt).await
    }

    /// Open a new tab using WebDriver.
    #[cfg(feature = "webdriver")]
    pub async fn webdriver_new_tab(&self) -> AgentResult<crate::webdriver::WindowHandle> {
        // Check limits before proceeding
        if let Some(limit) = self.usage.check_webbrowser_limit(&self.config.limits) {
            return Err(AgentError::LimitExceeded(limit));
        }

        let driver = self
            .webdriver
            .as_ref()
            .ok_or(AgentError::NotConfigured("webdriver"))?;

        self.usage.increment_webbrowser_calls();

        driver
            .new_tab()
            .await
            .map_err(|e| AgentError::WebDriver(e.to_string()))
    }

    // ==================== Temp Storage Methods ====================

    /// Get the temp storage if configured.
    #[cfg(feature = "fs")]
    pub fn temp_storage(&self) -> Option<&TempStorage> {
        self.temp_storage.as_ref()
    }

    /// Store data in temp storage.
    #[cfg(feature = "fs")]
    pub fn store_temp(&self, name: &str, data: &[u8]) -> AgentResult<std::path::PathBuf> {
        let storage = self
            .temp_storage
            .as_ref()
            .ok_or(AgentError::NotConfigured("temp storage"))?;
        storage.store_bytes(name, data).map_err(AgentError::Io)
    }

    /// Store JSON in temp storage.
    #[cfg(feature = "fs")]
    pub fn store_temp_json(
        &self,
        name: &str,
        data: &serde_json::Value,
    ) -> AgentResult<std::path::PathBuf> {
        let storage = self
            .temp_storage
            .as_ref()
            .ok_or(AgentError::NotConfigured("temp storage"))?;
        storage.store_json(name, data).map_err(AgentError::Io)
    }

    /// Read data from temp storage.
    #[cfg(feature = "fs")]
    pub fn read_temp(&self, name: &str) -> AgentResult<Vec<u8>> {
        let storage = self
            .temp_storage
            .as_ref()
            .ok_or(AgentError::NotConfigured("temp storage"))?;
        storage.read_bytes(name).map_err(AgentError::Io)
    }

    /// Read JSON from temp storage.
    #[cfg(feature = "fs")]
    pub fn read_temp_json(&self, name: &str) -> AgentResult<serde_json::Value> {
        let storage = self
            .temp_storage
            .as_ref()
            .ok_or(AgentError::NotConfigured("temp storage"))?;
        storage.read_json(name).map_err(AgentError::Io)
    }

    // ==================== Helper Methods ====================

    /// Clean HTML by removing scripts, styles, etc.
    fn clean_html(&self, html: &str) -> String {
        use crate::config::HtmlCleaningMode;

        match self.config.html_cleaning {
            HtmlCleaningMode::Raw => html.to_string(),
            HtmlCleaningMode::Minimal => {
                // Remove scripts only
                remove_tags(html, &["script"])
            }
            HtmlCleaningMode::Default => {
                // Remove scripts, styles, comments
                remove_tags(html, &["script", "style", "noscript"])
            }
            HtmlCleaningMode::Aggressive => {
                // Remove more elements
                remove_tags(
                    html,
                    &[
                        "script", "style", "noscript", "svg", "canvas", "video", "audio", "iframe",
                    ],
                )
            }
        }
    }

    /// Truncate HTML to max bytes.
    fn truncate_html<'a>(&self, html: &'a str) -> &'a str {
        if html.len() <= self.config.html_max_bytes {
            html
        } else {
            // Walk back to the nearest UTF-8 char boundary so slicing a page
            // whose multibyte char straddles `html_max_bytes` cannot panic.
            let mut end = self.config.html_max_bytes;
            while end > 0 && !html.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = &html[..end];
            // Try to break at a tag boundary ('<' is ASCII, always a boundary)
            if let Some(pos) = truncated.rfind('<') {
                &truncated[..pos]
            } else {
                truncated
            }
        }
    }

    /// Parse JSON from LLM response.
    fn parse_json(&self, content: &str) -> AgentResult<serde_json::Value> {
        // Try direct parse first
        if let Ok(json) = serde_json::from_str(content) {
            return Ok(json);
        }

        // Try to extract JSON from markdown code block
        if let Some(start) = content.find("```json") {
            let start = start + 7;
            if let Some(end) = content[start..].find("```") {
                let json_str = content[start..start + end].trim();
                if let Ok(json) = serde_json::from_str(json_str) {
                    return Ok(json);
                }
            }
        }

        // Try to extract JSON from plain code block
        if let Some(start) = content.find("```") {
            let start = start + 3;
            // Skip language identifier if present
            let start = content[start..]
                .find('\n')
                .map(|n| start + n + 1)
                .unwrap_or(start);
            if let Some(end) = content[start..].find("```") {
                let json_str = content[start..start + end].trim();
                if let Ok(json) = serde_json::from_str(json_str) {
                    return Ok(json);
                }
            }
        }

        // Try to find JSON object in content
        if let Some(start) = content.find('{') {
            if let Some(end) = content.rfind('}') {
                // Guard against `}`-before-`{` (e.g. "} text {"): `start > end`
                // would make `content[start..=end]` panic.
                if start <= end {
                    let json_str = &content[start..=end];
                    if let Ok(json) = serde_json::from_str(json_str) {
                        return Ok(json);
                    }
                }
            }
        }

        log::warn!(
            "Failed to parse LLM JSON response; raw content: {:?}",
            content
        );

        Err(AgentError::Json(
            serde_json::from_str::<serde_json::Value>(content).unwrap_err(),
        ))
    }
}

fn is_supported_research_content_type(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    media_type.is_empty() || media_type == "text/html" || media_type == "application/xhtml+xml"
}

fn is_obvious_block_document(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "/cdn-cgi/challenge-platform/",
        "cf-chl-",
        "<title>just a moment...</title>",
        "<title>attention required! | cloudflare</title>",
        "cloudflare ray id",
        "cf-error-details",
        "<title>access denied</title>",
        "<title>verify you are human</title>",
        "<title>robot check</title>",
        "g-recaptcha",
        "hcaptcha",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// Remove specified HTML tags (and their content) from HTML.
///
/// Single-pass per tag — avoids O(n²) from repeated `find()` + concatenation.
fn remove_tags(html: &str, tags: &[&str]) -> String {
    let mut result = html.to_string();
    for tag in tags {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        let mut out = String::with_capacity(result.len());
        // ASCII-lowercase preserves byte length 1:1, so offsets computed on
        // `lower` map exactly onto `result`. `to_lowercase()` can change byte
        // length (e.g. 'İ' -> 2 bytes -> 3), desyncing the offsets and panicking
        // on the `result[..]` slices below. Tag names are ASCII, so this is
        // equivalent for matching.
        let lower = result.to_ascii_lowercase();
        let mut pos = 0;
        while pos < result.len() {
            if let Some(rel_start) = lower[pos..].find(&open) {
                let start = pos + rel_start;
                out.push_str(&result[pos..start]);
                if let Some(rel_end) = lower[start..].find(&close) {
                    pos = start + rel_end + close.len();
                } else {
                    // No closing tag — keep remaining text as-is
                    out.push_str(&result[start..]);
                    pos = result.len();
                }
            } else {
                out.push_str(&result[pos..]);
                break;
            }
        }
        result = out;
    }
    result
}

/// Result from fetching a URL.
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// The URL that was fetched.
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// Content type header.
    pub content_type: String,
    /// HTML content.
    pub html: String,
}

impl From<FetchResult> for AcquiredSource {
    fn from(fetch: FetchResult) -> Self {
        Self {
            requested_url: fetch.url.clone(),
            final_url: fetch.url,
            status: fetch.status,
            content_type: fetch.content_type,
            content: fetch.html,
            acquisition_id: None,
        }
    }
}

/// Result from research.
#[cfg(feature = "search")]
#[derive(Debug, Clone)]
pub struct ResearchResult {
    /// The original research topic.
    pub topic: String,
    /// Search results used.
    pub search_results: SearchResults,
    /// Extracted data from each page.
    pub extractions: Vec<PageExtraction>,
    /// Synthesized summary.
    pub summary: Option<String>,
    /// Token usage.
    pub usage: TokenUsage,
}

/// Extraction from a single page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PageExtraction {
    /// Page URL.
    pub url: String,
    /// Page title.
    pub title: String,
    /// Extracted data.
    pub extracted: serde_json::Value,
    /// Opaque identity of the acquisition used for this extraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition_id: Option<String>,
}

/// Agent builder for configuring and creating agents.
pub struct AgentBuilder {
    config: AgentConfig,
    llm: Option<Box<dyn LLMProvider>>,
    spider_cloud: Option<SpiderCloudToolConfig>,
    spider_browser: Option<SpiderBrowserToolConfig>,
    proxies: Option<Vec<String>>,
    client: Option<reqwest::Client>,
    page_acquirer: Option<Box<dyn PageAcquirer>>,
    #[cfg(feature = "search")]
    search_provider: Option<Box<dyn SearchProvider>>,
    #[cfg(feature = "chrome")]
    browser: Option<BrowserContext>,
    #[cfg(feature = "webdriver")]
    webdriver: Option<WebDriverContext>,
    #[cfg(feature = "fs")]
    enable_temp_storage: bool,
}

impl AgentBuilder {
    /// Create a new builder with defaults.
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            llm: None,
            spider_cloud: None,
            spider_browser: None,
            proxies: None,
            client: None,
            page_acquirer: None,
            #[cfg(feature = "search")]
            search_provider: None,
            #[cfg(feature = "chrome")]
            browser: None,
            #[cfg(feature = "webdriver")]
            webdriver: None,
            #[cfg(feature = "fs")]
            enable_temp_storage: false,
        }
    }

    /// Set the agent configuration.
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    /// Set max concurrent LLM calls.
    pub fn with_max_concurrent_llm_calls(mut self, n: usize) -> Self {
        self.config.max_concurrent_llm_calls = n;
        self
    }

    /// Configure with OpenAI provider.
    #[cfg(feature = "openai")]
    pub fn with_openai(mut self, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        self.llm = Some(Box::new(crate::llm::OpenAIProvider::new(api_key, model)));
        self
    }

    /// Configure with OpenAI-compatible provider.
    #[cfg(feature = "openai")]
    pub fn with_openai_compatible(
        mut self,
        api_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.llm = Some(Box::new(
            crate::llm::OpenAIProvider::new(api_key, model).with_api_url(api_url),
        ));
        self
    }

    /// Configure with OpenAI Responses API.
    ///
    /// Uses the stateful Responses API (`/v1/responses`) instead of Chat
    /// Completions.  System messages become `instructions`, user/assistant
    /// messages become `input` items.
    #[cfg(feature = "openai")]
    pub fn with_openai_responses(
        mut self,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.llm = Some(Box::new(
            crate::llm::OpenAIProvider::new(api_key, model).with_responses_api(),
        ));
        self
    }

    /// Configure with OpenAI-compatible provider using the Responses API.
    #[cfg(feature = "openai")]
    pub fn with_openai_compatible_responses(
        mut self,
        api_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        self.llm = Some(Box::new(
            crate::llm::OpenAIProvider::new(api_key, model)
                .with_responses_api()
                .with_api_url(api_url),
        ));
        self
    }

    /// Register Spider Cloud tools using an API key.
    ///
    /// Registers `/crawl`, `/scrape`, `/search`, `/links`, `/transform`, and
    /// `/unblocker`.
    /// AI routes remain disabled unless enabled in `with_spider_cloud_config`.
    pub fn with_spider_cloud(mut self, api_key: impl Into<String>) -> Self {
        let key = api_key.into();
        if is_placeholder_api_key(&key) {
            log::warn!("Spider Cloud API key looks like a placeholder — skipping. Get a real key at https://spider.cloud");
            return self;
        }
        self.spider_cloud = Some(SpiderCloudToolConfig::new(key));
        self
    }

    /// Register Spider Cloud tools using a full config.
    ///
    /// Use this when you need custom API URL, route toggles, or AI route gating.
    pub fn with_spider_cloud_config(mut self, config: SpiderCloudToolConfig) -> Self {
        self.spider_cloud = Some(config);
        self
    }

    /// Register [Spider Browser Cloud](https://spider.cloud/docs/api#browser) tools.
    ///
    /// Connects to a remote browser instance at `wss://browser.spider.cloud/v1/browser`
    /// via CDP. Registers navigate, html, screenshot, evaluate, click, fill, and wait tools.
    pub fn with_spider_browser(mut self, api_key: impl Into<String>) -> Self {
        let key = api_key.into();
        if is_placeholder_api_key(&key) {
            log::warn!("Spider Browser Cloud API key looks like a placeholder — skipping. Get a real key at https://spider.cloud");
            return self;
        }
        self.spider_browser = Some(SpiderBrowserToolConfig::new(key));
        self
    }

    /// Register Spider Browser Cloud tools using a full config.
    ///
    /// Use this when you need stealth mode, country targeting, or custom WSS URL.
    pub fn with_spider_browser_config(mut self, config: SpiderBrowserToolConfig) -> Self {
        self.spider_browser = Some(config);
        self
    }

    /// Set the HTTP request timeout.
    ///
    /// Pass `None` for no timeout (infinite).  Defaults to 60 seconds.
    ///
    /// # Example
    /// ```ignore
    /// use std::time::Duration;
    /// let agent = Agent::builder()
    ///     .with_timeout(Some(Duration::from_secs(300)))
    ///     .build()?;
    /// ```
    pub fn with_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        match timeout {
            Some(d) => self.config.timeout = d,
            None => self.config.timeout = std::time::Duration::MAX,
        }
        self
    }

    /// Provide a pre-built [`reqwest::Client`].
    ///
    /// When set, the builder skips its own client construction (timeout,
    /// proxy settings, etc.) and uses this client directly.  This gives
    /// full control over TLS, timeouts, connection pools, and proxies.
    ///
    /// # Example
    /// ```ignore
    /// let client = reqwest::Client::builder()
    ///     .timeout(std::time::Duration::from_secs(120))
    ///     .proxy(reqwest::Proxy::all("http://proxy:8080")?)
    ///     .build()?;
    ///
    /// let agent = Agent::builder()
    ///     .with_client(client)
    ///     .with_openai("sk-...", "gpt-4o")
    ///     .build()?;
    /// ```
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Some(client);
        self
    }

    /// Inject the exclusive acquisition authority used by [`Agent::research`].
    ///
    /// Acquisition errors are surfaced directly and never fall back to the
    /// Agent's compatibility HTTP client.
    pub fn with_page_acquirer(mut self, acquirer: Box<dyn PageAcquirer>) -> Self {
        self.page_acquirer = Some(acquirer);
        self
    }

    /// Configure one or more HTTP/SOCKS proxies.
    ///
    /// Each entry is a proxy URL (e.g. `http://host:port`, `socks5://host:port`).
    /// The first proxy is applied to the underlying HTTP client; additional
    /// proxies are added via `reqwest::Proxy::all`.
    ///
    /// # Example
    /// ```ignore
    /// let agent = Agent::builder()
    ///     .with_openai("sk-...", "gpt-4o")
    ///     .with_proxies(vec!["http://proxy.example.com:8080".into()])
    ///     .build()?;
    /// ```
    pub fn with_proxies(mut self, proxies: Vec<String>) -> Self {
        if !proxies.is_empty() {
            self.proxies = Some(proxies);
        }
        self
    }

    /// Configure a single HTTP/SOCKS proxy.
    pub fn with_proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxies = Some(vec![proxy.into()]);
        self
    }

    /// Configure with Serper search provider.
    #[cfg(feature = "search_serper")]
    pub fn with_search_serper(mut self, api_key: impl Into<String>) -> Self {
        self.search_provider = Some(Box::new(crate::search::SerperProvider::new(api_key)));
        self
    }

    /// Configure with Brave search provider.
    #[cfg(feature = "search_brave")]
    pub fn with_search_brave(mut self, api_key: impl Into<String>) -> Self {
        self.search_provider = Some(Box::new(crate::search::BraveProvider::new(api_key)));
        self
    }

    /// Configure with Bing search provider.
    #[cfg(feature = "search_bing")]
    pub fn with_search_bing(mut self, api_key: impl Into<String>) -> Self {
        self.search_provider = Some(Box::new(crate::search::BingProvider::new(api_key)));
        self
    }

    /// Configure with Tavily search provider.
    #[cfg(feature = "search_tavily")]
    pub fn with_search_tavily(mut self, api_key: impl Into<String>) -> Self {
        self.search_provider = Some(Box::new(crate::search::TavilyProvider::new(api_key)));
        self
    }

    /// Configure with a self-hosted SearXNG search provider — no
    /// commercial API key. `base_url` is the operator's own SearXNG
    /// instance (e.g. `"http://localhost:8080"`); there is no public
    /// default instance and none is ever assumed.
    #[cfg(feature = "search_searxng")]
    pub fn with_search_searxng(mut self, base_url: impl Into<String>) -> Self {
        self.search_provider = Some(Box::new(crate::search::SearxngProvider::new(base_url)));
        self
    }

    /// Configure with a browser context for Chrome automation.
    #[cfg(feature = "chrome")]
    pub fn with_browser(mut self, browser: BrowserContext) -> Self {
        self.browser = Some(browser);
        self
    }

    /// Configure with a browser from existing browser and page.
    #[cfg(feature = "chrome")]
    pub fn with_browser_page(
        mut self,
        browser: std::sync::Arc<crate::browser::Browser>,
        page: std::sync::Arc<crate::browser::Page>,
    ) -> Self {
        self.browser = Some(BrowserContext::new(browser, page));
        self
    }

    /// Enable temporary filesystem storage for large operations.
    #[cfg(feature = "fs")]
    pub fn with_temp_storage(mut self) -> Self {
        self.enable_temp_storage = true;
        self
    }

    /// Configure with a WebDriver context.
    #[cfg(feature = "webdriver")]
    pub fn with_webdriver(mut self, webdriver: WebDriverContext) -> Self {
        self.webdriver = Some(webdriver);
        self
    }

    /// Configure with a WebDriver from existing driver.
    #[cfg(feature = "webdriver")]
    pub fn with_webdriver_driver(
        mut self,
        driver: std::sync::Arc<crate::webdriver::WebDriver>,
    ) -> Self {
        self.webdriver = Some(WebDriverContext::new(driver));
        self
    }

    /// Build the agent.
    pub fn build(self) -> AgentResult<Agent> {
        let client = if let Some(client) = self.client {
            client
        } else {
            let mut builder = reqwest::Client::builder();

            // Duration::MAX effectively disables the timeout.
            if self.config.timeout != std::time::Duration::MAX {
                builder = builder.timeout(self.config.timeout);
            }

            if let Some(proxies) = &self.proxies {
                for proxy_url in proxies {
                    let proxy = reqwest::Proxy::all(proxy_url).map_err(AgentError::Http)?;
                    builder = builder.proxy(proxy);
                }
            }

            builder.build().map_err(AgentError::Http)?
        };

        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrent_llm_calls));

        #[cfg(feature = "fs")]
        let temp_storage = if self.enable_temp_storage {
            Some(TempStorage::new().map_err(AgentError::Io)?)
        } else {
            None
        };

        let custom_tools = CustomToolRegistry::new();
        if let Some(cfg) = self.spider_cloud.as_ref() {
            custom_tools.register_spider_cloud(cfg);
        }
        if let Some(cfg) = self.spider_browser.as_ref() {
            custom_tools.register_spider_browser(cfg);
        }

        Ok(Agent {
            llm: self.llm,
            client,
            page_acquirer: self.page_acquirer,
            #[cfg(feature = "search")]
            search_provider: self.search_provider,
            #[cfg(feature = "chrome")]
            browser: self.browser,
            #[cfg(feature = "webdriver")]
            webdriver: self.webdriver,
            #[cfg(feature = "fs")]
            temp_storage,
            memory: AgentMemory::new(),
            llm_semaphore: semaphore,
            config: self.config,
            usage: Arc::new(UsageStats::new()),
            custom_tools,
        })
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_registers_spider_cloud_default_routes() {
        let agent = Agent::builder()
            .with_spider_cloud("sk_spider_cloud")
            .build()
            .expect("agent should build");

        let tools = agent.list_custom_tools();
        assert!(tools.contains(&"spider_cloud_crawl".to_string()));
        assert!(tools.contains(&"spider_cloud_scrape".to_string()));
        assert!(tools.contains(&"spider_cloud_search".to_string()));
        assert!(tools.contains(&"spider_cloud_links".to_string()));
        assert!(tools.contains(&"spider_cloud_transform".to_string()));
        assert!(tools.contains(&"spider_cloud_unblocker".to_string()));
        assert!(!tools.contains(&"spider_cloud_ai_scrape".to_string()));
    }

    #[test]
    fn test_builder_registers_spider_cloud_ai_routes_when_enabled() {
        let cfg = SpiderCloudToolConfig::new("sk_spider_cloud").with_enable_ai_routes(true);
        let agent = Agent::builder()
            .with_spider_cloud_config(cfg)
            .build()
            .expect("agent should build");

        let tools = agent.list_custom_tools();
        assert!(tools.contains(&"spider_cloud_ai_crawl".to_string()));
        assert!(tools.contains(&"spider_cloud_ai_scrape".to_string()));
        assert!(tools.contains(&"spider_cloud_ai_search".to_string()));
        assert!(tools.contains(&"spider_cloud_ai_browser".to_string()));
        assert!(tools.contains(&"spider_cloud_ai_links".to_string()));
    }

    #[test]
    fn test_builder_with_single_proxy() {
        let agent = Agent::builder()
            .with_proxy("http://proxy.example.com:8080")
            .build()
            .expect("agent with proxy should build");
        // Client was constructed — no panic, proxy applied at reqwest level.
        drop(agent);
    }

    #[test]
    fn test_builder_with_multiple_proxies() {
        let agent = Agent::builder()
            .with_proxies(vec![
                "http://proxy1.example.com:8080".into(),
                "http://proxy2.example.com:9090".into(),
            ])
            .build()
            .expect("agent with multiple proxies should build");
        drop(agent);
    }

    #[test]
    fn test_builder_with_socks5_proxy() {
        let agent = Agent::builder()
            .with_proxy("socks5://127.0.0.1:1080")
            .build()
            .expect("agent with socks5 proxy should build");
        drop(agent);
    }

    #[test]
    fn test_builder_with_empty_proxies_no_op() {
        let agent = Agent::builder()
            .with_proxies(vec![])
            .build()
            .expect("agent with empty proxies should build");
        drop(agent);
    }

    #[test]
    fn test_builder_with_invalid_proxy_returns_error() {
        let result = Agent::builder()
            .with_proxy("not a valid url at all ://")
            .build();
        assert!(result.is_err(), "invalid proxy URL should fail at build");
    }

    #[test]
    fn test_builder_no_proxies_by_default() {
        // Default builder has no proxies — just ensure it builds fine.
        let agent = Agent::builder()
            .build()
            .expect("default agent should build");
        drop(agent);
    }

    #[test]
    fn test_builder_with_custom_client() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("custom client should build");

        let agent = Agent::builder()
            .with_client(client)
            .build()
            .expect("agent with custom client should build");
        // Verify the client accessor works.
        let _ = agent.client();
    }

    #[test]
    fn test_builder_with_custom_client_ignores_proxy_and_timeout() {
        // When a custom client is provided, proxy/timeout settings on the
        // builder are bypassed — the caller owns the full client config.
        let client = reqwest::Client::new();
        let agent = Agent::builder()
            .with_client(client)
            .with_proxy("http://proxy.example.com:8080")
            .with_timeout(Some(std::time::Duration::from_secs(999)))
            .build()
            .expect("custom client should take precedence");
        drop(agent);
    }

    #[test]
    fn test_builder_with_timeout_some() {
        let agent = Agent::builder()
            .with_timeout(Some(std::time::Duration::from_secs(300)))
            .build()
            .expect("agent with 300s timeout should build");
        drop(agent);
    }

    #[test]
    fn test_builder_with_timeout_none_infinite() {
        let agent = Agent::builder()
            .with_timeout(None)
            .build()
            .expect("agent with no timeout should build");
        drop(agent);
    }

    #[test]
    fn test_client_accessor_returns_reference() {
        let agent = Agent::builder().build().expect("agent should build");
        // Ensure we can obtain a shared reference without moving.
        let _c1 = agent.client();
        let _c2 = agent.client();
    }

    #[test]
    fn test_builder_registers_spider_browser_default_tools() {
        let agent = Agent::builder()
            .with_spider_browser("sk_browser_key")
            .build()
            .expect("agent should build");

        let tools = agent.list_custom_tools();
        assert!(tools.contains(&"spider_browser_navigate".to_string()));
        assert!(tools.contains(&"spider_browser_html".to_string()));
        assert!(tools.contains(&"spider_browser_screenshot".to_string()));
        assert!(tools.contains(&"spider_browser_evaluate".to_string()));
        assert!(tools.contains(&"spider_browser_click".to_string()));
        assert!(tools.contains(&"spider_browser_fill".to_string()));
        assert!(tools.contains(&"spider_browser_wait".to_string()));
    }

    #[test]
    fn test_builder_spider_browser_with_stealth_and_country() {
        let cfg = SpiderBrowserToolConfig::new("sk_key")
            .with_stealth(true)
            .with_country("us");
        assert_eq!(
            cfg.connection_url(),
            "wss://browser.spider.cloud/v1/browser?token=sk_key&stealth=true&country=us"
        );

        let agent = Agent::builder()
            .with_spider_browser_config(cfg)
            .build()
            .expect("agent should build");
        assert!(agent.has_custom_tool("spider_browser_navigate"));
    }

    #[test]
    fn test_builder_spider_cloud_and_browser_together() {
        let agent = Agent::builder()
            .with_spider_cloud("cloud-key")
            .with_spider_browser("browser-key")
            .build()
            .expect("agent should build");

        // Both sets of tools registered.
        assert!(agent.has_custom_tool("spider_cloud_crawl"));
        assert!(agent.has_custom_tool("spider_browser_navigate"));
    }

    #[cfg(feature = "search")]
    mod research_acquisition {
        use super::*;
        use crate::llm::{CompletionOptions, LLMProvider};
        use crate::search::{SearchResult, SearchResults};
        use async_trait::async_trait;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StaticSearch {
            urls: Vec<String>,
        }

        #[async_trait]
        impl SearchProvider for StaticSearch {
            async fn search(
                &self,
                query: &str,
                _options: &SearchOptions,
            ) -> Result<SearchResults, crate::error::SearchError> {
                let mut results = SearchResults::new(query);
                for (index, url) in self.urls.iter().enumerate() {
                    results.push(SearchResult::new(
                        format!("Source {}", index + 1),
                        url,
                        index + 1,
                    ));
                }
                Ok(results)
            }

            fn provider_name(&self) -> &'static str {
                "static"
            }

            fn is_configured(&self) -> bool {
                true
            }
        }

        struct JsonLlm {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl LLMProvider for JsonLlm {
            async fn complete(
                &self,
                _messages: Vec<Message>,
                _options: &CompletionOptions,
                _client: &reqwest::Client,
            ) -> AgentResult<CompletionResponse> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(CompletionResponse {
                    content: r#"{"ok":true}"#.to_string(),
                    usage: TokenUsage::default(),
                })
            }

            fn provider_name(&self) -> &'static str {
                "json-test"
            }

            fn is_configured(&self) -> bool {
                true
            }
        }

        struct StaticAcquirer {
            calls: Arc<AtomicUsize>,
            fail: bool,
            status: u16,
        }

        #[async_trait]
        impl PageAcquirer for StaticAcquirer {
            async fn acquire(&self, url: &str) -> AgentResult<AcquiredSource> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    return Err(AgentError::Remote("injected acquisition failed".into()));
                }
                Ok(AcquiredSource {
                    requested_url: url.to_string(),
                    final_url: format!("{url}/final"),
                    status: self.status,
                    content_type: "text/html".to_string(),
                    content: "<html><body>research source</body></html>".to_string(),
                    acquisition_id: Some(format!("opaque-{call}")),
                })
            }
        }

        fn agent(
            urls: Vec<String>,
            acquirer: Option<Box<dyn PageAcquirer>>,
            llm_calls: Arc<AtomicUsize>,
        ) -> Agent {
            let mut builder = Agent::builder();
            builder.search_provider = Some(Box::new(StaticSearch { urls }));
            builder.llm = Some(Box::new(JsonLlm { calls: llm_calls }));
            if let Some(acquirer) = acquirer {
                builder = builder.with_page_acquirer(acquirer);
            }
            builder.build().unwrap()
        }

        fn options(max_pages: usize) -> ResearchOptions {
            ResearchOptions::new()
                .with_max_pages(max_pages)
                .with_synthesize(false)
        }

        #[tokio::test]
        async fn injected_acquirer_handles_selected_urls_and_preserves_lineage() {
            let calls = Arc::new(AtomicUsize::new(0));
            let llm_calls = Arc::new(AtomicUsize::new(0));
            let agent = agent(
                vec![
                    "https://one.example".into(),
                    "https://two.example".into(),
                    "https://three.example".into(),
                ],
                Some(Box::new(StaticAcquirer {
                    calls: calls.clone(),
                    fail: false,
                    status: 200,
                })),
                llm_calls.clone(),
            );

            let result = agent.research("topic", options(2)).await.unwrap();

            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(llm_calls.load(Ordering::SeqCst), 2);
            assert_eq!(result.extractions.len(), 2);
            assert_eq!(
                result.extractions[0].acquisition_id.as_deref(),
                Some("opaque-0")
            );
            assert_eq!(
                result.extractions[1].acquisition_id.as_deref(),
                Some("opaque-1")
            );
            assert_eq!(result.extractions[0].url, "https://one.example/final");
        }

        #[tokio::test]
        async fn injected_failure_never_falls_back_to_agent_fetch() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/must-not-connect", listener.local_addr().unwrap());
            let calls = Arc::new(AtomicUsize::new(0));
            let agent = agent(
                vec![url],
                Some(Box::new(StaticAcquirer {
                    calls: calls.clone(),
                    fail: true,
                    status: 200,
                })),
                Arc::new(AtomicUsize::new(0)),
            );

            let result = agent.research("topic", options(1)).await.unwrap();

            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(result.extractions.is_empty());
            assert!(matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
        }

        #[tokio::test]
        async fn injected_http_rejection_skips_extraction() {
            let calls = Arc::new(AtomicUsize::new(0));
            let llm_calls = Arc::new(AtomicUsize::new(0));
            let agent = agent(
                vec!["https://forbidden.example".into()],
                Some(Box::new(StaticAcquirer {
                    calls: calls.clone(),
                    fail: false,
                    status: 403,
                })),
                llm_calls.clone(),
            );

            let result = agent.research("topic", options(1)).await.unwrap();

            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(llm_calls.load(Ordering::SeqCst), 0);
            assert!(result.extractions.is_empty());
        }

        #[tokio::test]
        async fn research_without_acquirer_uses_compatibility_fetch() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/compatibility", listener.local_addr().unwrap());
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                let body = b"<html><body>compatibility research</body></html>";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            });
            let agent = agent(vec![url], None, Arc::new(AtomicUsize::new(0)));

            let result = agent.research("topic", options(1)).await.unwrap();
            handle.join().unwrap();

            assert_eq!(result.extractions.len(), 1);
            assert_eq!(result.extractions[0].acquisition_id, None);
        }
    }
}
