use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub channel: String,
    pub identity: String,
    pub agent: String,
    pub workspace: PathBuf,
    pub cancelled: bool,
    pub updated_at: u64,
}

pub trait SessionStore: Send + Sync {
    fn get(&self, id: &str) -> Option<Session>;
    fn save(&mut self, session: Session);
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SessionFile {
    sessions: HashMap<String, Session>,
}

pub struct FileSessionStore {
    path: PathBuf,
    file: SessionFile,
}

impl FileSessionStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = if path.exists() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("read sessions {}", path.display()))?;
            serde_json::from_str(&text)
                .with_context(|| format!("parse sessions {}", path.display()))?
        } else {
            SessionFile::default()
        };
        Ok(Self { path, file })
    }

    pub fn ensure(
        &mut self,
        id: &str,
        channel: &str,
        identity: &str,
        agent: &str,
        workspace: &Path,
    ) -> Result<Session> {
        if id.trim().is_empty() {
            bail!("session id cannot be empty");
        }
        let session = self
            .file
            .sessions
            .entry(id.to_string())
            .or_insert_with(|| Session {
                id: id.to_string(),
                channel: channel.to_string(),
                identity: identity.to_string(),
                agent: agent.to_string(),
                workspace: workspace.to_path_buf(),
                cancelled: false,
                updated_at: now(),
            });
        session.channel = channel.to_string();
        session.identity = identity.to_string();
        session.agent = agent.to_string();
        session.workspace = workspace.to_path_buf();
        session.updated_at = now();
        let result = session.clone();
        self.persist()?;
        Ok(result)
    }

    pub fn cancel(&mut self, id: &str) -> Result<bool> {
        let Some(session) = self.file.sessions.get_mut(id) else {
            return Ok(false);
        };
        session.cancelled = true;
        session.updated_at = now();
        self.persist()?;
        Ok(true)
    }

    pub fn list(&self) -> Vec<Session> {
        let mut sessions = self.file.sessions.values().cloned().collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        sessions
    }

    pub fn cleanup_before(&mut self, cutoff: u64) -> Result<usize> {
        let before = self.file.sessions.len();
        self.file
            .sessions
            .retain(|_, session| session.updated_at >= cutoff);
        let removed = before.saturating_sub(self.file.sessions.len());
        if removed > 0 {
            self.persist()?;
        }
        Ok(removed)
    }

    pub fn rotate(&mut self, id: &str) -> Result<Session> {
        let current = self
            .file
            .sessions
            .get(id)
            .cloned()
            .with_context(|| format!("session not found: {id}"))?;
        let new_id = format!("{}:rotated:{}", id, now());
        let rotated = Session {
            id: new_id.clone(),
            channel: current.channel,
            identity: current.identity,
            agent: current.agent,
            workspace: current.workspace,
            cancelled: false,
            updated_at: now(),
        };
        self.file.sessions.remove(id);
        self.file.sessions.insert(new_id, rotated.clone());
        self.persist()?;
        Ok(rotated)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, serde_json::to_vec_pretty(&self.file)?)?;
        Ok(())
    }
}

impl SessionStore for FileSessionStore {
    fn get(&self, id: &str) -> Option<Session> {
        self.file.sessions.get(id).cloned()
    }

    fn save(&mut self, session: Session) {
        self.file.sessions.insert(session.id.clone(), session);
        let _ = self.persist();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_persists_and_cancels_a_scoped_session() {
        let root =
            std::env::temp_dir().join(format!("sapiens-session-test-{}", std::process::id()));
        let path = root.join("sessions.json");
        let mut store = FileSessionStore::open(&path).expect("open");
        let session = store
            .ensure(
                "webchat:alice",
                "webchat",
                "alice",
                "default",
                Path::new("workspace"),
            )
            .expect("ensure");
        assert_eq!(session.identity, "alice");
        assert!(store.cancel("webchat:alice").expect("cancel"));
        assert!(store.get("webchat:alice").expect("get").cancelled);
        let reopened = FileSessionStore::open(&path).expect("reopen");
        assert!(reopened.get("webchat:alice").expect("persisted").cancelled);
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn cleanup_and_rotate_are_persisted() {
        let root = std::env::temp_dir().join(format!(
            "sapiens-session-rotation-test-{}",
            std::process::id()
        ));
        let path = root.join("sessions.json");
        let mut store = FileSessionStore::open(&path).expect("open");
        store
            .ensure("old", "cli", "a", "agent", Path::new("workspace"))
            .expect("old");
        store
            .ensure("keep", "cli", "b", "agent", Path::new("workspace"))
            .expect("keep");
        let old = store.get("old").expect("old session");
        assert_eq!(
            store.cleanup_before(old.updated_at + 1).expect("cleanup"),
            2
        );

        store
            .ensure("active", "cli", "c", "agent", Path::new("workspace"))
            .expect("active");
        let rotated = store.rotate("active").expect("rotate");
        assert!(store.get("active").is_none());
        assert!(store.get(&rotated.id).is_some());
        let reopened = FileSessionStore::open(&path).expect("reopen");
        assert!(reopened.get(&rotated.id).is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_gateway_style_ensures_are_serialized_and_reopenable() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let root = std::env::temp_dir().join(format!(
            "sapiens-session-concurrency-test-{}",
            std::process::id()
        ));
        let path = root.join("sessions.json");
        let store = Arc::new(Mutex::new(FileSessionStore::open(&path).expect("open")));
        let mut workers = Vec::new();
        for index in 0..8 {
            let store = Arc::clone(&store);
            workers.push(thread::spawn(move || {
                let mut store = store.lock().expect("session lock");
                store
                    .ensure(
                        &format!("webchat:user-{index}"),
                        "webchat",
                        &format!("user-{index}"),
                        "sapiens-agent",
                        Path::new("workspace"),
                    )
                    .expect("ensure")
            }));
        }
        for worker in workers {
            worker.join().expect("worker");
        }
        assert_eq!(store.lock().expect("session lock").list().len(), 8);
        let reopened = FileSessionStore::open(&path).expect("reopen");
        assert_eq!(reopened.list().len(), 8);
        let _ = fs::remove_dir_all(root);
    }
}
