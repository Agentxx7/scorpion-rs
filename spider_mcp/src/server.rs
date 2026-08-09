use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use std::sync::Arc;

use crate::state::SharedState;
#[cfg(feature = "feed")]
use crate::tools::feed::FeedReadParams;
#[cfg(feature = "news_sitemap")]
use crate::tools::news_sitemap::NewsSitemapReadParams;
#[cfg(feature = "sitemap")]
use crate::tools::sitemap::SitemapReadParams;
use crate::tools::{
    crawl::CrawlParams, crawl_status::CrawlStatusParams, links::LinksParams,
    media_search::MediaSearchParams, news_search::NewsSearchParams, scrape::ScrapeParams,
    search::SearchParams, transform::TransformParams,
};

#[derive(Clone)]
pub struct SpiderMcpServer {
    state: Arc<SharedState>,
    #[allow(dead_code)]
    tool_router: rmcp::handler::server::router::tool::ToolRouter<Self>,
}

impl SpiderMcpServer {
    pub fn new() -> Self {
        let state = Arc::new(SharedState::new());
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        let router = Self::base_tool_router();
        #[cfg(feature = "feed")]
        let router = {
            let mut router = router;
            router.merge(Self::feed_tool_router());
            router
        };
        #[cfg(feature = "sitemap")]
        let router = {
            let mut router = router;
            router.merge(Self::sitemap_tool_router());
            router
        };
        #[cfg(feature = "news_sitemap")]
        let router = {
            let mut router = router;
            router.merge(Self::news_sitemap_tool_router());
            router
        };
        router
    }
}

#[cfg(feature = "news_sitemap")]
#[tool_router(router = news_sitemap_tool_router)]
impl SpiderMcpServer {
    #[tool(
        name = "spider_news_sitemap_read",
        description = "Fetch exactly one Google News Sitemap, preserve evidence for the exact response bytes, and return structurally associated News metadata without fetching articles, media, or child sitemaps."
    )]
    async fn news_sitemap_read(
        &self,
        Parameters(params): Parameters<NewsSitemapReadParams>,
    ) -> Result<String, String> {
        crate::tools::news_sitemap::run(params).await
    }
}

#[tool_router(router = base_tool_router)]
impl SpiderMcpServer {
    #[tool(
        name = "spider_scrape",
        description = "Fetch a web page and return its content as markdown, text, HTML, XML, screenshot, or explicit opt-in auto. Auto routes HTTP HTML to Markdown, preserves JSON/XML text, extracts PDF plain text, and reports unsupported extraction for other binary formats; headless auto transforms the rendered DOM to Markdown without claiming the original resource type. Set evidence=true to instead return a structured EvidenceBundle with content integrity hashes plus independent declared and byte-detected content types. HTTP byte-derived fields are null for Chrome/headless fetches; default output is unchanged."
    )]
    async fn scrape(&self, Parameters(params): Parameters<ScrapeParams>) -> Result<String, String> {
        crate::tools::scrape::run(params).await
    }

    #[tool(
        name = "spider_crawl",
        description = "Crawl a website discovering linked pages up to a configurable depth and limit. Returns page content in the requested format."
    )]
    async fn crawl(&self, Parameters(params): Parameters<CrawlParams>) -> Result<String, String> {
        crate::tools::crawl::run(params, self.state.clone()).await
    }

    #[tool(
        name = "spider_links",
        description = "Extract all links from a web page without fetching full content. Lightweight alternative to crawling."
    )]
    async fn links(&self, Parameters(params): Parameters<LinksParams>) -> Result<String, String> {
        crate::tools::links::run(params).await
    }

    #[tool(
        name = "spider_transform",
        description = "Convert raw HTML to markdown, plain text, or XML. No network requests — pure offline transformation."
    )]
    async fn transform(
        &self,
        Parameters(params): Parameters<TransformParams>,
    ) -> Result<String, String> {
        crate::tools::transform::run(params)
    }

    #[tool(
        name = "spider_crawl_status",
        description = "Check the status of a background crawl started by spider_crawl (crawls above the inline page-limit threshold). Returns running/complete/failed status, page count, and collected pages so far."
    )]
    async fn crawl_status(
        &self,
        Parameters(params): Parameters<CrawlStatusParams>,
    ) -> Result<String, String> {
        crate::tools::crawl_status::run(params, self.state.clone())
    }

    #[tool(
        name = "spider_search",
        description = "Run a web search via a configured search provider and return structured results (position, title, url, snippet, date, score) for a downstream agent to evaluate. Does not fetch or crawl the result URLs. Currently supports provider=\"searxng\" against a self-hosted SearXNG instance — base_url is required, no public instance is assumed, and no API key is used."
    )]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> Result<String, String> {
        crate::tools::search::run(params).await
    }

    #[tool(
        name = "spider_media_search",
        description = "Search a media category (video or image) via a configured search provider and return structured candidates (title/url/thumbnail/description/etc.) for a downstream agent to evaluate. Does not fetch, download, or open any result. Currently supports provider=\"searxng\" against a self-hosted SearXNG instance — base_url is required, no public instance is assumed, and no API key is used."
    )]
    async fn media_search(
        &self,
        Parameters(params): Parameters<MediaSearchParams>,
    ) -> Result<String, String> {
        crate::tools::media_search::run(params).await
    }

    #[tool(
        name = "spider_news_search",
        description = "Discover news results through a configured self-hosted SearXNG instance. Returns headlines, article URLs, summaries, opaque publish-date strings, authors, thumbnails, raw engine values, and scores. Does not fetch articles or images."
    )]
    async fn news_search(
        &self,
        Parameters(params): Parameters<NewsSearchParams>,
    ) -> Result<String, String> {
        crate::tools::news_search::run(params).await
    }
}

#[cfg(feature = "sitemap")]
#[tool_router(router = sitemap_tool_router)]
impl SpiderMcpServer {
    #[tool(
        name = "spider_sitemap_read",
        description = "Fetch exactly one standard sitemap urlset or sitemapindex, preserve evidence for the original response bytes, and return discovery candidates without fetching content or child sitemap URLs."
    )]
    async fn sitemap_read(
        &self,
        Parameters(params): Parameters<SitemapReadParams>,
    ) -> Result<String, String> {
        crate::tools::sitemap::run(params).await
    }
}

#[cfg(feature = "feed")]
#[tool_router(router = feed_tool_router)]
impl SpiderMcpServer {
    #[tool(
        name = "spider_feed_read",
        description = "Fetch exactly one RSS or Atom feed, preserve evidence for the original response bytes, and return normalized discovery entries without fetching article or media URLs."
    )]
    async fn feed_read(
        &self,
        Parameters(params): Parameters<FeedReadParams>,
    ) -> Result<String, String> {
        crate::tools::feed::run(params).await
    }
}

#[tool_handler]
impl ServerHandler for SpiderMcpServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::from_build_env())
    }

    /// Advertises `crawl://{crawl_id}/summary` — the resource `spider_crawl`'s
    /// background path promises callers ("read resource crawl://{crawl_id}/summary")
    /// but which had no handler at all until this.
    async fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListResourceTemplatesResult, rmcp::ErrorData> {
        Ok(rmcp::model::ListResourceTemplatesResult::with_all_items(
            vec![crate::tools::crawl_status::resource_template()],
        ))
    }

    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResult, rmcp::ErrorData> {
        crate::tools::crawl_status::read_resource(&request.uri, &self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "news_sitemap")]
    #[test]
    fn spider_news_sitemap_read_is_registered_with_exact_properties() {
        let tools = SpiderMcpServer::tool_router().list_all();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "spider_news_sitemap_read")
            .unwrap();
        let properties = tool.input_schema["properties"].as_object().unwrap();
        assert_eq!(properties.keys().collect::<Vec<_>>(), ["limit", "url"]);
    }

    #[cfg(feature = "sitemap")]
    #[test]
    fn spider_sitemap_read_tool_is_registered() {
        let router = SpiderMcpServer::tool_router();
        assert!(router
            .list_all()
            .iter()
            .any(|tool| tool.name == "spider_sitemap_read"));
    }

    #[cfg(feature = "feed")]
    #[test]
    fn spider_feed_read_tool_is_registered() {
        let router = SpiderMcpServer::tool_router();
        assert!(router
            .list_all()
            .iter()
            .any(|tool| tool.name == "spider_feed_read"));
    }

    /// 5 (tool half) — RED before crawl_status was wired into tools/mod.rs
    /// and registered here: this test could not even compile, since the
    /// tool didn't exist anywhere in the crate.
    #[test]
    fn spider_crawl_status_tool_is_registered() {
        let router = SpiderMcpServer::tool_router();
        assert!(
            router
                .list_all()
                .iter()
                .any(|t| t.name == "spider_crawl_status"),
            "spider_crawl_status must be registered in the tool router"
        );
    }

    /// 1. spider_search is registered in MCP.
    #[test]
    fn spider_search_tool_is_registered() {
        let router = SpiderMcpServer::tool_router();
        assert!(
            router.list_all().iter().any(|t| t.name == "spider_search"),
            "spider_search must be registered in the tool router"
        );
    }

    /// 1. spider_media_search is registered in MCP.
    #[test]
    fn spider_media_search_tool_is_registered() {
        let router = SpiderMcpServer::tool_router();
        assert!(
            router
                .list_all()
                .iter()
                .any(|t| t.name == "spider_media_search"),
            "spider_media_search must be registered in the tool router"
        );
    }

    #[test]
    fn spider_news_search_tool_is_registered_with_exact_input_schema() {
        let router = SpiderMcpServer::tool_router();
        let tools = router.list_all();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "spider_news_search")
            .unwrap();
        let actual = serde_json::Value::Object((*tool.input_schema).clone());
        let expected = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "base_url": {"description": "Base URL of the caller's self-hosted SearXNG instance.", "type": ["string", "null"]},
                "language": {"description": "Language code passed through to SearXNG (for example, \"en\").", "type": ["string", "null"]},
                "limit": {"description": "Maximum number of results to return.", "format": "uint", "type": ["integer", "null"], "minimum": 0},
                "provider": {"description": "Search provider to use. Currently supported: \"searxng\".", "type": "string"},
                "query": {"description": "The news query.", "type": "string"}
            },
            "required": ["query", "provider"]
        });
        assert_eq!(actual, expected);
    }
}
