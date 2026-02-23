//! Database skill for SQLite queries
//!
//! Provides:
//! - SQLite query execution
//! - Result formatting as tables
//! - Safe query execution (configurable restrictions)
//! - Connection to local database files

use anyhow::{Result, bail};
use std::collections::HashMap;
use rusqlite::{Connection, params_from_iter};

use super::super::registry::{
    Skill, SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType,
    SkillResult, SkillContext,
};

/// Create the database skill
pub fn create_skill() -> Skill {
    let meta = SkillMeta {
        id: "builtin-database".to_string(),
        name: "Database".to_string(),
        description: "Execute SQLite database queries with result formatting".to_string(),
        version: "1.0.0".to_string(),
        author: Some("my-agent".to_string()),
        category: SkillCategory::Data,
        permissions: vec![Permission::ReadFiles, Permission::WriteFiles],
        parameters: vec![
            SkillParameter {
                name: "operation".to_string(),
                param_type: ParameterType::Enum,
                required: true,
                default: None,
                description: "Operation to perform".to_string(),
                allowed_values: Some(vec![
                    "query".to_string(),
                    "execute".to_string(),
                    "tables".to_string(),
                    "schema".to_string(),
                    "create".to_string(),
                ]),
            },
            SkillParameter {
                name: "database".to_string(),
                param_type: ParameterType::Path,
                required: false,
                default: Some("~/.local/share/my-agent/memory.db".to_string()),
                description: "Path to SQLite database file".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "sql".to_string(),
                param_type: ParameterType::String,
                required: false,
                default: None,
                description: "SQL query to execute".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "read_only".to_string(),
                param_type: ParameterType::Boolean,
                required: false,
                default: Some("true".to_string()),
                description: "Restrict to read-only operations (SELECT)".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "limit".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("100".to_string()),
                description: "Maximum rows to return".to_string(),
                allowed_values: None,
            },
        ],
        builtin: true,
        tags: vec!["database".to_string(), "sqlite".to_string(), "sql".to_string(), "query".to_string()],
    };

    Skill::new(meta, execute_database)
}

/// Execute database operations
fn execute_database(
    params: HashMap<String, String>,
    ctx: &SkillContext,
) -> Result<SkillResult> {
    let operation = params.get("operation")
        .ok_or_else(|| anyhow::anyhow!("Missing 'operation' parameter"))?;

    // Check approval for write operations
    let read_only: bool = params.get("read_only")
        .and_then(|s| s.parse().ok())
        .unwrap_or(true);

    if !read_only && ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Write operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    match operation.as_str() {
        "query" => execute_query(&params, read_only),
        "execute" => execute_statement(&params, ctx),
        "tables" => list_tables(&params),
        "schema" => show_schema(&params),
        "create" => create_database(&params),
        _ => bail!("Unknown operation: {}", operation),
    }
}

/// Execute a SELECT query
fn execute_query(params: &HashMap<String, String>, read_only: bool) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let sql = params.get("sql")
        .ok_or_else(|| anyhow::anyhow!("Missing 'sql' parameter"))?;

    // Validate query for read-only mode
    if read_only && !is_read_only_query(sql) {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Query is not read-only (SELECT only in read-only mode)".to_string()),
            duration_ms: 0,
        });
    }

    let database = resolve_database_path(params)?;

    // Open connection
    let conn = Connection::open(&database)?;

    // Add LIMIT if not present
    let limit: usize = params.get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let sql_with_limit = if sql.to_uppercase().contains(" LIMIT ") {
        sql.to_string()
    } else {
        format!("{} LIMIT {}", sql.trim_end_matches(';'), limit)
    };

    // Execute query
    let result = execute_select(&conn, &sql_with_limit, limit);

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        }),
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Query failed: {}", e)),
            duration_ms,
        }),
    }
}

/// Execute a statement (INSERT, UPDATE, DELETE, etc.)
fn execute_statement(params: &HashMap<String, String>, ctx: &SkillContext) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let sql = params.get("sql")
        .ok_or_else(|| anyhow::anyhow!("Missing 'sql' parameter"))?;

    // Always require approval for write operations
    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Execute operations require approval".to_string()),
            duration_ms: 0,
        });
    }

    let database = resolve_database_path(params)?;

    // Open connection
    let conn = Connection::open(&database)?;

    // Execute statement
    let result = conn.execute_batch(sql);

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(_) => {
            let changes = conn.changes();
            Ok(SkillResult {
                success: true,
                output: format!("Statement executed. {} row(s) affected.", changes),
                error: None,
                duration_ms,
            })
        }
        Err(e) => Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Execute failed: {}", e)),
            duration_ms,
        }),
    }
}

/// List all tables in the database
fn list_tables(params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let database = resolve_database_path(params)?;

    if !std::path::Path::new(&database).exists() {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Database not found: {}", database.display())),
            duration_ms: 0,
        });
    }

    let conn = Connection::open(&database)?;

    let mut tables: Vec<String> = Vec::new();

    conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
        .query_map([], |row| row.get(0))?
        .for_each(|name| {
            if let Ok(name) = name {
                tables.push(name);
            }
        });

    let duration_ms = start.elapsed().as_millis() as u64;

    if tables.is_empty() {
        Ok(SkillResult {
            success: true,
            output: "No tables found in database.".to_string(),
            error: None,
            duration_ms,
        })
    } else {
        let output = format!("Tables in {}:\n\n{}\n",
            database.display(),
            tables.iter().map(|t| format!("- {}", t)).collect::<Vec<_>>().join("\n")
        );
        Ok(SkillResult {
            success: true,
            output,
            error: None,
            duration_ms,
        })
    }
}

/// Show schema for a table or all tables
fn show_schema(params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let database = resolve_database_path(params)?;

    if !std::path::Path::new(&database).exists() {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Database not found: {}", database.display())),
            duration_ms: 0,
        });
    }

    let conn = Connection::open(&database)?;

    let table_name = params.get("table");

    let mut schema_parts: Vec<String> = Vec::new();

    let sql = if let Some(table) = table_name {
        format!("SELECT sql FROM sqlite_master WHERE type='table' AND name='{}'", table)
    } else {
        "SELECT sql FROM sqlite_master WHERE type IN ('table', 'index') AND sql IS NOT NULL".to_string()
    };

    conn.prepare(&sql)?
        .query_map([], |row| row.get::<_, String>(0))?
        .for_each(|stmt| {
            if let Ok(stmt) = stmt {
                schema_parts.push(stmt);
            }
        });

    let duration_ms = start.elapsed().as_millis() as u64;

    if schema_parts.is_empty() {
        Ok(SkillResult {
            success: true,
            output: "No schema information found.".to_string(),
            error: None,
            duration_ms,
        })
    } else {
        Ok(SkillResult {
            success: true,
            output: schema_parts.join("\n\n"),
            error: None,
            duration_ms,
        })
    }
}

/// Create a new database
fn create_database(params: &HashMap<String, String>) -> Result<SkillResult> {
    let start = std::time::Instant::now();

    let database = resolve_database_path(params)?;

    if std::path::Path::new(&database).exists() {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(format!("Database already exists: {}", database.display())),
            duration_ms: 0,
        });
    }

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(&database).parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create empty database
    let _conn = Connection::open(&database)?;

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(SkillResult {
        success: true,
        output: format!("Database created: {}", database.display()),
        error: None,
        duration_ms,
    })
}

// ============================================================================
// Helper functions
// ============================================================================

/// Resolve database path (expand ~)
fn resolve_database_path(params: &HashMap<String, String>) -> Result<std::path::PathBuf> {
    let path = params.get("database")
        .map(|s| s.as_str())
        .unwrap_or("~/.local/share/my-agent/memory.db");

    if path.starts_with('~') {
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
        Ok(home.join(path.trim_start_matches('~').trim_start_matches('/')))
    } else {
        Ok(std::path::PathBuf::from(path))
    }
}

/// Check if a query is read-only (SELECT)
fn is_read_only_query(sql: &str) -> bool {
    let sql_upper = sql.to_uppercase().trim_start().to_string();

    // Allow SELECT and EXPLAIN
    if sql_upper.starts_with("SELECT") || sql_upper.starts_with("EXPLAIN") {
        // But not with INTO (SELECT ... INTO creates a table)
        !sql_upper.contains(" INTO ")
    } else {
        false
    }
}

/// Execute a SELECT query and format results as a table
fn execute_select(conn: &Connection, sql: &str, limit: usize) -> Result<String> {
    let mut stmt = conn.prepare(sql)?;

    // Get column names
    let column_names: Vec<String> = stmt.column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    if column_names.is_empty() {
        return Ok("Query returned no columns.".to_string());
    }

    // Collect rows
    let mut rows_data: Vec<Vec<String>> = Vec::new();
    let mut row_count = 0;

    let columns = stmt.column_count();
    let rows = stmt.query([])?;

    // We need to iterate manually since query returns a Rows iterator
    // that can't be used with for loops in the same way
    use rusqlite::Rows;
    let mut rows = rows;

    while let Some(row_result) = rows.next()? {
        if row_count >= limit {
            break;
        }

        let mut row_data = Vec::with_capacity(columns);
        for i in 0..columns {
            let value: String = row_result.get::<_, Option<String>>(i)?
                .unwrap_or_else(|| "NULL".to_string());
            row_data.push(value);
        }
        rows_data.push(row_data);
        row_count += 1;
    }

    // Format as table
    format_table(&column_names, &rows_data, row_count)
}

/// Format results as an ASCII table
fn format_table(headers: &[String], rows: &[Vec<String>], total_rows: usize) -> Result<String> {
    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Cap column width
    let max_width = 50;
    for w in &mut widths {
        *w = (*w).min(max_width);
    }

    // Build output
    let mut output = String::new();

    // Header separator
    let separator: String = widths.iter()
        .map(|w| "-".repeat(*w + 2))
        .collect::<Vec<_>>()
        .join("+");

    output.push_str(&format!("+{}\n", separator));

    // Headers
    let header_row: String = headers.iter()
        .enumerate()
        .map(|(i, h)| format!(" {:width$} ", h, width = widths.get(i).copied().unwrap_or(10)))
        .collect::<Vec<_>>()
        .join("|");

    output.push_str(&format!("|{}|\n", header_row));
    output.push_str(&format!("+{}\n", separator));

    // Data rows
    for row in rows {
        let row_str: String = row.iter()
            .enumerate()
            .map(|(i, cell)| {
                let truncated = if cell.len() > max_width {
                    format!("{}...", &cell[..max_width - 3])
                } else {
                    cell.clone()
                };
                format!(" {:width$} ", truncated, width = widths.get(i).copied().unwrap_or(10))
            })
            .collect::<Vec<_>>()
            .join("|");

        output.push_str(&format!("|{}|\n", row_str));
    }

    output.push_str(&format!("+{}\n", separator));

    // Summary
    output.push_str(&format!("\n{} row(s) returned.\n", total_rows));

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_create_skill() {
        let skill = create_skill();
        assert_eq!(skill.meta.id, "builtin-database");
        assert_eq!(skill.meta.category, SkillCategory::Data);
    }

    #[test]
    fn test_is_read_only_query() {
        assert!(is_read_only_query("SELECT * FROM users"));
        assert!(is_read_only_query("SELECT id, name FROM products WHERE active = 1"));
        assert!(is_read_only_query("  SELECT * FROM table  "));
        assert!(is_read_only_query("EXPLAIN SELECT * FROM users"));

        assert!(!is_read_only_query("INSERT INTO users VALUES (1, 'test')"));
        assert!(!is_read_only_query("UPDATE users SET name = 'test'"));
        assert!(!is_read_only_query("DELETE FROM users"));
        assert!(!is_read_only_query("DROP TABLE users"));
        assert!(!is_read_only_query("SELECT * INTO new_table FROM users"));
    }

    #[test]
    fn test_resolve_database_path() {
        let mut params = HashMap::new();
        params.insert("database".to_string(), "/tmp/test.db".to_string());

        let path = resolve_database_path(&params).unwrap();
        assert_eq!(path, std::path::PathBuf::from("/tmp/test.db"));
    }

    #[test]
    fn test_format_table() {
        let headers = vec!["ID".to_string(), "Name".to_string()];
        let rows = vec![
            vec!["1".to_string(), "Alice".to_string()],
            vec!["2".to_string(), "Bob".to_string()],
        ];

        let output = format_table(&headers, &rows, 2).unwrap();
        assert!(output.contains("ID"));
        assert!(output.contains("Name"));
        assert!(output.contains("Alice"));
        assert!(output.contains("Bob"));
        assert!(output.contains("2 row(s)"));
    }

    #[test]
    fn test_tables_operation() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        // Create a temp database
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY)", []).unwrap();
        drop(conn);

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "tables".to_string());
        params.insert("database".to_string(), db_path);

        let result = skill.execute(params, &ctx).unwrap();
        assert!(result.success);
        assert!(result.output.contains("test"));
    }

    #[test]
    fn test_read_only_enforcement() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "query".to_string());
        params.insert("sql".to_string(), "DELETE FROM users".to_string());
        params.insert("read_only".to_string(), "true".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("read-only"));
    }

    #[test]
    fn test_missing_sql() {
        let skill = create_skill();
        let ctx = SkillContext::default();

        let mut params = HashMap::new();
        params.insert("operation".to_string(), "query".to_string());
        // Missing SQL

        let result = skill.execute(params, &ctx);
        assert!(result.is_err() || !result.unwrap().success);
    }
}