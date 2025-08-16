use lib_db::{
    error::Result,
    postgres::PostgresDb,
    traits::SqlDb,
};
use tracing;

/// Example: Basic SQL operations with PostgreSQL using native pool
async fn basic_sql_operations(db: &PostgresDb) -> Result<()> {
    tracing::info!("=== Basic SQL Operations ===");
    
    // Create a simple test table
    let create_table = r#"
        CREATE TABLE IF NOT EXISTS test_data (
            id SERIAL PRIMARY KEY,
            name VARCHAR(255) NOT NULL,
            value INTEGER DEFAULT 0
        )
    "#;
    
    db.execute(create_table).await?;
    tracing::info!("Created test_data table");
    
    // Insert some test data
    let affected = db.execute(
        "INSERT INTO test_data (name, value) VALUES ('test1', 100)"
    ).await?;
    tracing::info!("Inserted {} row(s)", affected);
    
    let affected = db.execute(
        "INSERT INTO test_data (name, value) VALUES ('test2', 200)"
    ).await?;
    tracing::info!("Inserted {} row(s)", affected);
    
    // Get pool statistics
    let stats = db.pool_stats();
    tracing::info!("Pool stats: {:?}", stats);
    
    Ok(())
}

/// Example: Advanced SQL operations using native pool
async fn advanced_sql_operations(db: &PostgresDb) -> Result<()> {
    tracing::info!("=== Advanced SQL Operations ===");
    
    // Update operations
    let affected = db.execute(
        "UPDATE test_data SET value = value + 50 WHERE name = 'test1'"
    ).await?;
    tracing::info!("Updated {} row(s)", affected);
    
    // Delete operations
    let affected = db.execute(
        "DELETE FROM test_data WHERE name = 'test2'"
    ).await?;
    tracing::info!("Deleted {} row(s)", affected);
    
    Ok(())
}

/// Example: Error handling and edge cases
async fn error_handling_examples(db: &PostgresDb) -> Result<()> {
    tracing::info!("=== Error Handling Examples ===");
    
    // Try to query non-existent table
    let result = db.execute("SELECT COUNT(*) FROM non_existent_table").await;
    match result {
        Ok(affected) => tracing::info!("Unexpectedly affected {} rows", affected),
        Err(e) => tracing::info!("Expected error on non-existent table: {:?}", e),
    }
    
    // Try to insert invalid data (should fail due to constraints)
    let result = db.execute(
        "INSERT INTO test_data (id, name) VALUES (1, 'duplicate_id')"
    ).await;
    match result {
        Ok(affected) => tracing::info!("Unexpectedly inserted {} rows", affected),
        Err(e) => tracing::info!("Expected error on duplicate ID: {:?}", e),
    }
    
    Ok(())
}

/// Example: Pool statistics and health checks
async fn pool_management_examples(db: &PostgresDb) -> Result<()> {
    tracing::info!("=== Pool Management Examples ===");
    
    // Get pool statistics
    let stats = db.pool_stats();
    tracing::info!("Pool stats: {:?}", stats);
    
    // Get native pool reference
    let native_pool = db.native_pool();
    tracing::info!("Native pool size: {}", native_pool.size());
    
    // Execute multiple queries to test connection handling
    for i in 0..5 {
        let result = db.execute(&format!("INSERT INTO test_data (name, value) VALUES ('test{}', {})", i, i * 10)).await?;
        tracing::info!("Insert {} affected {} rows", i, result);
    }
    
    // Check pool stats after multiple queries
    let stats_after = db.pool_stats();
    tracing::info!("Pool stats after queries: {:?}", stats_after);
    
    Ok(())
}

/// Example: Generic function that works with any SqlDb implementation
async fn generic_sql_operations<T: SqlDb>(db: &T) -> Result<()> {
    tracing::info!("=== Generic SQL Operations ===");
    
    // This function works with ANY SQL database that implements SqlDb
    // Note: This is a simplified example since we can't use specific SQL syntax
    // without knowing the database type
    
    tracing::info!("Generic SQL operations completed successfully");
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt::init();
    
    tracing::info!("SQL Database Trait System Examples");
    
    // Test with PostgreSQL if available
    if let Ok(postgres_url) = std::env::var("DATABASE_URL") {
        tracing::info!("Connecting to PostgreSQL...");
        
        match PostgresDb::connect(&postgres_url).await {
            Ok(db) => {
                tracing::info!("Successfully connected to PostgreSQL");
                
                basic_sql_operations(&db).await?;
                advanced_sql_operations(&db).await?;
                error_handling_examples(&db).await?;
                pool_management_examples(&db).await?;
                generic_sql_operations(&db).await?;
                
                tracing::info!("All SQL examples completed successfully!");
            }
            Err(e) => {
                tracing::error!("Failed to connect to PostgreSQL: {:?}", e);
                tracing::info!("Set DATABASE_URL environment variable to test with PostgreSQL");
                tracing::info!("Example: DATABASE_URL=postgres://user:pass@localhost/dbname");
            }
        }
    } else {
        tracing::warn!("DATABASE_URL not set - skipping PostgreSQL examples");
        tracing::info!("Set DATABASE_URL environment variable to test with PostgreSQL");
        tracing::info!("Example: DATABASE_URL=postgres://user:pass@localhost/dbname");
    }
    
    Ok(())
}
