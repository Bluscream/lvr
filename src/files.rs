//! Same-directory atomic file replacement, shared by configuration and integrations.
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating temporary file beside {}", path.display()))?;
    if let Ok(metadata) = fs::metadata(path) {
        tmp.as_file().set_permissions(metadata.permissions())?;
    }
    tmp.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    fs::File::open(parent)?
        .sync_all()
        .with_context(|| format!("syncing {}", parent.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_keeps_old_open_readers_and_permissions() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        atomic_write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let mut old = fs::File::open(&path).unwrap();
        atomic_write(&path, b"new").unwrap();
        let mut original = String::new();
        old.read_to_string(&mut original).unwrap();
        assert_eq!(original, "old");
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
