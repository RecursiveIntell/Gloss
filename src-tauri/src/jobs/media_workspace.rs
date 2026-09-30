//! Private per-invocation scratch ownership. Persisted source identities are
//! metadata and never filesystem paths, including identities from imports.
use tempfile::TempDir;

#[derive(Clone, Copy)]
pub enum MediaWorkspaceKind {
    VideoFrames,
    AudioTranscript,
}

pub fn create_media_workspace(kind: MediaWorkspaceKind) -> std::io::Result<TempDir> {
    let prefix = match kind {
        MediaWorkspaceKind::VideoFrames => "gloss-video-frames-",
        MediaWorkspaceKind::AudioTranscript => "gloss-audio-transcript-",
    };
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_job_has_a_unique_private_workspace_and_drop_cleans_partial_outputs() {
        for kind in [
            MediaWorkspaceKind::VideoFrames,
            MediaWorkspaceKind::AudioTranscript,
        ] {
            let first = create_media_workspace(kind).unwrap();
            let second = create_media_workspace(kind).unwrap();
            assert_ne!(first.path(), second.path());
            let path = first.path().to_path_buf();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            std::fs::write(path.join("private-partial-output"), "fixture").unwrap();
            std::fs::write(second.path().join("other-job"), "retain").unwrap();
            drop(first);
            assert!(!path.exists());
            assert_eq!(
                std::fs::read(second.path().join("other-job")).unwrap(),
                b"retain"
            );
        }
    }
}
