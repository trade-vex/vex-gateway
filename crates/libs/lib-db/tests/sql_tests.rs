use lib_db::{
    error::Result,
    postgres::PostgresDb,
    traits::SqlDb,
};

/// Test helper function for basic SQL operations
async fn test_basic_sql_operations(db: &PostgresDb) -> Result<()> {
    // Create a test table
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
    
    // Insert test data
    let affected = db.execute(
        "INSERT INTO test_users (name, email, active) VALUES ('Alice', 'alice@example.com', true)"
    ).await?;
    assert_eq!(affected, 1);
    
    let affected = db.execute(
        "INSERT INTO test_users (name, email, active) VALUES ('Bob', 'bob@example.com', true)"
    ).await?;
    assert_eq!(affected, 1);
    
    // Update data
    let affected = db.execute(
        "UPDATE test_users SET active = false WHERE email = 'bob@example.com'"
    ).await?;
    assert_eq!(affected, 1);
    
    // Delete data
    let affected = db.execute(
        "DELETE FROM test_users WHERE email = 'bob@example.com'"
    ).await?;
    assert_eq!(affected, 1);
    
    Ok(())
}

/// Test helper function for SQL error handling
async fn test_sql_error_handling(db: &PostgresDb) -> Result<()> {
    // Test querying non-existent table
    let result = db.execute("SELECT COUNT(*) FROM non_existent_table").await;
    assert!(result.is_err());
    
    // Test inserting duplicate unique key
    let result = db.execute(
        "INSERT INTO test_users (name, email, active) VALUES ('Duplicate', 'alice@example.com', true)"
    ).await;
    assert!(result.is_err());
    
    // Test invalid SQL syntax
    let result = db.execute("INVALID SQL SYNTAX").await;
    assert!(result.is_err());
    
    Ok(())
}

/// Test helper function for pool management
async fn test_sql_pool_management(db: &PostgresDb) -> Result<()> {
    // Test pool statistics
    let stats = db.pool_stats();
    assert!(stats.size >= 0);
    assert!(stats.idle >= 0);
    assert!(stats.active >= 0);
    assert!(stats.max_size > 0);
    
    // Test native pool access
    let native_pool = db.native_pool();
    assert!(native_pool.size() >= 0);
    
    // Test multiple concurrent operations
    let mut handles = vec![];
    for i in 0..5 {
        let db_clone = db.clone();
        let handle = tokio::spawn(async move {
            let result = db_clone.execute(&format!(
                "INSERT INTO test_users (name, email, active) VALUES ('Concurrent{}', 'concurrent{}@example.com', true)",
                i, i
            )).await;
            result
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
        test_basic_sql_operations(&db).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_error_handling() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        test_sql_error_handling(&db).await
    } else {
        println!("DATABASE_URL not set - skipping PostgreSQL test");
        Ok(())
    }
}

#[tokio::test]
async fn test_postgres_pool_management() -> Result<()> {
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        let db = PostgresDb::connect(&postgres_url).await?;
        test_sql_pool_management(&db).await
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
    // Test connection with invalid URL
    let result = PostgresDb::connect("postgres://invalid:invalid@localhost:5432/invalid").await;
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
        
        // Test multiple operations that simulate a transaction
        // Insert multiple records
        db.execute("INSERT INTO test_users (name, email, active) VALUES ('Transaction1', 'tx1@example.com', true)").await?;
        db.execute("INSERT INTO test_users (name, email, active) VALUES ('Transaction2', 'tx2@example.com', true)").await?;
        
        // Update records
        db.execute("UPDATE test_users SET active = false WHERE name LIKE 'Transaction%'").await?;
        
        // Clean up
        db.execute("DELETE FROM test_users WHERE name LIKE 'Transaction%'").await?;
        
        Ok(())
    } else {
        println!("DATABASE_URL not set - skipping transaction test");
        Ok(())
    }
}
