use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::parser::FileSymbols;

#[derive(Serialize, Deserialize)]
struct CachedFile {
    mtime_secs: u64,
    symbols: FileSymbols,
}

pub struct Index {
    db: sled::Db,
}

impl Index {
    pub fn open(repo_root: &Path) -> Result<Self> {
        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let cache_dir = dirs_cache_path(&repo_name);
        std::fs::create_dir_all(&cache_dir).ok();

        let db = sled::open(&cache_dir).with_context(|| {
            format!("opening index at {}", cache_dir.display())
        })?;
        Ok(Index { db })
    }

    pub fn get(&self, path: &Path, current_mtime: u64) -> Option<FileSymbols> {
        let key = path.to_string_lossy();
        if let Ok(Some(data)) = self.db.get(key.as_bytes()) {
            if let Ok(cached) = bincode::deserialize::<CachedFile>(&data) {
                if cached.mtime_secs == current_mtime {
                    return Some(cached.symbols);
                }
            }
        }
        None
    }

    pub fn insert(&self, path: &Path, mtime: u64, syms: FileSymbols) {
        let key = path.to_string_lossy();
        let cached = CachedFile {
            mtime_secs: mtime,
            symbols: syms,
        };
        if let Ok(data) = bincode::serialize(&cached) {
            let _ = self.db.insert(key.as_bytes(), data);
        }
    }
}

fn dirs_cache_path(repo_name: &str) -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join(".cache").join("pyrisk").join(repo_name).join("index")
}

pub fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e)
        }))
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
