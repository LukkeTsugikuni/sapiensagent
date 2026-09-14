use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub timestamp: u64,
    pub session: String,
    pub text: String,
}
#[derive(Clone)]
pub struct MemoryStore {
    path: PathBuf,
    retention_days: u64,
}
impl MemoryStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        Self::open_with_retention(path, 0)
    }

    pub fn open_with_retention(path: PathBuf, retention_days: u64) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            fs::File::create(&path)?;
        }
        Ok(Self {
            path,
            retention_days,
        })
    }
    pub fn append(&self, session: &str, text: &str) -> Result<()> {
        let item = MemoryItem {
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            session: session.into(),
            text: crate::observability::redact(text),
        };
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&item)?)?;
        Ok(())
    }
    pub fn search(&self, query: &str) -> Result<Vec<MemoryItem>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|item| item.text.to_lowercase().contains(&query.to_lowercase()))
            .collect())
    }
    pub fn list(&self) -> Result<Vec<MemoryItem>> {
        let file = fs::File::open(&self.path)?;
        let cutoff = (self.retention_days > 0).then(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| {
                    duration
                        .as_secs()
                        .saturating_sub(self.retention_days.saturating_mul(86_400))
                })
                .unwrap_or_default()
        });
        BufReader::new(file)
            .lines()
            .filter(|line| {
                line.as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(true)
            })
            .map(|line| Ok(serde_json::from_str::<MemoryItem>(&line?)?))
            .filter(|item| {
                item.as_ref()
                    .map(|item| cutoff.is_none_or(|cutoff| item.timestamp >= cutoff))
                    .unwrap_or(true)
            })
            .collect()
    }
    pub fn delete_session(&self, session: &str) -> Result<usize> {
        let before = self.list()?;
        let removed = before.iter().filter(|item| item.session == session).count();
        let remaining: Vec<_> = before
            .into_iter()
            .filter(|item| item.session != session)
            .collect();
        self.replace(&remaining)?;
        Ok(removed)
    }
    pub fn clear(&self) -> Result<usize> {
        let count = self.list()?.len();
        self.replace(&[])?;
        Ok(count)
    }
    fn replace(&self, items: &[MemoryItem]) -> Result<()> {
        let temporary = self.path.with_extension("jsonl.tmp");
        let mut file = fs::File::create(&temporary)?;
        for item in items {
            writeln!(file, "{}", serde_json::to_string(item)?)?;
        }
        file.sync_all()?;
        drop(file);
        if fs::rename(&temporary, &self.path).is_err() {
            fs::write(
                &self.path,
                items
                    .iter()
                    .map(|item| serde_json::to_string(item).unwrap() + "\n")
                    .collect::<String>(),
            )?;
            let _ = fs::remove_file(temporary);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (std::path::PathBuf, MemoryStore) {
        let directory = std::env::temp_dir().join(format!(
            "sapiens-memory-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let store = MemoryStore::open(directory.join("memory.jsonl")).expect("store");
        (directory, store)
    }

    #[test]
    fn search_and_delete_are_scoped_to_a_session() {
        let (directory, store) = store();
        store.append("a", "alpha").unwrap();
        store.append("b", "alpha").unwrap();
        assert_eq!(store.search("alpha").unwrap().len(), 2);
        assert_eq!(store.delete_session("a").unwrap(), 1);
        assert_eq!(store.search("alpha").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn retention_filters_expired_items() {
        let (directory, store) = store();
        let path = directory.join("memory.jsonl");
        std::fs::write(
            &path,
            "{\"timestamp\":0,\"session\":\"old\",\"text\":\"old\"}\n",
        )
        .unwrap();
        let retained = MemoryStore::open_with_retention(path, 1).unwrap();
        assert!(retained.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(directory);
        let _ = store;
    }
}
