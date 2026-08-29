use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use wustite::executable::ExecutableFunction;
use wustite::profiler::ProfileArtifact;

const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProfileCacheStatus {
    Disabled,
    Miss,
    Hit,
    Rejected,
    Error,
}

pub(super) struct ProfileCache {
    directory: Option<PathBuf>,
    path: Option<PathBuf>,
    fingerprint: String,
    status: ProfileCacheStatus,
}

impl ProfileCache {
    pub(super) fn new(
        source: &str,
        function: &str,
        executable: &ExecutableFunction,
        enabled: bool,
    ) -> Self {
        let structural_identity = format!(
            "{:?}\n{:?}",
            executable.bytecode(),
            executable.structure_map()
        );
        let fingerprint = fingerprint(source, function, &structural_identity);
        if !enabled {
            return Self {
                directory: None,
                path: None,
                fingerprint,
                status: ProfileCacheStatus::Disabled,
            };
        }
        let directory = cache_directory();
        let path = directory
            .as_ref()
            .map(|directory| directory.join(format!("{fingerprint}.json")));
        Self {
            directory,
            path,
            fingerprint,
            status: ProfileCacheStatus::Miss,
        }
    }

    pub(super) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(super) const fn status(&self) -> ProfileCacheStatus {
        self.status
    }

    pub(super) fn reject(&mut self) {
        self.status = ProfileCacheStatus::Rejected;
    }

    pub(super) fn load(&mut self) -> Option<ProfileArtifact> {
        let path = self.path.as_ref()?;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) => {
                self.status = ProfileCacheStatus::Error;
                return None;
            }
        };
        if metadata.len() > MAX_ARTIFACT_BYTES {
            self.status = ProfileCacheStatus::Rejected;
            return None;
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.status = ProfileCacheStatus::Error;
                return None;
            }
        };
        let artifact: ProfileArtifact = match serde_json::from_slice(&bytes) {
            Ok(artifact) => artifact,
            Err(_) => {
                self.status = ProfileCacheStatus::Rejected;
                return None;
            }
        };
        if artifact.fingerprint() != self.fingerprint {
            self.status = ProfileCacheStatus::Rejected;
            return None;
        }
        self.status = ProfileCacheStatus::Hit;
        Some(artifact)
    }

    pub(super) fn store(&mut self, artifact: &ProfileArtifact) {
        let (Some(directory), Some(path)) = (&self.directory, &self.path) else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(artifact) else {
            self.status = ProfileCacheStatus::Error;
            return;
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ARTIFACT_BYTES
            || fs::create_dir_all(directory).is_err()
        {
            self.status = ProfileCacheStatus::Error;
            return;
        }
        let temporary = directory.join(format!(".{}.{}.tmp", self.fingerprint, std::process::id()));
        if fs::write(&temporary, bytes).is_err() || fs::rename(&temporary, path).is_err() {
            let _ = fs::remove_file(&temporary);
            self.status = ProfileCacheStatus::Error;
            return;
        }
        prune(directory);
    }
}

fn fingerprint(source: &str, function: &str, structural_identity: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"wustite-runtime-profile-v1\0");
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(&wustite::value::RUNTIME_SLOT_ABI_VERSION.to_le_bytes());
    hasher.update(function.as_bytes());
    hasher.update(&[0]);
    hasher.update(source.as_bytes());
    hasher.update(&[0]);
    hasher.update(structural_identity.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn cache_directory() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(root).join("wustite/profiles"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache/wustite/profiles"))
}

fn prune(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((
                path,
                metadata.len(),
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut bytes = files.iter().map(|(_, size, _)| *size).sum::<u64>();
    let mut count = files.len();
    for (path, size, _) in files {
        if count <= MAX_CACHE_ENTRIES && bytes <= MAX_CACHE_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            count -= 1;
            bytes = bytes.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

    #[test]
    fn fingerprint_is_exact_for_source_and_entry_function() {
        let baseline = fingerprint("def main():\n    return 1\n", "main", "code-v1");
        assert_eq!(
            baseline,
            fingerprint("def main():\n    return 1\n", "main", "code-v1")
        );
        assert_ne!(
            baseline,
            fingerprint("def main():\n    return 2\n", "main", "code-v1")
        );
        assert_ne!(
            baseline,
            fingerprint("def main():\n    return 1\n", "other", "code-v1")
        );
        assert_ne!(
            baseline,
            fingerprint("def main():\n    return 1\n", "main", "code-v2")
        );
    }
}
