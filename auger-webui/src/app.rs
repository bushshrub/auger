//! One router: the agent API under /v1, the webui build everywhere else.
use axum::Router;
use std::path::Path;
use tower_http::services::ServeDir;
use tower_http::services::ServeFile;

pub fn router(dir: &Path) -> Result<Router, String> {
    let api = agent_server::api_router()?;
    let index = ServeFile::new(dir.join("index.html"));
    let files = ServeDir::new(dir).fallback(index);
    Ok(Router::new().nest("/v1", api).fallback_service(files))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn serves_index_and_assets_and_api() {
        let dir = std::env::temp_dir().join(format!("auger-webui-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("_app")).unwrap();
        std::fs::write(dir.join("index.html"), "<html>app</html>").unwrap();
        std::fs::write(dir.join("_app/app.js"), "console.log(1)").unwrap();

        let router = super::router(&dir).unwrap();
        let get = |path: &str| {
            let request = Request::builder().uri(path).body(Body::empty()).unwrap();
            router.clone().oneshot(request)
        };

        let root = get("/").await.unwrap();
        assert_eq!(root.status(), 200);
        let client_route = get("/sessions/abc").await.unwrap();
        assert_eq!(client_route.status(), 200);
        let asset = get("/_app/app.js").await.unwrap();
        assert_eq!(asset.status(), 200);
        assert!(asset.headers()["content-type"].to_str().unwrap().contains("javascript"));
        let api = get("/v1/sessions").await.unwrap();
        assert_eq!(api.status(), 200);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
