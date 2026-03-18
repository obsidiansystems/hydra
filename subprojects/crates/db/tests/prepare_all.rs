/// Validates every SQL query constant against the real hydra schema.
///
/// Runs `prepare` on each query in `queries::ALL`, which asks postgres to parse
/// and plan the statement without executing it. This catches syntax errors,
/// missing tables/columns, and type mismatches — the same class of bugs that
/// sqlx's compile-time checking caught, but at test time.
///
/// Requires `HYDRA_DATABASE_URL` (set by the meson test harness).
/// Skipped when the env var is absent so `cargo test` still works locally
/// without a running postgres.
#[tokio::test]
async fn prepare_all_queries() {
    let url = match std::env::var("HYDRA_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("HYDRA_DATABASE_URL not set, skipping prepare_all_queries");
            return;
        }
    };

    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls)
        .await
        .expect("failed to connect to test database");

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });

    let mut failures = vec![];
    for (i, sql) in db::queries::ALL.iter().enumerate() {
        if let Err(e) = client.prepare(sql).await {
            failures.push(format!("  query[{i}]: {e}\n    SQL: {sql}"));
        }
    }

    assert!(
        failures.is_empty(),
        "Failed to prepare {} queries:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
