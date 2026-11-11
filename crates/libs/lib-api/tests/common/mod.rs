//! The Postgres database of one test binary, built as a deployment builds it.

use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use std::fs;
use std::path::Path;
use tokio::sync::OnceCell;

fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgresql://postgres:postgres123@localhost:5432/trading_db".to_string()
    })
}

/// Setup test database connection
async fn setup_test_db() -> Result<DatabaseConnection, sea_orm::DbErr> {
    let connection_future = Database::connect(database_url());
    let timeout_duration = std::time::Duration::from_secs(5);

    match tokio::time::timeout(timeout_duration, connection_future).await {
        Ok(Ok(db)) => Ok(db),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(sea_orm::DbErr::Custom(
            "Database connection timeout".to_string(),
        )),
    }
}

/// Creates the tests' database afresh, `<DATABASE_URL's database>_<test binary>`, as a
/// deployment does: the read model's Postgres migrations, then its development seeds. Returns
/// its URL.
async fn create_schema_database(server: &DatabaseConnection) -> String {
    let mut url = url::Url::parse(&database_url()).expect("DATABASE_URL is a URL");
    let name = format!(
        "{}_{}",
        url.path().trim_start_matches('/'),
        env!("CARGO_CRATE_NAME")
    );
    for statement in [
        format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#),
        format!(r#"CREATE DATABASE "{name}""#),
    ] {
        server.execute_unprepared(&statement).await.unwrap();
    }
    url.set_path(&name);
    let db = Database::connect(url.as_str()).await.unwrap();

    let readmodel = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../readmodel");
    let mut scripts: Vec<_> = fs::read_dir(readmodel.join("migrations/postgres"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".up.sql"))
        .collect();
    scripts.sort();
    scripts.push(readmodel.join("seeds/development.sql"));
    for script in scripts {
        let sql = fs::read_to_string(&script).unwrap();
        if let Err(error) = db.execute_unprepared(&sql).await {
            panic!("{}: {error}", script.display());
        }
    }

    url.into()
}

/// A connection to the tests' database, which the first caller creates; None, so that the test
/// is skipped, when no Postgres server answers at DATABASE_URL.
pub async fn require_database() -> Option<DatabaseConnection> {
    static SCHEMA_DATABASE: OnceCell<String> = OnceCell::const_new();
    let server = setup_test_db().await.ok()?;
    let url = SCHEMA_DATABASE
        .get_or_init(|| create_schema_database(&server))
        .await;
    Some(Database::connect(url).await.unwrap())
}
