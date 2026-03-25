use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{Response, StatusCode, header},
    routing::get,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub struct BundleFixture {
    pub status: StatusCode,
    pub body: String,
    pub content_type: &'static str,
}

impl BundleFixture {
    pub fn markdown(body: impl Into<String>) -> Self {
        Self {
            status: StatusCode::OK,
            body: body.into(),
            content_type: "text/markdown; charset=utf-8",
        }
    }

    pub fn with_status(status: StatusCode, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            content_type: "text/plain; charset=utf-8",
        }
    }
}

#[derive(Clone)]
struct BundleFixtureState {
    fixtures: Arc<RwLock<HashMap<String, BundleFixture>>>,
}

pub struct SkillBundleHarness {
    base_url: String,
    fixtures: Arc<RwLock<HashMap<String, BundleFixture>>>,
    server: JoinHandle<()>,
}

impl SkillBundleHarness {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub fn set_fixture(&self, path: &str, fixture: BundleFixture) {
        self.fixtures
            .write()
            .expect("bundle harness fixture write lock")
            .insert(normalize_path(path.to_owned()), fixture);
    }

    pub fn remove_fixture(&self, path: &str) {
        self.fixtures
            .write()
            .expect("bundle harness fixture write lock")
            .remove(&normalize_path(path.to_owned()));
    }
}

impl Drop for SkillBundleHarness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub async fn spawn_skill_bundle<I, P>(fixtures: I) -> SkillBundleHarness
where
    I: IntoIterator<Item = (P, BundleFixture)>,
    P: Into<String>,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local bundle harness");
    let address = listener
        .local_addr()
        .expect("read local bundle harness addr");
    let fixtures = Arc::new(RwLock::new(
        fixtures
            .into_iter()
            .map(|(path, fixture)| (normalize_path(path.into()), fixture))
            .collect(),
    ));
    let state = BundleFixtureState {
        fixtures: Arc::clone(&fixtures),
    };

    let app = Router::new()
        .route("/", get(handle_root))
        .route("/{*path}", get(handle_document))
        .with_state(state);
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    SkillBundleHarness {
        base_url: format!("http://{}", address),
        fixtures,
        server,
    }
}

async fn handle_root(State(state): State<BundleFixtureState>) -> Response<Body> {
    response_for_path(&state, "/")
}

async fn handle_document(
    State(state): State<BundleFixtureState>,
    Path(path): Path<String>,
) -> Response<Body> {
    response_for_path(&state, &format!("/{}", path))
}

fn response_for_path(state: &BundleFixtureState, path: &str) -> Response<Body> {
    match state
        .fixtures
        .read()
        .expect("bundle harness fixture read lock")
        .get(path)
        .cloned()
    {
        Some(fixture) => Response::builder()
            .status(fixture.status)
            .header(header::CONTENT_TYPE, fixture.content_type)
            .body(Body::from(fixture.body))
            .expect("bundle fixture response"),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from(format!("no bundle fixture for {path}")))
            .expect("bundle fixture 404 response"),
    }
}

fn normalize_path(path: String) -> String {
    if path.is_empty() {
        "/".to_owned()
    } else if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}
