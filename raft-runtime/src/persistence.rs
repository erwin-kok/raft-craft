use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use raft_core::PersistentState;

/// Abstraction over durable storage for Raft persistent state.
///
/// `save` is called synchronously inside the node's event loop whenever
/// `Action::PersistState` is emitted. It must complete before the node
/// sends any response — this is the Raft durability guarantee.
///
/// The caller is responsible for calling `load` before constructing the node
/// and passing any recovered state to `Node::new`.
pub trait Persistence: Send + Sync + 'static {
    fn save(&self, state: &PersistentState) -> Result<()>;
    fn load(&self) -> Result<Option<PersistentState>>;
}

// ── FilePersistence ───────────────────────────────────────────────────────────

/// Writes `PersistentState` as pretty-printed JSON.
///
/// Writes are atomic: content is written to `<path>.tmp` and then renamed
/// over the live file. On POSIX filesystems `rename(2)` is atomic so a crash
/// mid-write never leaves a corrupt or partial file.
pub struct FilePersistence {
    path: PathBuf,
}

impl FilePersistence {
    /// `data_dir` will be created if it does not exist.
    /// Each node should use a distinct `node_id` so files do not collide.
    pub fn new(data_dir: impl AsRef<Path>, node_id: u64) -> Result<Self> {
        let dir = data_dir.as_ref();
        fs::create_dir_all(dir)?;
        Ok(Self {
            path: dir.join(format!("node_{node_id}.json")),
        })
    }
}

impl Persistence for FilePersistence {
    fn save(&self, state: &PersistentState) -> Result<()> {
        let json = serde_json::to_vec_pretty(state)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &json)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn load(&self) -> Result<Option<PersistentState>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.path)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
}

// ── NoPersistence ─────────────────────────────────────────────────────────────

/// No-op implementation. Suitable for in-memory simulation where crash
/// recovery is not required.
pub struct NoPersistence;

impl Persistence for NoPersistence {
    fn save(&self, _: &PersistentState) -> Result<()> {
        Ok(())
    }
    fn load(&self) -> Result<Option<PersistentState>> {
        Ok(None)
    }
}
