//! Text files with byte-level fidelity.
//!
//! Editing always happens on `\n`-separated text. [`TextFile`] remembers the
//! file's line ending and byte-order mark so that writing it back reproduces
//! the original bytes. Files with mixed line endings are normalised to their
//! dominant ending, which only matters if they are edited: Igneous never writes
//! a file the user hasn't changed.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("file is not valid UTF-8 (first invalid byte at offset {offset})")]
pub struct NotUtf8 {
    pub offset: usize,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextFile {
    text: String,
    line_ending: LineEnding,
    bom: bool,
    mixed: bool,
}

impl TextFile {
    /// A new file with `\n` line endings and no byte-order mark.
    pub fn new(text: impl Into<String>) -> Self {
        let mut file = Self::default();
        file.set_text(text.into());
        file
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NotUtf8> {
        let (bom, body) = match bytes.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let raw = std::str::from_utf8(body).map_err(|e| NotUtf8 {
            offset: e.valid_up_to() + if bom { BOM.len() } else { 0 },
        })?;

        let crlf = raw.matches("\r\n").count();
        let lf = raw.matches('\n').count() - crlf;
        let line_ending = if crlf > lf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let text = if crlf > 0 {
            raw.replace("\r\n", "\n")
        } else {
            raw.to_owned()
        };
        Ok(Self {
            text,
            line_ending,
            bom,
            mixed: crlf > 0 && lf > 0,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.text.len() + 16);
        if self.bom {
            out.extend_from_slice(BOM);
        }
        match self.line_ending {
            LineEnding::Lf => out.extend_from_slice(self.text.as_bytes()),
            LineEnding::CrLf => out.extend_from_slice(self.text.replace('\n', "\r\n").as_bytes()),
        }
        out
    }

    /// The content with `\n` line endings.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the content. Any `\r\n` in `text` is normalised to `\n`.
    pub fn set_text(&mut self, text: String) {
        self.text = if text.contains("\r\n") {
            text.replace("\r\n", "\n")
        } else {
            text
        };
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn set_line_ending(&mut self, line_ending: LineEnding) {
        self.line_ending = line_ending;
    }

    pub fn has_bom(&self) -> bool {
        self.bom
    }

    /// True if the file was read with both `\n` and `\r\n` line endings, so
    /// writing it would not reproduce the original bytes.
    pub fn had_mixed_line_endings(&self) -> bool {
        self.mixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn round_trip(bytes: &[u8]) {
        let file = TextFile::from_bytes(bytes).unwrap();
        assert!(!file.had_mixed_line_endings());
        assert_eq!(
            file.to_bytes(),
            bytes,
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }

    #[test]
    fn preserves_lf_crlf_bom_and_final_newline() {
        round_trip(b"# Title\n\nbody\n");
        round_trip(b"# Title\n\nbody");
        round_trip(b"# Title\r\n\r\nbody\r\n");
        round_trip(b"\xEF\xBB\xBF# Title\r\nbody");
        round_trip(b"");
        round_trip(b"lone\rcarriage return\n");
    }

    #[test]
    fn crlf_is_normalised_for_editing() {
        let file = TextFile::from_bytes(b"a\r\nb\r\n").unwrap();
        assert_eq!(file.text(), "a\nb\n");
        assert_eq!(file.line_ending(), LineEnding::CrLf);
    }

    #[test]
    fn mixed_line_endings_use_dominant() {
        let file = TextFile::from_bytes(b"a\r\nb\r\nc\n").unwrap();
        assert!(file.had_mixed_line_endings());
        assert_eq!(file.line_ending(), LineEnding::CrLf);
        assert_eq!(file.to_bytes(), b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn set_text_normalises() {
        let mut file = TextFile::from_bytes(b"x\r\n").unwrap();
        file.set_text("pasted\r\nline\n".into());
        assert_eq!(file.text(), "pasted\nline\n");
        assert_eq!(file.to_bytes(), b"pasted\r\nline\r\n");
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert_eq!(
            TextFile::from_bytes(b"ok\xFFbad"),
            Err(NotUtf8 { offset: 2 })
        );
    }

    proptest! {
        #[test]
        fn consistent_endings_round_trip(
            lines in proptest::collection::vec("[^\r\n]{0,12}", 0..8),
            crlf in any::<bool>(),
            bom in any::<bool>(),
            final_newline in any::<bool>(),
        ) {
            let sep = if crlf { "\r\n" } else { "\n" };
            let mut text = lines.join(sep);
            if final_newline { text.push_str(sep); }
            let mut bytes = if bom { BOM.to_vec() } else { Vec::new() };
            bytes.extend_from_slice(text.as_bytes());
            let file = TextFile::from_bytes(&bytes).unwrap();
            prop_assert_eq!(file.to_bytes(), bytes);
        }
    }
}
