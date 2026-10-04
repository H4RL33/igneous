//! Opt-in checks against real vaults. Read-only; prints only counts.
//!
//! ```sh
//! IGNEOUS_CORPUS=/path/to/vault-copy:/another cargo test -p igneous-markdown --release -- --ignored
//! ```

use std::path::Path;
use std::time::{Duration, Instant};

fn markdown_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

#[test]
#[ignore = "needs IGNEOUS_CORPUS"]
fn corpus_parses_cleanly() {
    let Ok(corpus) = std::env::var("IGNEOUS_CORPUS") else {
        eprintln!("IGNEOUS_CORPUS not set; skipping");
        return;
    };
    let mut files = Vec::new();
    for dir in corpus.split(':').filter(|d| !d.is_empty()) {
        markdown_files(Path::new(dir), &mut files);
    }
    let (mut bytes, mut slowest, mut total) = (0usize, Duration::ZERO, Duration::ZERO);
    let (mut fm_errors, mut links, mut tags, mut spans) = (0, 0, 0, 0);
    for path in &files {
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let Ok(file) = igneous_core::TextFile::from_bytes(&raw) else {
            continue;
        };
        if !file.had_mixed_line_endings() {
            assert_eq!(file.to_bytes(), raw, "round trip failed for a corpus file");
        }
        let text = file.text();
        bytes += text.len();
        let start = Instant::now();
        let doc = igneous_markdown::parse(text);
        let presented = igneous_markdown::present::spans(&doc, text);
        let elapsed = start.elapsed();
        total += elapsed;
        slowest = slowest.max(elapsed);
        for node in &doc.nodes {
            assert!(
                text.is_char_boundary(node.range.start) && text.is_char_boundary(node.range.end)
            );
            for m in &node.markers {
                assert!(m.start >= node.range.start && m.end <= node.range.end);
            }
        }
        for span in &presented {
            for m in &span.markers {
                assert!(text.is_char_boundary(m.start) && text.is_char_boundary(m.end));
            }
        }
        fm_errors += doc.frontmatter.as_ref().is_some_and(|f| f.error.is_some()) as usize;
        links += doc.links.len();
        tags += doc.tags.len();
        spans += presented.len();
    }
    eprintln!(
        "{} notes, {} KiB: parse+present total {:?}, slowest {:?}; {} links, {} tags, {} spans, {} invalid frontmatter",
        files.len(),
        bytes / 1024,
        total,
        slowest,
        links,
        tags,
        spans,
        fm_errors
    );
}
