//! One read-only resolver for the exact local model layout the Candle loader supports.
use std::path::{Path, PathBuf};
const REPOSITORY: &str = "models--nomic-ai--nomic-embed-text-v1.5";

fn complete(snapshot: &Path) -> bool {
    ["config.json", "model.safetensors", "tokenizer.json"]
        .iter()
        .all(|name| snapshot.join(name).is_file())
}

fn resolve_repository(repo: &Path) -> Option<PathBuf> {
    let snapshots = repo.join("snapshots");
    if let Ok(revision) = std::fs::read_to_string(repo.join("refs/main")) {
        let revision = revision.trim();
        // A revision is a single cache directory identity, never an arbitrary path.
        if !revision.is_empty()
            && !revision.contains(['/', '\\'])
            && revision != "."
            && revision != ".."
        {
            let snapshot = snapshots.join(revision);
            if complete(&snapshot) {
                return Some(snapshot);
            }
        }
    }
    let mut candidates = std::fs::read_dir(&snapshots)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && complete(path));
    let first = candidates.next();
    if candidates.next().is_some() {
        return None;
    } // ambiguous snapshots need a valid ref
    first.or_else(|| complete(&snapshots).then_some(snapshots))
}

pub fn resolve_cached_snapshot(hf_cache: &Path, legacy_cache: &Path) -> Option<PathBuf> {
    [hf_cache, legacy_cache]
        .into_iter()
        .find_map(|root| resolve_repository(&root.join(REPOSITORY)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(root: &Path, revision: &str) -> PathBuf {
        let path = root.join(REPOSITORY).join("snapshots").join(revision);
        std::fs::create_dir_all(&path).unwrap();
        for file in ["config.json", "model.safetensors", "tokenizer.json"] {
            std::fs::write(path.join(file), "fixture").unwrap();
        }
        path
    }
    #[test]
    fn resolves_actual_hf_or_legacy_location_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let hf = dir.path().join("hf");
        let legacy = dir.path().join("legacy");
        let old = snapshot(&legacy, "abc");
        assert_eq!(resolve_cached_snapshot(&hf, &legacy), Some(old));
        let new = snapshot(&hf, "def");
        assert_eq!(resolve_cached_snapshot(&hf, &legacy), Some(new.clone()));
        assert!(!hf.join(REPOSITORY).join("refs").exists());
        std::fs::create_dir_all(hf.join(REPOSITORY).join("refs")).unwrap();
        for value in ["", "missing", "../../escape"] {
            std::fs::write(hf.join(REPOSITORY).join("refs/main"), value).unwrap();
            assert_eq!(resolve_cached_snapshot(&hf, &legacy), Some(new.clone()));
            assert_eq!(
                std::fs::read_to_string(hf.join(REPOSITORY).join("refs/main")).unwrap(),
                value
            );
        }
    }
    #[test]
    fn ambiguous_partial_and_sharded_only_caches_are_not_claimed_loadable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let one = snapshot(dir.path(), "one");
        let _two = snapshot(dir.path(), "two");
        assert!(resolve_cached_snapshot(dir.path(), &missing).is_none());
        std::fs::create_dir_all(dir.path().join(REPOSITORY).join("refs")).unwrap();
        std::fs::write(dir.path().join(REPOSITORY).join("refs/main"), "one").unwrap();
        assert_eq!(
            resolve_cached_snapshot(dir.path(), &missing),
            Some(one.clone())
        );
        std::fs::remove_file(one.join("model.safetensors")).unwrap();
        std::fs::write(one.join("model-00001-of-00002.safetensors"), "partial").unwrap();
        std::fs::remove_dir_all(dir.path().join(REPOSITORY).join("snapshots/two")).unwrap();
        assert!(resolve_cached_snapshot(dir.path(), &missing).is_none());
    }
    #[test]
    fn supports_complete_direct_snapshot_layout() {
        let dir = tempfile::tempdir().unwrap();
        let direct = snapshot(dir.path(), "");
        assert_eq!(
            resolve_cached_snapshot(dir.path(), dir.path()),
            Some(direct)
        );
    }
}
