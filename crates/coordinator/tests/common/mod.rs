#![allow(dead_code, clippy::expect_used)]
use orchestrator_coordinator::store::Store;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

pub struct Fixture {
    pub store: Store,
    pub raw: PgPool,
    pub url: String,
    schema: String,
    admin: PgPool,
}
impl Fixture {
    pub async fn new() -> Self {
        let base = std::env::var("COORDINATOR_TEST_DATABASE_URL")
            .expect("set COORDINATOR_TEST_DATABASE_URL to a dedicated PostgreSQL test database");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&base)
            .await
            .expect("test database");
        let schema = format!("test_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&admin)
            .await
            .expect("isolated schema");
        let mut url = reqwest::Url::parse(&base).expect("database URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let url = url.to_string();
        let store = Store::open(&url).await.expect("migrations");
        let raw = PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("inspection pool");
        Self {
            store,
            raw,
            url,
            schema,
            admin,
        }
    }
    pub async fn enrolled() -> Self {
        let f = Self::new().await;
        let report = super::data::report();
        f.store
            .enroll(report.group_id, report.node_id, &super::data::token())
            .await
            .expect("enroll");
        f
    }
    pub async fn sql(&self, sql: &'static str) {
        sqlx::raw_sql(sql)
            .execute(&self.raw)
            .await
            .expect("fault injection");
    }
    pub async fn close(self) {
        self.store.close().await;
        self.raw.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("remove schema");
        self.admin.close().await;
    }
}

pub struct Server {
    pub origin: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn start(router: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let (stop, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = done.await;
                })
                .await
                .expect("server");
        });
        Self {
            origin,
            stop: Some(stop),
            task,
        }
    }
    pub async fn close(mut self) {
        self.stop.take().expect("stop").send(()).expect("running");
        (&mut self.task).await.expect("joined");
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
