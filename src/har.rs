use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine as _;
use serde::Deserialize;

#[derive(Deserialize)]
struct Har {
    log: Log,
}

#[derive(Deserialize)]
struct Log {
    #[serde(default)]
    entries: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    request: RawRequest,
    response: Option<RawResponse>,
}

#[derive(Deserialize)]
struct RawRequest {
    url: String,
}

#[derive(Deserialize)]
struct RawResponse {
    status: Option<i64>,
    content: Option<RawContent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawContent {
    mime_type: Option<String>,
    text: Option<String>,
    encoding: Option<String>,
}

/// One request/response pair from the HAR file.
pub struct Entry {
    pub url: String,
    pub host: String,
    /// Percent-decoded URL path, e.g. `/assets/logo.png`.
    pub path: String,
    pub status: i64,
    pub mime: String,
    pub ext: String,
    pub category: &'static str,
    /// Decoded body size in bytes (0 when the HAR has no body).
    pub size: usize,
    /// Decoded body bytes (base64 already decoded).
    body: Option<Vec<u8>>,
    /// Set when the body was marked base64 but could not be decoded.
    body_error: Option<String>,
}

impl Entry {
    pub fn has_body(&self) -> bool {
        self.body.as_ref().is_some_and(|b| !b.is_empty()) || self.body_error.is_some()
    }

    /// The decoded response body, or `None` if the HAR did not record it.
    pub fn body(&self) -> Option<Result<&[u8]>> {
        if let Some(err) = &self.body_error {
            return Some(Err(anyhow::anyhow!("{err}")));
        }
        self.body.as_deref().filter(|b| !b.is_empty()).map(Ok)
    }

    /// URL shortened for display (data: URLs can be megabytes long).
    pub fn display_url(&self) -> String {
        if self.url.starts_with("data:") && self.url.len() > 80 {
            let cut = self.url.char_indices().nth(60).map(|(i, _)| i).unwrap_or(self.url.len());
            format!("{}…", &self.url[..cut])
        } else {
            self.url.clone()
        }
    }
}

pub fn load(path: &Path) -> Result<Vec<Entry>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let har: Har = serde_json::from_str(&raw).context("not a valid HAR file")?;
    Ok(har
        .log
        .entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| convert(i, e))
        .collect())
}

/// Lenient base64: ignores whitespace, missing/extra padding, accepts standard and URL-safe alphabets.
pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    use base64::alphabet;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let clean = clean.trim_end_matches('=');
    let cfg = GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true);
    GeneralPurpose::new(&alphabet::STANDARD, cfg)
        .decode(clean)
        .or_else(|_| GeneralPurpose::new(&alphabet::URL_SAFE, cfg).decode(clean))
        .ok()
}

fn looks_like_base64(s: &str) -> bool {
    let mut n = 0;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_' | '=') {
            n += 1;
        } else if !c.is_whitespace() {
            return false;
        }
    }
    n >= 4
}

/// Mime types whose body can never be plain text, so text in the HAR must be base64.
fn is_binary_mime(m: &str) -> bool {
    (m.starts_with("image/") && m != "image/svg+xml")
        || m.starts_with("font/")
        || m.starts_with("video/")
        || m.starts_with("audio/")
        || m.contains("woff")
        || matches!(
            m,
            "application/octet-stream"
                | "application/wasm"
                | "application/pdf"
                | "application/zip"
                | "application/gzip"
                | "application/x-font-ttf"
                | "application/vnd.ms-fontobject"
                | "application/x-protobuf"
                | "application/protobuf"
        )
}

/// `data:image/png;base64,AAAA` -> (mime, bytes)
fn parse_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mut parts = meta.split(';');
    let mime = parts.next().unwrap_or("").trim().to_lowercase();
    let is_b64 = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    let bytes = if is_b64 {
        let decoded = percent_encoding::percent_decode_str(payload).decode_utf8_lossy();
        decode_base64(&decoded)?
    } else {
        percent_encoding::percent_decode_str(payload).collect()
    };
    Some((if mime.is_empty() { "text/plain".into() } else { mime }, bytes))
}

fn convert(index: usize, e: RawEntry) -> Entry {
    let (status, content) = match e.response {
        Some(r) => (r.status.unwrap_or(0), r.content),
        None => (0, None),
    };
    let (mut mime, text, encoding) = match content {
        Some(c) => (
            c.mime_type
                .unwrap_or_default()
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase(),
            c.text.filter(|t| !t.is_empty()),
            c.encoding.unwrap_or_default().trim().to_lowercase(),
        ),
        None => (String::new(), None, String::new()),
    };

    let mut body_error = None;
    let mut body = text.map(|t| {
        if encoding == "base64" {
            decode_base64(&t).unwrap_or_else(|| {
                body_error = Some("body is marked base64 but could not be decoded".into());
                Vec::new()
            })
        } else if is_binary_mime(&mime) && looks_like_base64(&t) {
            // Some tools store binary as base64 without setting "encoding".
            decode_base64(&t).unwrap_or_else(|| t.into_bytes())
        } else {
            t.into_bytes()
        }
    });

    let (host, path) = if e.request.url.starts_with("data:") {
        let data = parse_data_url(&e.request.url);
        if let Some((data_mime, bytes)) = data {
            if body.as_ref().is_none_or(|b| b.is_empty()) {
                body = Some(bytes);
                body_error = None;
            }
            if mime.is_empty() || mime == "application/octet-stream" {
                mime = data_mime;
            }
        }
        let name = match ext_for_mime(&mime) {
            Some(ext) => format!("/data-{index}.{ext}"),
            None => format!("/data-{index}"),
        };
        ("data-url".to_string(), name)
    } else {
        let parsed = url::Url::parse(&e.request.url).ok();
        let host = parsed
            .as_ref()
            .and_then(|u| {
                u.host_str().map(|h| match u.port() {
                    Some(p) => format!("{h}:{p}"),
                    None => h.to_string(),
                })
            })
            .unwrap_or_else(|| "unknown".into());
        let path = parsed
            .as_ref()
            .map(|u| percent_encoding::percent_decode_str(u.path()).decode_utf8_lossy().into_owned())
            .unwrap_or_default();
        (host, path)
    };

    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| Path::new(name).extension())
        .map(|x| x.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    Entry {
        category: categorize(&mime, &ext),
        size: body.as_ref().map_or(0, Vec::len),
        url: e.request.url,
        host,
        path,
        status,
        mime,
        ext,
        body,
        body_error,
    }
}

fn category_from_mime(mime: &str) -> Option<&'static str> {
    let m = mime;
    Some(if m.starts_with("image/") {
        "image"
    } else if m.contains("javascript") || m.contains("ecmascript") {
        "script"
    } else if m == "text/css" {
        "css"
    } else if m.starts_with("font/") || m.contains("woff") || m.contains("font") {
        "font"
    } else if m.contains("json") {
        "json"
    } else if m.contains("html") {
        "html"
    } else if m.contains("xml") {
        "xml"
    } else if m.starts_with("video/") {
        "video"
    } else if m.starts_with("audio/") {
        "audio"
    } else if m.contains("wasm") {
        "wasm"
    } else if m.starts_with("text/") {
        "text"
    } else {
        return None;
    })
}

fn category_from_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "bmp" => "image",
        "js" | "mjs" | "cjs" => "script",
        "css" => "css",
        "woff" | "woff2" | "ttf" | "otf" | "eot" => "font",
        "json" | "map" => "json",
        "html" | "htm" => "html",
        "xml" => "xml",
        "mp4" | "webm" | "mov" | "m3u8" | "ts" => "video",
        "mp3" | "wav" | "ogg" | "m4a" | "aac" => "audio",
        "wasm" => "wasm",
        "txt" => "text",
        _ => return None,
    })
}

/// Mime type wins, except for vague ones (text/plain, octet-stream...) where the extension is more telling.
fn categorize(mime: &str, ext: &str) -> &'static str {
    match category_from_mime(mime) {
        Some(c) if c != "text" => c,
        by_mime => category_from_ext(ext).or(by_mime).unwrap_or("other"),
    }
}

pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/avif" => "avif",
        "text/css" => "css",
        "text/html" => "html",
        "text/plain" => "txt",
        "application/javascript" | "text/javascript" | "application/x-javascript" => "js",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "font/woff2" => "woff2",
        "font/woff" | "application/font-woff" => "woff",
        "font/ttf" => "ttf",
        "font/otf" => "otf",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "application/wasm" => "wasm",
        _ => return None,
    })
}

pub fn format_size(n: usize) -> String {
    let n = n as f64;
    if n < 1024.0 {
        format!("{n} B")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / 1024.0 / 1024.0)
    }
}

/// Parses sizes like `500`, `10k`, `1.5mb`.
pub fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return Some(0);
    }
    let (num, mul) = if let Some(n) = s.strip_suffix("kb").or_else(|| s.strip_suffix('k')) {
        (n, 1024.0)
    } else if let Some(n) = s.strip_suffix("mb").or_else(|| s.strip_suffix('m')) {
        (n, 1024.0 * 1024.0)
    } else {
        (s.strip_suffix('b').unwrap_or(&s), 1.0)
    };
    let v: f64 = num.trim().parse().ok()?;
    (v >= 0.0).then(|| (v * mul) as usize)
}
