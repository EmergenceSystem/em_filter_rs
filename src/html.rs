use scraper::{Html, Selector, Element};
use regex::Regex;
use std::sync::OnceLock;
use crate::EmFilterError;

/// Removes all `<script>…</script>` blocks from an HTML string.
///
/// The match is case-insensitive and handles multi-line script bodies. Inline
/// event attributes (`onclick="…"`) are **not** removed — use [`get_text`] if
/// you need attribute-free plain text.
///
/// Always succeeds — the regex is a compile-time constant. The `Result` return
/// type is kept for API consistency with callers that handle HTML errors uniformly.
///
/// # Example
///
/// ```
/// # use em_filter::strip_scripts;
/// let html = r#"<p>Hello</p><script>alert("xss")</script><p>World</p>"#;
/// assert_eq!(strip_scripts(html).unwrap(), "<p>Hello</p><p>World</p>");
/// ```
pub fn strip_scripts(html: &str) -> Result<String, EmFilterError> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?si)<script[^>]*>.*?</script>").unwrap()
    });
    Ok(re.replace_all(html, "").into_owned())
}

/// Strips all HTML tags from a string, returning plain text.
///
/// Text nodes are concatenated in document order without any separator. Useful
/// for feeding scraped content to a search index or an LLM.
///
/// # Example
///
/// ```
/// # use em_filter::get_text;
/// assert_eq!(get_text("<p>Hello <b>world</b></p>"), "Hello world");
/// ```
pub fn get_text(html: &str) -> String {
    let doc = Html::parse_fragment(html);
    doc.root_element().text().collect::<Vec<_>>().join("")
}

/// Extracts elements matching a CSS selector, returning the inner HTML of each match.
///
/// Accepts any CSS selector supported by the [`scraper`](https://docs.rs/scraper) crate:
/// `tag`, `.class`, `#id`, `tag.class`, `[attr=value]`, descendant combinators, etc.
///
/// Returns an empty `Vec` if the selector is syntactically invalid or no elements match.
///
/// # Example
///
/// ```
/// # use em_filter::extract_elements;
/// let html = r#"<ul><li class="item">One</li><li class="item">Two</li></ul>"#;
/// let items = extract_elements(html, "li.item");
/// assert_eq!(items, vec!["One", "Two"]);
/// ```
pub fn extract_elements(html: &str, selector: &str) -> Vec<String> {
    let sel = match Selector::parse(selector) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let doc = Html::parse_document(html);
    doc.select(&sel).map(|el| el.inner_html()).collect()
}

/// Extracts the value of an HTML attribute from the first element in a fragment.
///
/// Returns `None` if the fragment contains no elements or the attribute is absent.
///
/// # Example
///
/// ```
/// # use em_filter::extract_attribute;
/// let html = r#"<a href="https://example.com">link</a>"#;
/// assert_eq!(
///     extract_attribute(html, "href"),
///     Some("https://example.com".to_string())
/// );
/// assert_eq!(extract_attribute(html, "class"), None);
/// ```
pub fn extract_attribute(html: &str, attr: &str) -> Option<String> {
    let doc = Html::parse_fragment(html);
    doc.root_element()
        .first_element_child()?
        .value()
        .attr(attr)
        .map(|s| s.to_string())
}

/// Decodes `&#N;`, `&#xHH;`, and `&name;` HTML entities in a string.
///
/// Named entities supported:
/// `&nbsp;`, `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`,
/// and the common Western European accented characters:
/// `&eacute;`, `&egrave;`, `&agrave;`, `&ccedil;`, `&ocirc;`, `&ecirc;`,
/// `&icirc;`, `&ugrave;`, `&aacute;`.
///
/// This set matches the Erlang `em_filter` library. For full HTML5 entity
/// coverage, pass the text through a dedicated HTML parser first.
///
/// # Example
///
/// ```
/// # use em_filter::decode_html_entities;
/// assert_eq!(decode_html_entities("caf&eacute;"), "café");
/// assert_eq!(decode_html_entities("&#x41;"),      "A");
/// assert_eq!(decode_html_entities("&#233;"),       "é");
/// assert_eq!(decode_html_entities("&amp;"),        "&");
/// ```
pub fn decode_html_entities(text: &str) -> String {
    let s = decode_numeric_entities(text);
    let s = decode_hex_entities(&s);
    decode_named_entities(&s)
}

/// Returns `true` if a link should be skipped by a web scraper.
///
/// A link is skipped when:
/// - It does not start with `http` (relative paths, `ftp://`, `javascript:`, etc.)
/// - Its URL contains any substring from `excluded`
///
/// # Example
///
/// ```
/// # use em_filter::should_skip_link;
/// assert!(should_skip_link("/relative/path", &[]));
/// assert!(should_skip_link("ftp://files.example.com", &[]));
/// assert!(should_skip_link("https://ads.example.com", &["ads.example.com"]));
/// assert!(!should_skip_link("https://example.com", &["ads.example.com"]));
/// ```
pub fn should_skip_link(link: &str, excluded: &[&str]) -> bool {
    if !link.starts_with("http") {
        return true;
    }
    excluded.iter().any(|pat| link.contains(pat))
}

// ── Private helpers ──────────────────────────────────────────────────

/// Decodes decimal numeric HTML entities: `&#233;` → `é`.
fn decode_numeric_entities(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"&#([0-9]+);").unwrap());
    re.replace_all(text, |caps: &regex::Captures| {
        let n: u32 = caps[1].parse().unwrap_or(0);
        char::from_u32(n).map(|c| c.to_string()).unwrap_or_default()
    })
    .into_owned()
}

/// Decodes hexadecimal numeric HTML entities: `&#x41;` / `&#X41;` → `A`.
fn decode_hex_entities(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"&#[xX]([0-9A-Fa-f]+);").unwrap());
    re.replace_all(text, |caps: &regex::Captures| {
        let n = u32::from_str_radix(&caps[1], 16).unwrap_or(0);
        char::from_u32(n).map(|c| c.to_string()).unwrap_or_default()
    })
    .into_owned()
}

/// Decodes named HTML entities using the same set as the Erlang em_filter library.
fn decode_named_entities(text: &str) -> String {
    const ENTITIES: &[(&str, &str)] = &[
        ("&nbsp;",   " "),
        ("&amp;",    "&"),
        ("&lt;",     "<"),
        ("&gt;",     ">"),
        ("&quot;",   "\""),
        ("&apos;",   "'"),
        ("&eacute;", "é"),
        ("&egrave;", "è"),
        ("&agrave;", "à"),
        ("&ccedil;", "ç"),
        ("&ocirc;",  "ô"),
        ("&ecirc;",  "ê"),
        ("&icirc;",  "î"),
        ("&ugrave;", "ù"),
        ("&aacute;", "á"),
    ];
    let mut result = text.to_string();
    for (entity, replacement) in ENTITIES {
        result = result.replace(entity, replacement);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_scripts_removes_script_blocks() {
        let html = r#"<p>Hello</p><script>alert('xss')</script><p>World</p>"#;
        let result = strip_scripts(html).unwrap();
        assert_eq!(result, "<p>Hello</p><p>World</p>");
    }

    #[test]
    fn test_strip_scripts_multiline() {
        let html = "<p>A</p><script>\nvar x = 1;\n</script><p>B</p>";
        let result = strip_scripts(html).unwrap();
        assert_eq!(result, "<p>A</p><p>B</p>");
    }

    #[test]
    fn test_get_text_strips_tags() {
        let html = "<p>Hello <b>world</b></p>";
        assert_eq!(get_text(html), "Hello world");
    }

    #[test]
    fn test_get_text_plain_string() {
        assert_eq!(get_text("no tags here"), "no tags here");
    }

    #[test]
    fn test_extract_elements_by_class() {
        let html = r#"<ul><li class="item">One</li><li class="item">Two</li></ul>"#;
        let items = extract_elements(html, "li.item");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], "One");
        assert_eq!(items[1], "Two");
    }

    #[test]
    fn test_extract_elements_invalid_selector_returns_empty() {
        let items = extract_elements("<p>text</p>", ":::invalid");
        assert!(items.is_empty());
    }

    #[test]
    fn test_extract_attribute_present() {
        let html = r#"<a href="https://example.com">link</a>"#;
        assert_eq!(
            extract_attribute(html, "href"),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn test_extract_attribute_absent_returns_none() {
        let html = r#"<a>link</a>"#;
        assert_eq!(extract_attribute(html, "href"), None);
    }

    #[test]
    fn test_decode_numeric_entity() {
        assert_eq!(decode_html_entities("caf&#233;"), "café");
    }

    #[test]
    fn test_decode_hex_entity() {
        assert_eq!(decode_html_entities("&#x41;"), "A");
    }

    #[test]
    fn test_decode_named_entity_amp() {
        assert_eq!(decode_html_entities("fish &amp; chips"), "fish & chips");
    }

    #[test]
    fn test_decode_named_entity_accents() {
        assert_eq!(decode_html_entities("caf&eacute;"), "café");
        assert_eq!(decode_html_entities("&agrave;"), "à");
    }

    #[test]
    fn test_should_skip_non_http() {
        assert!(should_skip_link("ftp://example.com", &[]));
        assert!(should_skip_link("/relative/path", &[]));
        assert!(should_skip_link("", &[]));
    }

    #[test]
    fn test_should_skip_excluded_pattern() {
        assert!(should_skip_link(
            "https://ads.example.com/track",
            &["ads.example.com"]
        ));
    }

    #[test]
    fn test_should_not_skip_valid_link() {
        assert!(!should_skip_link("https://example.com", &[]));
        assert!(!should_skip_link("http://example.com/page", &["other.com"]));
    }
}
