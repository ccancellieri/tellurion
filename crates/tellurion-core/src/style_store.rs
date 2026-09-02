//! `StyleStore` mirrors `ConfigStore`: style documents (MapLibre Style JSON)
//! are config-like — read once at startup and rarely thereafter (changing a
//! style is an operator action, not a request-path event), so `load`/`list`
//! stay synchronous. There is no benefit to an async signature here and every
//! call site (the styles/tiles protocol crates) stays simpler without one. A
//! future DB- or Valkey-backed store can still do blocking I/O inside `load`
//! (or spawn its own runtime) without changing this trait.

use std::collections::HashMap;

use crate::config::StyleRef;
use crate::error::{Error, Result};

pub trait StyleStore: Send + Sync {
    /// `Ok(None)` means the id is not registered; `Err` means the id is
    /// registered but the store failed to produce a document for it (e.g. a
    /// missing or unparseable file).
    fn load(&self, id: &str) -> Result<Option<serde_json::Value>>;

    /// Every registered style id, in no particular guaranteed order beyond
    /// implementation stability.
    fn list(&self) -> Result<Vec<String>>;
}

/// Reads MapLibre Style JSON documents from the paths declared in
/// `AppConfig.styles` once, at construction, and serves them from an
/// in-memory cache thereafter — `load` never touches the filesystem or
/// re-parses JSON on the request path, matching this module's own "read
/// once at startup" contract instead of merely documenting it. A bad `path`
/// only fails `load` for the id that points at it (the read/parse error is
/// captured once and replayed on every call) — `list` always reflects every
/// registered id regardless of whether its file was readable when the store
/// was built.
pub struct FileStyleStore {
    docs: HashMap<String, std::result::Result<serde_json::Value, String>>,
}

impl FileStyleStore {
    pub fn new(styles: &[StyleRef]) -> Self {
        let docs = styles
            .iter()
            .map(|style| (style.id.clone(), Self::read(&style.id, &style.path)))
            .collect();
        Self { docs }
    }

    fn read(id: &str, path: &str) -> std::result::Result<serde_json::Value, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|source| format!("reading style '{id}' at '{path}': {source}"))?;
        serde_json::from_str(&contents)
            .map_err(|source| format!("parsing style '{id}' at '{path}': {source}"))
    }
}

impl StyleStore for FileStyleStore {
    fn load(&self, id: &str) -> Result<Option<serde_json::Value>> {
        match self.docs.get(id) {
            None => Ok(None),
            Some(Ok(document)) => Ok(Some(document.clone())),
            Some(Err(message)) => Err(Error::Config(message.clone())),
        }
    }

    fn list(&self) -> Result<Vec<String>> {
        let mut ids: Vec<String> = self.docs.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Nanosecond timestamps alone can collide between threads running
    /// concurrent `#[test]`s in the same process; the counter guarantees a
    /// unique path per call regardless of clock resolution.
    static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

    fn write_temp_json(contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-core-style-test-{}-{}.json",
            std::process::id(),
            NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_a_registered_style() {
        let path = write_temp_json(r#"{"version": 8, "layers": []}"#);
        let store = FileStyleStore::new(&[StyleRef {
            id: "basic".to_string(),
            path: path.to_string_lossy().to_string(),
        }]);

        let doc = store.load("basic").unwrap().unwrap();
        assert_eq!(doc["version"], 8);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_id_is_ok_none() {
        let store = FileStyleStore::new(&[]);
        assert_eq!(store.load("missing").unwrap(), None);
    }

    #[test]
    fn registered_id_with_missing_file_is_an_error() {
        let store = FileStyleStore::new(&[StyleRef {
            id: "basic".to_string(),
            path: "/nonexistent/path/style.json".to_string(),
        }]);
        assert!(matches!(store.load("basic"), Err(Error::Config(_))));
    }

    #[test]
    fn registered_id_with_invalid_json_is_an_error() {
        let path = write_temp_json("not valid json");
        let store = FileStyleStore::new(&[StyleRef {
            id: "basic".to_string(),
            path: path.to_string_lossy().to_string(),
        }]);
        assert!(matches!(store.load("basic"), Err(Error::Config(_))));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn list_returns_sorted_registered_ids() {
        let store = FileStyleStore::new(&[
            StyleRef {
                id: "dark".to_string(),
                path: "styles/dark.json".to_string(),
            },
            StyleRef {
                id: "basic".to_string(),
                path: "styles/basic.json".to_string(),
            },
        ]);
        assert_eq!(store.list().unwrap(), vec!["basic", "dark"]);
    }

    #[test]
    fn list_is_empty_when_no_styles_registered() {
        let store = FileStyleStore::new(&[]);
        assert!(store.list().unwrap().is_empty());
    }
}
