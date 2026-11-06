use crate::BoxServiceIo;
use crate::records::{CHUNK, CONTROL_LIMIT, expect_fin, write_record};
use anyhow::{Context, Result, bail, ensure};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::fs::{Dir, OpenOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use uuid::Uuid;

pub(crate) struct Files {
    root: Arc<Dir>,
    pub max_size: u64,
}

impl Files {
    pub fn new(root: Dir, max_size: u64) -> Self {
        Self {
            root: Arc::new(root),
            max_size,
        }
    }

    fn components(path: &str, root_ok: bool) -> Result<Vec<&str>> {
        if root_ok && (path == "." || path.is_empty()) {
            return Ok(Vec::new());
        }
        ensure!(!path.is_empty() && path.len() <= 4096, "invalid path");
        ensure!(
            !path.contains(['\\', '\0', ':', '<', '>', '"', '|', '?', '*']),
            "invalid path"
        );
        let parts: Vec<_> = path.split('/').collect();
        ensure!(parts.len() <= 128, "path too deep");
        ensure!(
            parts.iter().all(|s| !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.len() <= 255
                && !s.ends_with(['.', ' '])
                && !s.starts_with(".lightremote-upload-")),
            "invalid path"
        );
        Ok(parts)
    }

    fn directory(&self, parts: &[&str]) -> Result<Dir> {
        let mut dir = self.root.try_clone()?;
        for part in parts {
            // The no-follow open is one operation on an anchored directory handle;
            // a metadata check followed by a normal open would have a symlink race.
            dir = dir
                .open_dir_nofollow(part)
                .context("cannot open directory")?;
        }
        Ok(dir)
    }

    fn parent<'a>(&self, path: &'a str) -> Result<(Dir, &'a str)> {
        let parts = Self::components(path, false)?;
        let name = parts.last().context("invalid path")?;
        Ok((self.directory(&parts[..parts.len() - 1])?, name))
    }

    pub fn list(&self, path: &str) -> Result<Value> {
        let dir = self.directory(&Self::components(path, true)?)?;
        let mut entries = Vec::new();
        for (index, entry) in dir.entries()?.enumerate() {
            ensure!(index < 4096, "directory entry limit exceeded");
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // Internal staging files are not a public part of the shared tree.
            if name.starts_with(".lightremote-upload-") {
                continue;
            }
            let metadata = dir.symlink_metadata(name)?;
            let kind = if metadata.is_symlink() {
                "symlink"
            } else if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "other"
            };
            entries.push(json!({"name":name,"type":kind,"size":metadata.len()}));
        }
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let result = json!({"entries": entries});
        ensure!(
            serde_json::to_vec(&result)?.len() <= CONTROL_LIMIT / 2,
            "directory response too large"
        );
        Ok(result)
    }

    pub fn mkdir(&self, path: &str) -> Result<Value> {
        let (parent, name) = self.parent(path)?;
        parent.create_dir(name)?;
        Ok(json!({}))
    }

    pub fn remove(&self, path: &str) -> Result<Value> {
        let (parent, name) = self.parent(path)?;
        let meta = parent.symlink_metadata(name)?;
        ensure!(!meta.is_symlink(), "symlink access denied");
        // Deliberately non-recursive; removes can never traverse a replaced child.
        if meta.is_dir() {
            parent.remove_dir(name)?;
        } else {
            ensure!(meta.is_file(), "only regular files can be removed");
            parent.remove_file(name)?;
        }
        Ok(json!({}))
    }

    pub async fn get(&self, path: &str, io: BoxServiceIo, idle: Duration) -> Result<()> {
        let (parent, name) = self.parent(path)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = parent
            .open_with(name, &options)
            .context("cannot open file")?;
        let meta = file.metadata()?;
        ensure!(meta.is_file(), "only regular files can be downloaded");
        ensure!(meta.len() <= self.max_size, "file size limit exceeded");
        let mut file = tokio::fs::File::from_std(file.into_std());
        let (mut input, mut output) = tokio::io::split(io);
        let send = async {
            let mut left = meta.len();
            let mut buffer = vec![0; CHUNK];
            while left > 0 {
                let count = (left.min(buffer.len() as u64)) as usize;
                let n = timeout(idle, file.read(&mut buffer[..count])).await??;
                ensure!(n > 0, "file changed during download");
                timeout(idle, output.write_all(&buffer[..n])).await??;
                left -= n as u64;
            }
            Ok(())
        };
        tokio::try_join!(send, expect_fin(&mut input, idle))?;
        timeout(idle, output.shutdown()).await??;
        Ok(())
    }

    pub async fn put(
        &self,
        path: &str,
        size: u64,
        mut io: BoxServiceIo,
        idle: Duration,
    ) -> Result<()> {
        ensure!(size <= self.max_size, "file size limit exceeded");
        let (parent, name) = self.parent(path)?;
        // Fail early, but the final hard-link operation is the authoritative
        // no-overwrite check, including destinations raced into existence.
        match parent.symlink_metadata(name) {
            Ok(_) => bail!("destination already exists"),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let temp_name = format!(".lightremote-upload-{}", Uuid::new_v4());
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = parent.open_with(&temp_name, &options)?;
        let staging = Staging {
            parent,
            name: temp_name,
        };
        let mut file = tokio::fs::File::from_std(file.into_std());
        let mut remaining = size;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; CHUNK];
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            let n = timeout(idle, io.read(&mut buffer[..count])).await??;
            ensure!(n != 0, "upload ended before declared size");
            hash.update(&buffer[..n]);
            timeout(idle, file.write_all(&buffer[..n])).await??;
            remaining -= n as u64;
        }
        // No commit without FIN, even when the declared number of bytes arrived.
        expect_fin(&mut io, idle).await?;
        timeout(idle, file.flush()).await??;
        timeout(idle, file.sync_all()).await??;
        drop(file);
        staging
            .parent
            .hard_link(&staging.name, &staging.parent, name)
            .context("cannot commit upload without overwriting")?;
        let receipt =
            serde_json::to_vec(&json!({"size":size,"sha256":format!("{:x}", hash.finalize())}))?;
        write_record(&mut io, &receipt, CONTROL_LIMIT, idle).await?;
        timeout(idle, io.shutdown()).await??;
        Ok(())
    }
}

struct Staging {
    parent: Dir,
    name: String,
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = self.parent.remove_file(&self.name);
    }
}
