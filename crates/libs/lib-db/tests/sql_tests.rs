use lib_db::{
    error::Result,
    postgres::{DbConfig, PostgresDb},
    traits::SqlDb,
};
use uuid::Uuid;

/// Test helper function for basic SQL operations
async fn test_basic_sql_operations(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Create a test table
    let create_table = format!(
        r#"
        CREATE TABLE IF NOT EXISTS {} (
            id SERIAL PRIMARY KEY,
            name VARCHAR(255) NOT NULL,
            email VARCHAR(255) UNIQUE NOT NULL,
            active BOOLEAN DEFAULT true,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
        )
    "#,
        table_name
    );

    db.execute(&create_table).await?;

    // Run the basic operations and ensure cleanup happens even if operations fail
    let result = run_basic_operations(db, table_name).await;

    // Always clean up the test table, even if operations failed
    let _ = db
        .execute(&format!("DROP TABLE IF EXISTS {}", table_name))
        .await;

    result
}

/// Helper function that contains the actual test logic for basic SQL operations
async fn run_basic_operations(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Insert test data
    let affected = db
        .execute(&format!(
            "INSERT INTO {} (name, email, active) VALUES ('Alice', 'alice@example.com', true)",
            table_name
        ))
        .await?;
    assert_eq!(affected, 1);

    let affected = db
        .execute(&format!(
            "INSERT INTO {} (name, email, active) VALUES ('Bob', 'bob@example.com', true)",
            table_name
        ))
        .await?;
    assert_eq!(affected, 1);

    // Update data
    let affected = db
        .execute(&format!(
            "UPDATE {} SET active = false WHERE email = 'bob@example.com'",
            table_name
        ))
        .await?;
    assert_eq!(affected, 1);

    // Delete data
    let affected = db
        .execute(&format!(
            "DELETE FROM {} WHERE email = 'bob@example.com'",
            table_name
        ))
        .await?;
    assert_eq!(affected, 1);

    Ok(())
}

/// Test helper function for SQL error handling
async fn test_sql_error_handling(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Create a test table for error testing
    let create_table = format!(
        r#"
        CREATE TABLE IF NOT EXISTS {} (
            id SERIAL PRIMARY KEY,
            name VARCHAR(255) NOT NULL,
            email VARCHAR(255) UNIQUE NOT NULL,
            active BOOLEAN DEFAULT true,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
        )
    "#,
        table_name
    );

    db.execute(&create_table).await?;

    // Run error handling tests and ensure cleanup happens even if tests fail
    let result = run_error_handling_tests(db, table_name).await;

    // Always clean up the test table, even if tests failed
    let _ = db
        .execute(&format!("DROP TABLE IF EXISTS {}", table_name))
        .await;

    result
}

/// Helper function that contains the actual error handling test logic
async fn run_error_handling_tests(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Test querying non-existent table
    let result = db.execute("SELECT COUNT(*) FROM non_existent_table").await;
    assert!(result.is_err());

    // Test inserting duplicate unique key
    // First, insert the original row and assert it succeeds
    let seed_result = db
        .execute(&format!(
            "INSERT INTO {} (name, email, active) VALUES ('Alice', 'alice@example.com', true)",
            table_name
        ))
        .await;
    assert!(seed_result.is_ok());

    // Now attempt to insert a duplicate with the same unique key (email)
    let duplicate_result = db
        .execute(&format!(
            "INSERT INTO {} (name, email, active) VALUES ('Duplicate', 'alice@example.com', true)",
            table_name
        ))
        .await;
    assert!(duplicate_result.is_err());

    // Test invalid SQL syntax
    let result = db.execute("INVALID SQL SYNTAX").await;
    assert!(result.is_err());

    Ok(())
}

/// Test helper function for pool management
async fn test_sql_pool_management(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Create a test table for pool testing
    let create_table = format!(
        r#"
        CREATE TABLE IF NOT EXISTS {} (
            id SERIAL PRIMARY KEY,
            name VARCHAR(255) NOT NULL,
            email VARCHAR(255) UNIQUE NOT NULL,
            active BOOLEAN DEFAULT true,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
        )
    "#,
        table_name
    );

    db.execute(&create_table).await?;

    // Run pool management tests and ensure cleanup happens even if tests fail
    let result = run_pool_management_tests(db, table_name).await;

    // Always clean up the test table, even if tests failed
    let _ = db
        .execute(&format!("DROP TABLE IF EXISTS {}", table_name))
        .await;

    result
}

/// Helper function that contains the actual pool management test logic
async fn run_pool_management_tests(db: &PostgresDb, table_name: &str) -> Result<()> {
    // Test pool statistics
    let stats = db.pool_stats();
    assert!(stats.max_size > 0);

    // Test multiple concurrent operations
    let mut handles = vec![];
    for i in 0..5 {
        let db_clone = db.clone();
        let table_name = table_name.to_string();
        let handle = tokio::spawn(async move {
            db_clone.execute(&format!(
                "INSERT INTO {} (name, email, active) VALUES ('Concurrent{}', 'concurrent{}@example.com', true)",
                table_name, i, i
            )).await
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    // Check pool stats after concurrent operations
    let stats_after = db.pool_stats();
    assert!(stats_after.size >= stats.size);

    Ok(())
}

/// Test helper function for generic SQL operations
async fn test_generic_sql_operations<T: SqlDb>(db: &T) -> Result<()> {
    // Test that generic functions work with any SqlDb implementation
    // This is a simplified test since we can't use specific SQL syntax
    // without knowing the database type

    // Test pool stats (available on all SqlDb implementations)
    let stats = db.pool_stats();
    assert!(stats.max_size > 0);

    Ok(())
}

#[tokio::test]
async fn test_postgres_basic_operations() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        let table_name = format!(
            "test_users_{}",
            uuid::Uuid::new_v4().to_string().replace('-', "")
        );
        test_basic_sql_operations(&db, &table_name).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_error_handling() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        let table_name = format!("test_users_{}", Uuid::new_v4().to_string().replace('-', ""));
        test_sql_error_handling(&db, &table_name).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_pool_management() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        let table_name = format!("test_users_{}", Uuid::new_v4().to_string().replace('-', ""));
        test_sql_pool_management(&db, &table_name).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_generic_operations() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        test_generic_sql_operations(&db).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_sql_trait_compatibility() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;

        // Test that PostgreSQL implements SqlDb trait
        test_generic_sql_operations(&db).await?;

        // Test pool stats (available on all SqlDb implementations)
        let stats = db.pool_stats();
        assert!(stats.max_size > 0);

        Ok(())
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_connection_handling() -> Result<()> {
    // Test connection with invalid URL using short timeout to avoid slow tests
    let short_timeout_config = DbConfig {
        max_connections: 5,
        min_connections: 1,
        acquire_timeout_ms: 500, // Short timeout for invalid connection test
        idle_timeout_secs: 60,
        max_lifetime_secs: 300,
    };
    let result = PostgresDb::with_config(
        "postgres://invalid:invalid@localhost:5432/invalid",
        short_timeout_config,
    )
    .await;
    assert!(result.is_err());

    // Test connection with valid URL (if available)
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;

        // Test that connection is working
        let stats = db.pool_stats();
        assert!(stats.max_size > 0);

        Ok(())
    } else {
        println!("DATABASE_URL not set - skipping connection test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_transaction_like_operations() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;

        // Create a test table for transaction testing
        let create_table = r#"
            CREATE TABLE IF NOT EXISTS test_users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                email VARCHAR(255) UNIQUE NOT NULL,
                active BOOLEAN DEFAULT true,
                created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
            )
        "#;

        db.execute(create_table).await?;

        // Run transaction tests and ensure cleanup happens even if tests fail
        let result = run_transaction_tests(&db).await;

        // Always clean up the test table, even if tests failed
        let _ = db.execute("DROP TABLE IF EXISTS test_users").await;

        result
    } else {
        println!("DATABASE_URL not set - skipping transaction test");
        Ok(())
    }
}

/// Helper function that contains the actual transaction test logic
async fn run_transaction_tests(db: &PostgresDb) -> Result<()> {
    // Test multiple operations that simulate a transaction
    // Insert multiple records
    db.execute("INSERT INTO test_users (name, email, active) VALUES ('Transaction1', 'tx1@example.com', true)").await?;
    db.execute("INSERT INTO test_users (name, email, active) VALUES ('Transaction2', 'tx2@example.com', true)").await?;

    // Update records
    db.execute("UPDATE test_users SET active = false WHERE name LIKE 'Transaction%'")
        .await?;

    // Clean up
    db.execute("DELETE FROM test_users WHERE name LIKE 'Transaction%'")
        .await?;

    Ok(())
}
