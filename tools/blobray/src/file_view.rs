//! Read capabilities bounded to one captured file extent, with private cursors.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use crate::Result;

/// Clones retain the same file descriptor and bounds, never a pathname lookup.
#[derive(Clone)]
pub(crate) struct FileView {
    file: Arc<File>,
    start: u64,
    length: u64,
}

impl FileView {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Self::new(file, 0, length)
    }

    pub(crate) fn new(file: File, start: u64, length: u64) -> Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || start
                .checked_add(length)
                .is_none_or(|end| end > metadata.len())
        {
            return Err(crate::Error::invalid(
                "file view exceeds its captured regular file",
            ));
        }
        Ok(Self {
            file: Arc::new(file),
            start,
            length,
        })
    }

    pub(crate) fn cursor(&self) -> FileCursor {
        FileCursor {
            view: self.clone(),
            position: 0,
        }
    }

    pub(crate) fn len(&self) -> u64 {
        self.length
    }

    pub(crate) fn read_to_string(&self) -> Result<String> {
        let mut text = String::new();
        self.cursor().read_to_string(&mut text)?;
        Ok(text)
    }

    pub(crate) fn read_range(&self, offset: u64, length: u64) -> Result<Vec<u8>> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            return Err(crate::Error::invalid(
                "indexed range exceeds captured file extent",
            ));
        }
        let mut reader = self.cursor();
        reader.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        reader.take(length).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// A seek offset is relative to the view, never to the containing CAS pack.
pub(crate) struct FileCursor {
    view: FileView,
    position: u64,
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at(_file: &File, _buffer: &mut [u8], _offset: u64) -> io::Result<usize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "positional file reads require a supported host",
    ))
}

impl Read for FileCursor {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer
            .len()
            .min(usize::try_from(self.view.length - self.position).unwrap_or(usize::MAX));
        if count == 0 {
            return Ok(0);
        }
        let offset = self.view.start + self.position;
        let read = read_at(&self.view.file, &mut buffer[..count], offset)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "captured file extent was truncated",
            ));
        }
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for FileCursor {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self.view.length.checked_add_signed(offset),
        }
        .filter(|position| *position <= self.view.length)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside captured file extent",
            )
        })?;
        self.position = position;
        Ok(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_cursors_cannot_read_adjacent_frames_or_follow_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pack");
        std::fs::write(&path, b"headPAYLOADtail").unwrap();
        let file = File::open(&path).unwrap();
        let other = file.try_clone().unwrap();
        let view = FileView::new(file, 4, 7).unwrap();
        let adjacent = FileView::new(other, 11, 4).unwrap();
        let mut first = view.cursor();
        let mut second = view.cursor();
        let mut byte = [0];
        first.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"P");
        assert_eq!(adjacent.read_to_string().unwrap(), "tail");
        second.seek(SeekFrom::End(-1)).unwrap();
        second.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"D");
        first.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"A");
        assert_eq!(second.read(&mut byte).unwrap(), 0);
        assert!(first.seek(SeekFrom::Start(8)).is_err());
        assert!(first.seek(SeekFrom::Current(-3)).is_err());
        std::fs::rename(&path, directory.path().join("old-pack")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(view.read_to_string().unwrap(), "PAYLOAD");
    }

    #[test]
    fn truncated_view_is_an_error_and_invalid_extent_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pack");
        std::fs::write(&path, b"payload").unwrap();
        assert!(FileView::new(File::open(&path).unwrap(), u64::MAX, 1).is_err());
        let view = FileView::open(&path).unwrap();
        assert!(view.read_range(1, u64::MAX).is_err());
        assert!(view.read_range(5, 3).is_err());
        assert_eq!(view.read_range(2, 3).unwrap(), b"ylo");
        std::fs::write(&path, b"short").unwrap();
        assert!(view.read_to_string().is_err());
    }
}
