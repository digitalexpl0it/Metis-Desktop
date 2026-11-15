//! Text extractors (plain UTF-8 / source, HTML strip, optional `pdftotext`).

use metis_config::SearchExtractors;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::categorize::{extension_of, guess_mime};

const PDFTOTEXT_TIMEOUT: Duration = Duration::from_secs(8);
const PDF_STDOUT_CAP: usize = 8 * 1024 * 1024;

pub trait Extractor: Send + Sync {
    fn can_handle(&self, path: &Path, mime: &str) -> bool;
    fn extract(&self, path: &Path, max_chars: usize) -> Option<String>;
}

pub struct PlainTextExtractor;

impl Extractor for PlainTextExtractor {
    fn can_handle(&self, path: &Path, mime: &str) -> bool {
        if mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") {
            return true;
        }
        let ext = extension_of(path);
        matches!(
            ext.as_str(),
            "txt"
                | "md"
                | "rst"
                | "log"
                | "rs"
                | "py"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "rb"
                | "php"
                | "lua"
                | "sh"
                | "bash"
                | "zsh"
                | "toml"
                | "json"
                | "yaml"
                | "yml"
                | "xml"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "sql"
                | "cmake"
                | "vue"
                | "svelte"
                | "gitignore"
                | "dockerfile"
        ) || path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n.eq_ignore_ascii_case("Dockerfile") || n.eq_ignore_ascii_case("Makefile")
        })
    }

    fn extract(&self, path: &Path, max_chars: usize) -> Option<String> {
        let mut file = File::open(path).ok()?;
        let mut buf = Vec::new();
        // Cap read size (~4 bytes/char worst-case for UTF-8 BMP).
        let cap = (max_chars.saturating_mul(4)).min(8 * 1024 * 1024);
        buf.resize(cap, 0);
        let n = file.read(&mut buf).ok()?;
        buf.truncate(n);
        if looks_binary(&buf) {
            return None;
        }
        let text = String::from_utf8_lossy(&buf);
        let mut out: String = text.chars().take(max_chars).collect();
        // Soften nulls / control noise.
        out.retain(|c| c == '\n' || c == '\t' || !c.is_control());
        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// Strip tags/scripts/styles to visible text for HTML documents.
pub struct HtmlExtractor;

impl Extractor for HtmlExtractor {
    fn can_handle(&self, path: &Path, mime: &str) -> bool {
        mime.contains("html") || matches!(extension_of(path).as_str(), "html" | "htm")
    }

    fn extract(&self, path: &Path, max_chars: usize) -> Option<String> {
        let raw = PlainTextExtractor.extract(path, max_chars.saturating_mul(2))?;
        let stripped = strip_html(&raw);
        let out: String = stripped.chars().take(max_chars).collect();
        if out.trim().is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// Extract PDF text via optional `pdftotext` (poppler-utils). Soft-fails if missing.
pub struct PdfExtractor;

impl Extractor for PdfExtractor {
    fn can_handle(&self, path: &Path, mime: &str) -> bool {
        mime.contains("pdf") || extension_of(path) == "pdf"
    }

    fn extract(&self, path: &Path, max_chars: usize) -> Option<String> {
        pdftotext_extract(path, max_chars)
    }
}

pub fn build_extractors(cfg: &SearchExtractors) -> Vec<Box<dyn Extractor>> {
    let mut out: Vec<Box<dyn Extractor>> = Vec::new();
    // HTML before plain so `.html` gets stripped markup when enabled.
    if cfg.html {
        out.push(Box::new(HtmlExtractor));
    }
    if cfg.plain_text {
        out.push(Box::new(PlainTextExtractor));
    }
    if cfg.pdf {
        out.push(Box::new(PdfExtractor));
    }
    out
}

pub fn extract_body(
    path: &Path,
    max_bytes: u64,
    max_chars: usize,
    extractors: &[Box<dyn Extractor>],
) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let mime = guess_mime(path);
    for ex in extractors {
        if ex.can_handle(path, &mime)
            && let Some(body) = ex.extract(path, max_chars)
        {
            return Some(body);
        }
    }
    None
}

fn looks_binary(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return true;
    }
    let sample = &buf[..buf.len().min(512)];
    let nul = sample.iter().filter(|&&b| b == 0).count();
    if nul > 0 {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| b < 0x09 || (b > 0x0d && b < 0x20) || b == 0x7f)
        .count();
    non_text * 10 > sample.len()
}

/// Minimal HTML → visible text (no HTML5 parser dependency).
pub(crate) fn strip_html(input: &str) -> String {
    let mut s = input.to_string();
    // Drop script / style blocks (case-insensitive, non-greedy-ish via scan).
    s = strip_paired_blocks(&s, "script");
    s = strip_paired_blocks(&s, "style");
    s = strip_paired_blocks(&s, "noscript");

    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut last_space = true;
    for ch in s.chars() {
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        last_space = false;
        // Decode a few common entities inline.
        out.push(ch);
    }
    decode_basic_entities(&out).trim().to_string()
}

fn strip_paired_blocks(input: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = input.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < lower.len() {
        if let Some(rel) = lower[i..].find(&open) {
            let start = i + rel;
            out.push_str(&input[i..start]);
            // Find end of opening tag.
            let after_open = start + open.len();
            let gt = lower[after_open..]
                .find('>')
                .map(|p| after_open + p + 1)
                .unwrap_or(lower.len());
            if let Some(rel_close) = lower[gt..].find(&close) {
                i = gt + rel_close + close.len();
            } else {
                // Unclosed — drop remainder of open tag content conservatively.
                i = gt;
            }
            continue;
        }
        out.push_str(&input[i..]);
        break;
    }
    out
}

fn decode_basic_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn pdftotext_extract(path: &Path, max_chars: usize) -> Option<String> {
    let path = path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("metis-pdftotext".into())
        .spawn(move || {
            let output = Command::new("pdftotext")
                .args(["-q", "-enc", "UTF-8"])
                .arg(&path)
                .arg("-")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            let _ = tx.send(output);
        })
        .ok()?;

    let output = match rx.recv_timeout(PDFTOTEXT_TIMEOUT) {
        Ok(Ok(o)) => o,
        Ok(Err(_)) | Err(_) => return None,
    };
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    let mut buf = output.stdout;
    if buf.len() > PDF_STDOUT_CAP {
        buf.truncate(PDF_STDOUT_CAP);
    }
    if looks_binary(&buf) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out: String = text.chars().take(max_chars).collect();
    out.retain(|c| c == '\n' || c == '\t' || !c.is_control());
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn extracts_utf8_source() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "fn main() {{ println!(\"hi\"); }}").unwrap();
        let path = f.path().with_extension("rs");
        std::fs::copy(f.path(), &path).unwrap();
        let extractors = build_extractors(&SearchExtractors {
            plain_text: true,
            ..Default::default()
        });
        let body = extract_body(&path, 1_000_000, 10_000, &extractors).unwrap();
        assert!(body.contains("println"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn strip_html_removes_tags_and_script() {
        let html = r#"<html><head><style>.x{color:red}</style><script>evil()</script></head>
            <body><h1>Hello&nbsp;World</h1><p>Metis &amp; search</p></body></html>"#;
        let text = strip_html(html);
        assert!(text.contains("Hello World"), "{text}");
        assert!(text.contains("Metis & search"), "{text}");
        assert!(!text.contains("evil"), "{text}");
        assert!(!text.contains("color:red"), "{text}");
        assert!(!text.contains('<'), "{text}");
    }

    #[test]
    fn html_extractor_preferred_over_plain() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            "<html><body><p>VisibleOnly</p><script>hide()</script></body></html>"
        )
        .unwrap();
        let path = f.path().with_extension("html");
        std::fs::copy(f.path(), &path).unwrap();
        let extractors = build_extractors(&SearchExtractors {
            plain_text: true,
            html: true,
            pdf: false,
        });
        let body = extract_body(&path, 1_000_000, 10_000, &extractors).unwrap();
        assert!(body.contains("VisibleOnly"), "{body}");
        assert!(!body.contains("hide()"), "{body}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pdf_extractor_soft_fails_or_extracts() {
        // Minimal valid-enough PDF is hard; just ensure can_handle + missing-file soft-fail.
        let extractors = build_extractors(&SearchExtractors {
            plain_text: false,
            html: false,
            pdf: true,
        });
        assert_eq!(extractors.len(), 1);
        let missing = Path::new("/tmp/metis-no-such-file-xyz.pdf");
        assert!(extract_body(missing, 1_000_000, 1000, &extractors).is_none());

        if Command::new("pdftotext")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            // Create a tiny PDF via printf if we can; otherwise skip content assert.
            // Many systems lack a writer; soft-fail path already covered.
        }
    }
}
