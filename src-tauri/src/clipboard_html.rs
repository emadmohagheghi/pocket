//! Reading HTML from the Windows clipboard (`CF_HTML`).
//!
//! The text-capture flow (double-Shift) synthesizes a Ctrl+C into the
//! foreground app, so the clipboard briefly holds whatever that app copied.
//! Rich sources (browsers, Word, editors) place an HTML flavor next to the
//! plain text; the HTML flavor carries the formatting that plain text loses,
//! so the grab prefers it (converted to markdown) and falls back to plain.
//!
//! `CF_HTML` wraps a UTF-8 fragment with a versioned text header; the
//! offsets are byte offsets and some writers omit them, so both header and
//! fallback parsing are handled defensively.

/// `RegisterClipboardFormat("HTML Format")` result. Cached on first use;
/// the value is stable for the lifetime of the process.
#[cfg(windows)]
fn html_format() -> u32 {
    use windows::Win32::System::DataExchange::RegisterClipboardFormatA;
    use windows::core::PCSTR;

    const HTML_FORMAT_NAME: &[u8; 12] = b"HTML Format\0";
    // SAFETY: literal NUL-terminated string, no pointers retained.
    unsafe { RegisterClipboardFormatA(PCSTR::from_raw(HTML_FORMAT_NAME.as_ptr())) }
}

/// Whether the current clipboard carries an HTML flavor. Cheap poll used to
/// decide whether to bother reading and converting it.
#[cfg(windows)]
pub fn has_html() -> bool {
    use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, OpenClipboard};
    let fmt = html_format();
    // OpenClipboard serializes with other readers/writers; IsClipboardFormatAvailable
    // works on the open clipboard, so retry briefly like read_clipboard_text does.
    for _ in 0..5 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            let available = unsafe { IsClipboardFormatAvailable(fmt) }.is_ok();
            unsafe { let _ = windows::Win32::System::DataExchange::CloseClipboard(); }
            return available;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[cfg(not(windows))]
pub fn has_html() -> bool {
    false
}

/// The current clipboard's HTML flavor, if any. Empty on failure.
#[cfg(windows)]
pub fn read_html() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    let fmt = html_format();
    let mut opened = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !opened {
        return None;
    }

    let result = unsafe {
        if IsClipboardFormatAvailable(fmt).is_err() {
            None
        } else {
            match GetClipboardData(fmt) {
                Ok(h) if !h.0.is_null() => {
                    // CF_HTML is UTF-8; get the byte length via GlobalSize.
                    let size = windows::Win32::System::Memory::GlobalSize(HGLOBAL(h.0));
                    let ptr = GlobalLock(HGLOBAL(h.0)) as *const u8;
                    if ptr.is_null() || size == 0 {
                        None
                    } else {
                        let bytes = std::slice::from_raw_parts(ptr, size);
                        let raw = String::from_utf8_lossy(bytes).into_owned();
                        let _ = GlobalUnlock(HGLOBAL(h.0));
                        Some(raw)
                    }
                }
                _ => None,
            }
        }
    };
    unsafe { let _ = CloseClipboard(); };
    result.and_then(|raw| extract_cf_html_fragment(&raw))
}

#[cfg(not(windows))]
pub fn read_html() -> Option<String> {
    None
}

/// Pull the markup out of a `CF_HTML` blob: prefer the header's
/// StartFragment/EndFragment byte offsets, fall back to the
/// `<!--StartFragment-->` comment markers, and as a last resort take the
/// whole payload (headers are line-based and harmless-ish to a lenient
/// parser, but the two structured paths cover real writers).
pub(crate) fn extract_cf_html_fragment(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }

    // Header values live in the leading (up to blank line) section.
    let (header, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    for line in header.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let Ok(n) = value.parse::<usize>() else {
            continue;
        };
        match key.trim().to_ascii_uppercase().as_str() {
            "STARTFRAGMENT" => start = Some(n),
            "ENDFRAGMENT" => end = Some(n),
            _ => {}
        }
    }

    if let (Some(s), Some(e)) = (start, end) {
        // Offsets are measured from the very start of the blob, in bytes.
        if s < e && e <= raw.len() {
            if let Some(frag) = raw.get(s..e) {
                if !frag.trim().is_empty() {
                    return Some(frag.to_string());
                }
            }
        }
    }

    // Fallback: fragment comment markers.
    if let (Some(s), Some(e)) = (
        raw.find("<!--StartFragment-->"),
        raw.find("<!--EndFragment-->"),
    ) {
        if s < e {
            let from = s + "<!--StartFragment-->".len();
            if let Some(frag) = raw.get(from..e) {
                if !frag.trim().is_empty() {
                    return Some(frag.to_string());
                }
            }
        }
    }

    // Last resort: everything after the header's blank line.
    let body = body.trim();
    if !body.is_empty() {
        return Some(body.to_string());
    }
    // Header-less payload (some writers): the whole thing.
    (!raw.trim().is_empty()).then(|| raw.to_string())
}

/// Convert an HTML fragment to markdown. Shared by the grab flow (backend)
/// and the paste handler (frontend command). Strips image tags entirely —
/// notes' images are first-class attachments, never remote hotlinks.
pub fn html_to_markdown(html: &str) -> String {
    let result = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["img", "video", "audio", "style", "script"])
        .build()
        .convert(html);
    match result {
        Ok(md) => clean_markdown(&md),
        Err(_) => String::new(),
    }
}

/// Tidy conversion output for note storage: drop whitespace-only results and
/// collapse more than two consecutive blank lines (htmd emits blank lines
/// around headings/lists that bloat short notes).
fn clean_markdown(md: &str) -> String {
    let trimmed = md.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut blank_run = 0usize;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 2 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_extraction_prefers_header_offsets() {
        // Build a CF_HTML blob the way real writers do: a line header whose
        // StartFragment/EndFragment byte offsets bracket the fragment.
        let fragment = "<b>bold</b>";
        let head_marker = "<html><body><!--StartFragment-->";
        let tail_marker = "<!--EndFragment--></body></html>";
        // 7-char numeric placeholders keep the header length deterministic.
        let header_tmpl = "Version:0.9\r\nStartHTML:0000000\r\nEndHTML:0000000\r\nStartFragment:0000000\r\nEndFragment:0000000\r\n";
        let start = header_tmpl.len() + head_marker.len();
        let end = start + fragment.len();
        let total = end + tail_marker.len();
        let header = header_tmpl
            .replacen("0000000", &format!("{start:07}"), 1)
            .replacen("0000000", &format!("{total:07}"), 1)
            .replacen("0000000", &format!("{start:07}"), 1)
            .replacen("0000000", &format!("{end:07}"), 1);
        let blob = format!("{header}{head_marker}{fragment}{tail_marker}");
        assert_eq!(extract_cf_html_fragment(&blob).unwrap(), fragment);
    }

    #[test]
    fn fragment_extraction_falls_back_to_comment_markers() {
        let blob = "Version:0.9\r\nStartHTML:0\r\nEndHTML:0\r\n\r\n<html><!--StartFragment--><i>it</i><!--EndFragment--></html>";
        assert_eq!(extract_cf_html_fragment(blob).unwrap(), "<i>it</i>");
    }

    #[test]
    fn html_to_markdown_basics() {
        assert_eq!(html_to_markdown("<b>bold</b>"), "**bold**");
        assert_eq!(html_to_markdown("<i>it</i>"), "*it*");
        assert_eq!(
            html_to_markdown("<a href=\"https://x.co\">x</a>"),
            "[x](https://x.co)"
        );
        assert!(html_to_markdown("<h1>Title</h1>").starts_with("# Title"));
        let list = html_to_markdown("<ul><li>a</li><li>b</li></ul>");
        assert!(list.contains("a") && list.contains("b"), "list out: {list}");
    }

    #[test]
    fn html_to_markdown_strips_images() {
        assert_eq!(html_to_markdown("<img src=\"x.png\">pic"), "pic");
    }

    #[test]
    fn html_to_markdown_plain_is_lossless() {
        assert_eq!(html_to_markdown("just text"), "just text");
    }

    #[test]
    fn html_to_markdown_empty() {
        assert_eq!(html_to_markdown(""), "");
        assert_eq!(html_to_markdown("<img src=\"a.png\">"), "");
    }

    #[test]
    fn clean_markdown_collapses_blank_runs() {
        assert_eq!(clean_markdown("a\n\n\n\n\nb"), "a\n\n\nb");
    }
}
