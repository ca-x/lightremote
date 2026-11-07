use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};

use flate2::read::GzDecoder;

use crate::{Error, Release, Result};

/// Bounds expanded data, including ignored documents and tar headers/padding.
const EXPANDED_LIMIT: u64 = 512 * 1024 * 1024;
const BINARY_LIMIT: u64 = 256 * 1024 * 1024;
const ENTRY_LIMIT: usize = 1024;

pub(crate) fn extract(release: &Release, source: &mut File, output: &mut File) -> Result<()> {
    source.seek(SeekFrom::Start(0))?;
    let root = format!(
        "{}-{}-{}",
        release.product.component(),
        release.tag,
        release.target.triple()
    );
    let binary = release.product.binary(release.target);
    if release.target.windows() {
        extract_zip(source, output, &root, &binary, EXPANDED_LIMIT, BINARY_LIMIT)
    } else {
        extract_tar(source, output, &root, &binary, EXPANDED_LIMIT, BINARY_LIMIT)
    }
}

struct Layout<'a> {
    root: &'a str,
    binary: &'a str,
    seen: HashSet<String>,
    found: bool,
    expanded: u64,
    limit: u64,
    binary_limit: u64,
}

impl<'a> Layout<'a> {
    fn new(root: &'a str, binary: &'a str, limit: u64, binary_limit: u64) -> Self {
        Self {
            root,
            binary,
            seen: HashSet::new(),
            found: false,
            expanded: 0,
            limit,
            binary_limit,
        }
    }

    fn entry(&mut self, path: &str, directory: bool, size: u64) -> Result<bool> {
        // Check raw slash-separated names; host Path rules differ between Unix and Windows.
        let path = if directory {
            path.strip_suffix('/').unwrap_or(path)
        } else {
            path
        };
        if path.len() > 4096
            || path.contains(['\\', ':'])
            || path.chars().any(char::is_control)
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::Archive("invalid entry path"));
        }
        if path != self.root
            && !path
                .strip_prefix(self.root)
                .is_some_and(|rest| rest.starts_with('/'))
        {
            return Err(Error::Archive("unexpected archive root"));
        }
        if self.seen.len() >= ENTRY_LIMIT || !self.seen.insert(path.to_owned()) {
            return Err(Error::Archive("too many or duplicate entries"));
        }
        self.expanded = self
            .expanded
            .checked_add(size)
            .ok_or(Error::SizeLimit(self.limit))?;
        if self.expanded > self.limit || size > self.binary_limit {
            return Err(Error::SizeLimit(self.binary_limit));
        }
        let expected = format!("{}/{}", self.root, self.binary);
        let candidate = path == expected;
        if directory && (size != 0 || candidate) {
            return Err(Error::Archive("invalid directory entry"));
        }
        if !directory
            && (path == self.root || (path.rsplit('/').next() == Some(self.binary) && !candidate))
        {
            return Err(Error::Archive("binary in unexpected location"));
        }
        if candidate {
            if self.found || size == 0 {
                return Err(Error::Archive("duplicate or empty binary"));
            }
            self.found = true;
        }
        Ok(candidate)
    }

    fn finish(self) -> Result<()> {
        if self.found {
            Ok(())
        } else {
            Err(Error::Archive("expected binary missing"))
        }
    }
}

fn extract_tar(
    source: impl Read,
    output: &mut impl Write,
    root: &str,
    binary: &str,
    limit: u64,
    binary_limit: u64,
) -> Result<()> {
    let mut decoded = GzDecoder::new(source).take(limit + 1);
    let mut layout = Layout::new(root, binary, limit, binary_limit);
    {
        let mut archive = tar::Archive::new(&mut decoded);
        let mut timestamp_header = false;
        for (count, entry) in archive.entries()?.raw(true).enumerate() {
            if count >= ENTRY_LIMIT {
                return Err(Error::Archive("too many tar entries"));
            }
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if kind == tar::EntryType::XHeader {
                // Python tarfile's default PAX format stores float mtimes in a local
                // extension. Do not let extension metadata rewrite paths or links.
                if timestamp_header
                    || entry.path_bytes().as_ref() != b"././@PaxHeader"
                    || entry.size() > 4096
                {
                    return Err(Error::Archive("unsupported tar extension"));
                }
                let mut metadata = Vec::new();
                entry.read_to_end(&mut metadata)?;
                timestamp_pax(&metadata)?;
                timestamp_header = true;
                continue;
            }
            if !kind.is_file() && !kind.is_dir() {
                return Err(Error::Archive(
                    "links and special tar entries are forbidden",
                ));
            }
            let name = entry.path_bytes();
            let path = std::str::from_utf8(&name).map_err(|_| Error::Archive("non-UTF-8 path"))?;
            let size = entry.header().size()?;
            let wanted = layout.entry(path, kind.is_dir(), size)?;
            let copied = if wanted {
                io::copy(&mut entry, output)?
            } else {
                io::copy(&mut entry, &mut io::sink())?
            };
            if copied != size {
                return Err(Error::Archive("truncated tar entry"));
            }
            timestamp_header = false;
        }
        if timestamp_header {
            return Err(Error::Archive("dangling tar extension"));
        }
    }
    // Force gzip trailer/CRC validation and bound padding after the tar terminator.
    io::copy(&mut decoded, &mut io::sink())?;
    if decoded.limit() == 0 {
        return Err(Error::SizeLimit(limit));
    }
    layout.finish()
}

fn timestamp_pax(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Archive("invalid PAX encoding"))?;
    let (length, record) = text
        .split_once(' ')
        .ok_or(Error::Archive("invalid PAX record"))?;
    if length.parse::<usize>().ok() != Some(bytes.len()) {
        return Err(Error::Archive("invalid PAX record length"));
    }
    let value = record
        .strip_prefix("mtime=")
        .and_then(|s| s.strip_suffix('\n'))
        .ok_or(Error::Archive("PAX path/link overrides are forbidden"))?;
    if !value
        .bytes()
        .all(|c| c.is_ascii_digit() || c == b'.' || c == b'-')
        || !value.parse::<f64>().is_ok_and(f64::is_finite)
    {
        return Err(Error::Archive("invalid PAX timestamp"));
    }
    Ok(())
}

fn extract_zip(
    mut source: impl Read + Seek,
    output: &mut impl Write,
    root: &str,
    binary: &str,
    limit: u64,
    binary_limit: u64,
) -> Result<()> {
    let entries = zip_entry_count(&mut source)?;
    let mut archive = zip::ZipArchive::new(source)?;
    // zip 2 indexes names and can collapse duplicate central-directory entries.
    if archive.len() != entries || archive.offset() != 0 {
        return Err(Error::Archive("duplicate zip entries or prepended data"));
    }
    if archive.len() > ENTRY_LIMIT {
        return Err(Error::Archive("too many zip entries"));
    }
    let mut layout = Layout::new(root, binary, limit, binary_limit);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.encrypted() {
            return Err(Error::Archive("encrypted zip entry"));
        }
        let directory = entry.is_dir();
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                return Err(Error::Archive(
                    "links and special zip entries are forbidden",
                ));
            }
            if kind == 0o040000 && !directory {
                return Err(Error::Archive("inconsistent zip directory"));
            }
        }
        let size = entry.size();
        let wanted = layout.entry(entry.name(), directory, size)?;
        // Read every entry, including docs, to validate CRC and declared lengths.
        let mut bounded = (&mut entry).take(size.min(binary_limit) + 1);
        let copied = if wanted {
            io::copy(&mut bounded, output)?
        } else {
            io::copy(&mut bounded, &mut io::sink())?
        };
        if copied != size {
            return Err(Error::Archive("zip entry size mismatch"));
        }
    }
    layout.finish()
}

// Official artifacts fit classic ZIP bounds. Reject ZIP64/multidisk layouts before
// zip allocates its central-directory index, and retain the raw advertised count.
fn zip_entry_count(source: &mut (impl Read + Seek)) -> Result<usize> {
    let length = source.seek(SeekFrom::End(0))?;
    let tail_len = length.min(65_557) as usize;
    source.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0; tail_len];
    source.read_exact(&mut tail)?;
    let offset = tail
        .windows(4)
        .rposition(|bytes| bytes == b"PK\x05\x06")
        .ok_or(Error::Archive("missing zip end record"))?;
    let header = tail
        .get(offset..offset + 22)
        .ok_or(Error::Archive("truncated zip end record"))?;
    let word = |n| u16::from_le_bytes([header[n], header[n + 1]]);
    let dword = |n| u32::from_le_bytes([header[n], header[n + 1], header[n + 2], header[n + 3]]);
    let count = usize::from(word(10));
    let central_size = u64::from(dword(12));
    let central_start = u64::from(dword(16));
    if word(4) != 0
        || word(6) != 0
        || word(8) != word(10)
        || count == 0
        || count > ENTRY_LIMIT
        || word(10) == u16::MAX
        || offset + 22 + usize::from(word(20)) != tail.len()
        || central_start + central_size != length - tail_len as u64 + offset as u64
    {
        return Err(Error::Archive("unsupported or oversized zip layout"));
    }
    source.seek(SeekFrom::Start(0))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Cursor;

    fn tar_bytes(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (name, kind, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            // Set raw bytes to exercise traversal that tar::Builder normally refuses.
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_entry_type(*kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn accepts_python_timestamp_pax_but_rejects_path_overrides_and_dangling_headers() {
        let record = |key, value| {
            let payload = format!("{key}={value}\n");
            let mut length = payload.len() + 2;
            loop {
                let actual = length.to_string().len() + 1 + payload.len();
                if actual == length {
                    break;
                }
                length = actual;
            }
            format!("{length} {payload}").into_bytes()
        };
        let timestamp = record("mtime", "1762463100.123456");
        let good = (
            "product/bin",
            tar::EntryType::Regular,
            b"executable".as_slice(),
        );
        let bytes = tar_bytes(&[
            ("././@PaxHeader", tar::EntryType::XHeader, &timestamp),
            good,
        ]);
        let mut output = Vec::new();
        extract_tar(
            bytes.as_slice(),
            &mut output,
            "product",
            "bin",
            16_384,
            1024,
        )
        .unwrap();
        assert_eq!(output, b"executable");
        for key in ["path", "linkpath", "GNU.sparse.name"] {
            let metadata = record(key, "../evil");
            let bytes = tar_bytes(&[("././@PaxHeader", tar::EntryType::XHeader, &metadata), good]);
            assert!(
                extract_tar(
                    bytes.as_slice(),
                    &mut Vec::new(),
                    "product",
                    "bin",
                    16_384,
                    1024
                )
                .is_err(),
                "{key}"
            );
        }
        let dangling = tar_bytes(&[
            good,
            ("././@PaxHeader", tar::EntryType::XHeader, &timestamp),
        ]);
        assert!(
            extract_tar(
                dangling.as_slice(),
                &mut Vec::new(),
                "product",
                "bin",
                16_384,
                1024
            )
            .is_err()
        );
        assert!(timestamp_pax(b"99 mtime=1.0\n").is_err());
    }

    #[test]
    fn tar_extracts_only_exact_binary_and_rejects_malicious_entries() {
        let good = (
            "product/bin",
            tar::EntryType::Regular,
            b"executable".as_slice(),
        );
        let bytes = tar_bytes(&[("product/README", tar::EntryType::Regular, b"doc"), good]);
        let mut output = Vec::new();
        extract_tar(
            bytes.as_slice(),
            &mut output,
            "product",
            "bin",
            16_384,
            1024,
        )
        .unwrap();
        assert_eq!(output, b"executable");
        for name in [
            "../bin",
            "/bin",
            "product/../bin",
            "product/./bin",
            "product\\bin",
            "C:/bin",
            "other/bin",
            "product/nested/bin",
        ] {
            let bytes = tar_bytes(&[good, (name, tar::EntryType::Regular, b"bad")]);
            assert!(
                extract_tar(
                    bytes.as_slice(),
                    &mut Vec::new(),
                    "product",
                    "bin",
                    16_384,
                    1024
                )
                .is_err(),
                "{name}"
            );
        }
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Fifo,
            tar::EntryType::GNULongName,
            tar::EntryType::XHeader,
        ] {
            let bytes = tar_bytes(&[good, ("product/evil", kind, b"")]);
            assert!(
                extract_tar(
                    bytes.as_slice(),
                    &mut Vec::new(),
                    "product",
                    "bin",
                    16_384,
                    1024
                )
                .is_err(),
                "{kind:?}"
            );
        }
        let duplicate = tar_bytes(&[good, good]);
        assert!(
            extract_tar(
                duplicate.as_slice(),
                &mut Vec::new(),
                "product",
                "bin",
                16_384,
                1024
            )
            .is_err()
        );
        let missing = tar_bytes(&[("product/README", tar::EntryType::Regular, b"doc")]);
        assert!(
            extract_tar(
                missing.as_slice(),
                &mut Vec::new(),
                "product",
                "bin",
                16_384,
                1024
            )
            .is_err()
        );
        let oversized = tar_bytes(&[good, ("product/README", tar::EntryType::Regular, &[0; 32])]);
        assert!(
            extract_tar(
                oversized.as_slice(),
                &mut Vec::new(),
                "product",
                "bin",
                16_384,
                16
            )
            .is_err()
        );
        assert!(
            extract_tar(
                bytes.as_slice(),
                &mut Vec::new(),
                "product",
                "bin",
                32,
                1024
            )
            .is_err()
        );
    }

    #[test]
    fn zip_rejects_traversal_duplicates_links_and_expansion() {
        use zip::write::SimpleFileOptions;
        let zip_bytes = |names: &[&str]| {
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for name in names {
                archive
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(b"binary").unwrap();
            }
            archive.finish().unwrap().into_inner()
        };
        for names in [
            vec!["product/bin", "../escape"],
            vec!["product/bin", "product/README", "product/README/"],
            vec!["wrong/bin"],
            vec!["product/bin", "product\\evil"],
            vec!["product/bin", "product/nested/bin"],
        ] {
            let bytes = zip_bytes(&names);
            assert!(
                extract_zip(
                    Cursor::new(bytes),
                    &mut Vec::new(),
                    "product",
                    "bin",
                    1024,
                    128
                )
                .is_err(),
                "{names:?}"
            );
        }
        let bytes = zip_bytes(&["product/bin"]);
        assert!(
            extract_zip(
                Cursor::new(&bytes),
                &mut Vec::new(),
                "product",
                "bin",
                1024,
                3
            )
            .is_err()
        );
        let mut output = Vec::new();
        extract_zip(Cursor::new(bytes), &mut output, "product", "bin", 1024, 128).unwrap();
        assert_eq!(output, b"binary");
        let mut duplicate = zip_bytes(&["product/bin", "product/doc1", "product/doc2"]);
        let needle = b"product/doc2";
        for index in 0..duplicate.len() - needle.len() {
            if &duplicate[index..index + needle.len()] == needle {
                duplicate[index + needle.len() - 1] = b'1';
            }
        }
        assert!(
            extract_zip(
                Cursor::new(duplicate),
                &mut Vec::new(),
                "product",
                "bin",
                1024,
                128
            )
            .is_err()
        );
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .add_symlink("product/bin", "target", SimpleFileOptions::default())
            .unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        assert!(
            extract_zip(
                Cursor::new(bytes),
                &mut Vec::new(),
                "product",
                "bin",
                1024,
                128
            )
            .is_err()
        );
    }
}
