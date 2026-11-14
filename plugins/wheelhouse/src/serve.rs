//! `wheelhouse serve`: the registry's views over MCP, one process for every artifact and every
//! project the host has open.
//!
//! The host names the asking artifact, project and pane in `_meta` (`wheel/artifact`,
//! `wheel/dir`, `wheel/pane`) on each call, read and tool listing, and each request is answered by
//! the view that artifact names. The host subscribes to `a2ui://<view>?dir=<project>` while a pane
//! shows the project; a change to the project's files is announced with
//! `notifications/resources/updated` for that URI, and a project nobody subscribes to is not
//! watched.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};

use crate::core::project::{parse_uri, Origin};
use crate::core::watch::Watcher;
use crate::view::{Registry, MIME};

#[derive(Clone)]
pub struct Hub {
    registry: Registry,
    watcher: Arc<Mutex<Watcher>>,
}

impl Hub {
    pub fn new(registry: Registry) -> Self {
        Hub { registry, watcher: Arc::default() }
    }

    fn view_of(&self, artifact: Option<&str>) -> Result<Arc<dyn crate::view::View>, ErrorData> {
        self.registry.of(artifact).map_err(|e| ErrorData::invalid_params(e, None))
    }

    fn view_named_by(&self, uri: &str) -> Result<(Arc<dyn crate::view::View>, Option<String>), ErrorData> {
        let (name, dir) = parse_uri(uri).ok_or_else(|| ErrorData::resource_not_found(format!("no resource {uri}"), None))?;
        let view = self.registry.get(&name).ok_or_else(|| ErrorData::resource_not_found(format!("no resource {uri}"), None))?;
        Ok((view, dir))
    }

    /// One second passed: refresh the subscriptions that are due and return the URIs that changed.
    fn tick(&self) -> Vec<String> {
        let due = self.watcher.lock().unwrap().tick();
        let mut changed = vec![];
        // A page that finished work in the background is drawn again at once.
        let subs = self.watcher.lock().unwrap().subscriptions();
        for d in subs {
            if self.registry.get(&d.view).is_some_and(|v| v.finished(&d.dir)) && !changed.contains(&d.uri) {
                changed.push(d.uri);
            }
        }
        for d in due {
            if let Some(view) = self.registry.get(&d.view) {
                if view.refresh(&d.dir) {
                    // The files the page depends on may have changed with it.
                    self.watcher.lock().unwrap().rewatch(&d.view, &d.dir, view.watches(&d.dir));
                    changed.push(d.uri);
                }
            }
        }
        changed
    }
}

impl ServerHandler for Hub {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder().enable_tools().enable_resources().enable_resources_subscribe().build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let artifact = Origin::of(&context.meta).artifact;
        let tools = match artifact {
            Some(name) => self.view_of(Some(&name))?.tools(),
            None => self.registry.all().iter().flat_map(|v| v.tools()).collect(),
        };
        Ok(ListToolsResult { tools, ..Default::default() })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let artifact = Origin::of(&context.meta).artifact;
        let view = self.view_of(artifact.as_deref())?;
        let mut request = request;
        if request.name == "data_changed" {
            let args = request.arguments.clone().unwrap_or_default();
            let (path, value) = (args.get("path").and_then(|p| p.as_str()).unwrap_or(""), args.get("value").cloned().unwrap_or_default());
            let (tool, arguments) = view
                .changed(path, &value)
                .ok_or_else(|| ErrorData::invalid_params(format!("{} has no control at {path}", view.name()), None))?;
            request.name = tool.into();
            request.arguments = Some(arguments);
        }
        view.call(request, context).await
    }

    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let (view, dir) = self.view_named_by(&request.uri)?;
        if let Some(dir) = dir {
            // The mtimes are taken before the load, so a change during it is announced.
            {
                let mut w = self.watcher.lock().unwrap();
                w.watch(view.name(), &dir, &request.uri, view.watches(&dir), view.every());
            }
            view.open(&dir);
        }
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        // A URI no view owns has nothing to end.
        if let Ok((view, Some(dir))) = self.view_named_by(&request.uri) {
            self.watcher.lock().unwrap().unwatch(view.name(), &dir);
            view.close(&dir);
        }
        Ok(())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let (view, _) = self.view_named_by(&request.uri)?;
        Ok(ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: request.uri,
            mime_type: Some(MIME.into()),
            text: view.skeleton().to_string(),
            meta: None,
        }])
        .into())
    }
}

pub async fn run(registry: Registry) -> anyhow::Result<()> {
    let hub = Hub::new(registry);
    let running = hub.clone().serve(rmcp::transport::stdio()).await?;
    let peer = running.peer().clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let h = hub.clone();
            let changed = tokio::task::spawn_blocking(move || h.tick()).await.unwrap_or_default();
            for uri in changed {
                if peer.notify_resource_updated(rmcp::model::ResourceUpdatedNotificationParam::new(uri)).await.is_err() {
                    return;
                }
            }
        }
    });
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::View;
    use async_trait::async_trait;
    use rmcp::model::{CallToolResult, ContentBlock, RequestMetaObject, MetaObject, Tool};
    use serde_json::{json, Value};
    use std::path::PathBuf;

    /// A view that answers `whoami` with its own name and the project the request is for.
    struct Stub(&'static str);

    #[async_trait]
    impl View for Stub {
        fn name(&self) -> &str {
            self.0
        }
        fn tools(&self) -> Vec<Tool> {
            let schema = json!({ "type": "object" }).as_object().unwrap().clone();
            vec![Tool::new(format!("{}_whoami", self.0), "who is asking", schema)]
        }
        async fn call(
            &self,
            params: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let o = Origin::of(&context.meta);
            let _ = params;
            Ok(CallToolResult::success(vec![ContentBlock::text(format!("{}:{}:{}", self.0, o.dir, o.pane))]).into())
        }
        fn skeleton(&self) -> Value {
            json!({ "view": self.0 })
        }
        fn watches(&self, _dir: &str) -> Vec<PathBuf> {
            vec![]
        }
        fn open(&self, _dir: &str) {}
        fn close(&self, _dir: &str) {}
        fn refresh(&self, _dir: &str) -> bool {
            false
        }
    }

    fn meta(artifact: &str, dir: &str) -> RequestMetaObject {
        let m = json!({ "wheel/artifact": artifact, "wheel/dir": dir, "wheel/pane": "p1" });
        RequestMetaObject(MetaObject(m.as_object().unwrap().clone()))
    }

    async fn client() -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
        let (server_io, client_io) = tokio::io::duplex(65536);
        let hub = Hub::new(Registry::default().with(Stub("alpha")).with(Stub("beta")));
        tokio::spawn(async move {
            hub.serve(server_io).await.unwrap().waiting().await.unwrap();
        });
        ().serve(client_io).await.unwrap()
    }

    async fn ask(c: &rmcp::service::RunningService<rmcp::RoleClient, ()>, artifact: &str, dir: &str, tool: &str) -> Result<String, String> {
        let mut p = CallToolRequestParams::new(tool.to_string());
        p.meta = Some(meta(artifact, dir));
        let r = c.peer().call_tool(p).await.map_err(|e| e.to_string())?;
        Ok(serde_json::to_value(&r.content[0]).unwrap()["text"].as_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn each_artifact_is_answered_by_its_own_view() {
        let c = client().await;
        assert_eq!(ask(&c, "alpha", "/p/a", "alpha_whoami").await.unwrap(), "alpha:/p/a:p1");
        assert_eq!(ask(&c, "beta", "/p/a", "beta_whoami").await.unwrap(), "beta:/p/a:p1");
        assert!(ask(&c, "gamma", "/p/a", "alpha_whoami").await.is_err(), "no such view");
    }

    #[tokio::test]
    async fn one_view_shows_each_project_its_own_data() {
        let c = client().await;
        assert_eq!(ask(&c, "alpha", "/p/one", "alpha_whoami").await.unwrap(), "alpha:/p/one:p1");
        assert_eq!(ask(&c, "alpha", "/p/two", "alpha_whoami").await.unwrap(), "alpha:/p/two:p1");
    }

    #[tokio::test]
    async fn tools_and_pages_are_listed_per_view_by_the_named_artifact() {
        let c = client().await;
        let mut req = PaginatedRequestParams::default();
        req.meta = Some(meta("beta", "/p"));
        let tools = c.peer().list_tools(Some(req)).await.unwrap().tools;
        assert_eq!(tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(), vec!["beta_whoami"]);
        let all = c.peer().list_tools(None).await.unwrap().tools;
        assert_eq!(all.len(), 2, "no artifact named: every view's tools");
        let page = c.peer().read_resource(ReadResourceRequestParams::new("a2ui://beta?dir=%2Fp")).await.unwrap();
        assert!(serde_json::to_string(&page).unwrap().contains("beta"));
        assert!(c.peer().read_resource(ReadResourceRequestParams::new("a2ui://gamma")).await.is_err());
    }
}
