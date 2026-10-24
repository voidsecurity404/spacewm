//! Changing one setting in the config file from inside spacewm (the settings box), the
//! way a person would: only that value changes; every comment, blank line and the
//! order of keys stay as they were (toml_edit keeps the file's formatting).
//!
//! The result is checked like any edit (`parse`) before it is written, and written
//! atomically (a temp file renamed over the old one), so the file is never left broken
//! or half-written. The file watcher then applies it like a hand edit.

use std::path::Path;

use toml_edit::{DocumentMut, Item, Table, Value};

/// Set `table.key = value` in the config text and return the new text.
pub fn set_in_text(text: &str, table: &str, key: &str, value: Value) -> Result<String, String> {
    let mut doc: DocumentMut =
        text.parse().map_err(|e| format!("the config file has a syntax error: {e}"))?;
    if !doc.contains_key(table) {
        doc.insert(table, Item::Table(Table::new()));
    }
    let t = doc[table].as_table_mut().ok_or_else(|| format!("[{table}] is not a table"))?;
    match t.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            // Keep the comment after the value ("fov = 75.0  # wide").
            let decor = old.decor().clone();
            *old = value;
            *old.decor_mut() = decor;
        }
        None => {
            t.insert(key, Item::Value(value));
        }
    }
    let new = doc.to_string();
    super::parse(&new).map_err(|problems| {
        problems.first().map_or_else(|| "invalid value".to_string(), |p| p.message.clone())
    })?;
    Ok(new)
}

/// Set one value in the file at `path` (created from the defaults if missing).
pub fn set_value(path: &Path, table: &str, key: &str, value: Value) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => super::DEFAULT_CONFIG.to_string(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let new = set_in_text(&text, table, key, value)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, new).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
    tracing::info!(path = %path.display(), setting = format!("{table}.{key}"), "config file updated");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_value_changes() {
        let text = "# my settings\n[render]\n# how wide\nfov = 75.0  # degrees\nfog = true\n";
        let new = set_in_text(text, "render", "fov", Value::from(90.0)).expect("ok");
        assert_eq!(new, "# my settings\n[render]\n# how wide\nfov = 90.0  # degrees\nfog = true\n");
    }

    #[test]
    fn missing_keys_and_tables_are_added() {
        let new = set_in_text("[render]\nfog = true\n", "input", "invert_y", Value::from(true))
            .expect("ok");
        assert!(new.contains("[input]") && new.contains("invert_y = true"), "{new}");
        assert!(crate::config::parse(&new).expect("valid").input.invert_y);
    }

    #[test]
    fn invalid_values_are_refused() {
        let err = set_in_text("", "render", "fov", Value::from(500.0)).expect_err("out of range");
        assert!(err.contains("30") || err.contains("120"), "{err}");
        assert!(set_in_text("[render\n", "render", "fov", Value::from(80.0)).is_err());
    }

    #[test]
    fn the_shipped_default_file_round_trips() {
        let new = set_in_text(
            crate::config::DEFAULT_CONFIG,
            "desktop",
            "lock_after_minutes",
            Value::from(5),
        )
        .expect("ok");
        assert_eq!(crate::config::parse(&new).expect("valid").desktop.lock_after_minutes, 5);
        // Every other line is untouched.
        let changed =
            crate::config::DEFAULT_CONFIG.lines().zip(new.lines()).filter(|(a, b)| a != b).count();
        assert_eq!(changed, 1);
    }
}
