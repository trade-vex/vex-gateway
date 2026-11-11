//! The gateway never writes the read model's `trading` schema: only Kafka Connect does, and
//! Postgres grants the gateway's role nothing but `SELECT` there (readmodel/). This test keeps it
//! so in the code too: no production file of the repository may hold a statement that writes a
//! `trading` table, nor a sea-orm entity of one, through which a write needs no SQL at all. Test
//! fixtures, in `tests` directories, may. It reads text, so a write it cannot see as such, built
//! with sea-query's builder or with a table name assembled at run time, gets past it: the grants
//! are what stop a write, this test what keeps the code from trying.

use std::fs;
use std::path::{Path, PathBuf};

/// What writes a `trading` table, in text lower-cased, without identifier quotes (or the
/// backslashes that escape them in a string) and with whitespace collapsed (`normalize`).
const WRITES: [&str; 8] = [
    "insert into trading.",
    "update trading.",
    "delete from trading.",
    "truncate trading.",
    "truncate table trading.",
    "merge into trading.",
    "copy trading.",
    "schema_name=trading",
];

fn normalize(text: &str) -> String {
    let unquoted = text.to_lowercase().replace(['"', '`', '\\'], "");
    unquoted
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" = ", "=")
}

/// Every file under `directory` that is not test code, documentation or build output.
fn production_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if !matches!(name.as_ref(), "tests" | "target" | ".git") {
                production_files(&path, files);
            }
        } else if !name.ends_with(".md") {
            files.push(path);
        }
    }
}

#[test]
fn no_production_path_writes_the_trading_schema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    production_files(&root, &mut files);

    let mut reads = 0;
    let writes: Vec<String> = files
        .iter()
        .filter_map(|path| {
            // Binary files cannot hold SQL.
            let text = normalize(&fs::read_to_string(path).ok()?);
            reads += text.matches("from trading.").count();
            let found: Vec<_> = WRITES.into_iter().filter(|w| text.contains(w)).collect();
            (!found.is_empty()).then(|| format!("{}: {found:?}", path.display()))
        })
        .collect();

    // The walk reached the repository's code: the gateway's reads of trading are in it.
    assert!(reads > 0, "no file under {} reads trading", root.display());
    assert!(
        writes.is_empty(),
        "only Kafka Connect writes the trading schema; found writes in:\n{}",
        writes.join("\n")
    );
}
