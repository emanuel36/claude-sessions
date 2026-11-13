//! Where a reading of a file that only grows stopped, so the next one parses only what was
//! added. Claude Code's history and transcripts are such files; their readers keep a
//! [`Position`] on disk between runs.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The bytes before where a reading stopped, kept to tell the file is still the one read.
pub const MARK_BYTES: usize = 64;

/// Which file a reading was made of.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileId {
    pub path: String,
    pub dev: u64,
    pub ino: u64,
}

impl FileId {
    /// Returns the id of the file at `path`, whose metadata is `meta`.
    pub fn of(path: &Path, meta: &Metadata) -> Self {
        FileId {
            path: path.to_string_lossy().into_owned(),
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}

/// Where a reading stopped in a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub file: FileId,
    pub offset: u64,
    /// The bytes just before `offset`.
    pub mark: Vec<u8>,
}

impl Position {
    /// The start of the file `file`.
    pub fn start(file: FileId) -> Self {
        Position {
            file,
            offset: 0,
            mark: Vec::new(),
        }
    }

    /// Tells whether an open file, `id` with metadata `meta`, is the one this reading stopped
    /// in, grown since.
    pub fn continues(&self, file: &mut File, meta: &Metadata, id: &FileId) -> bool {
        if self.file != *id || meta.len() < self.offset || self.mark.len() > MARK_BYTES {
            return false;
        }
        let Some(from) = self.offset.checked_sub(self.mark.len() as u64) else {
            return false;
        };
        let mut found = vec![0; self.mark.len()];
        file.seek(SeekFrom::Start(from)).is_ok()
            && file.read_exact(&mut found).is_ok()
            && found == self.mark
    }

    /// Moves past `read`, the complete lines read from `offset` on.
    pub fn advance(&mut self, read: &[u8]) {
        self.offset += read.len() as u64;
        self.mark = read[read.len().saturating_sub(MARK_BYTES)..].to_vec();
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;

    use tempfile::TempDir;

    use crate::core::growing::{FileId, Position, MARK_BYTES};

    fn open(path: &std::path::Path) -> (File, std::fs::Metadata, FileId) {
        let file = File::open(path).unwrap();
        let meta = file.metadata().unwrap();
        let id = FileId::of(path, &meta);
        (file, meta, id)
    }

    #[test]
    fn a_position_continues_only_in_the_same_file_grown() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("f");
        fs::write(&path, "one\n").unwrap();
        let (_, _, id) = open(&path);
        let mut at = Position::start(id);
        at.advance(b"one\n");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"two\n")
            .unwrap();
        let (mut file, meta, id) = open(&path);
        assert!(at.continues(&mut file, &meta, &id));
        // Other bytes before the offset, a mark too long, another file: start over
        let mut other = at.clone();
        other.mark = b"ONE\n".to_vec();
        assert!(!other.continues(&mut file, &meta, &id));
        let mut long = at.clone();
        long.mark = vec![b'x'; MARK_BYTES + 1];
        assert!(!long.continues(&mut file, &meta, &id));
        let mut moved = at.clone();
        moved.file.ino += 1;
        assert!(!moved.continues(&mut file, &meta, &id));
        // Shorter than the offset
        fs::write(&path, "o").unwrap();
        let (mut file, meta, id) = open(&path);
        assert!(!at.continues(&mut file, &meta, &id));
    }

    #[test]
    fn the_mark_keeps_the_last_bytes_read() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("f"), "").unwrap();
        let (_, _, id) = open(&tmp.path().join("f"));
        let mut at = Position::start(id);
        let long = vec![b'a'; MARK_BYTES * 2];
        at.advance(&long);
        assert_eq!((at.offset, at.mark.len()), (long.len() as u64, MARK_BYTES));
        at.advance(b"b\n");
        assert_eq!(at.mark, b"b\n");
    }
}
