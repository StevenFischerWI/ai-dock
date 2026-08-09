use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use regex::Regex;
use reqwest::{
    Client, Response,
    header::{ACCEPT, RANGE},
};
use tauri::Url;

const MAX_HTML_BYTES: usize = 512 * 1024;
const MAX_ICON_BYTES: usize = 256 * 1024;
const MAX_ICON_CANDIDATES: usize = 16;

pub async fn fetch_web_app_favicon(page_url: &str) -> Option<String> {
    let requested_url = Url::parse(page_url).ok()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("AI-Dock/0.1 favicon loader")
        .build()
        .ok()?;

    let mut candidates = Vec::new();
    let page_response = client
        .get(requested_url.clone())
        .header(ACCEPT, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
        .header(RANGE, format!("bytes=0-{}", MAX_HTML_BYTES - 1))
        .send()
        .await;

    if let Ok(response) = page_response {
        let final_url = response.url().clone();
        if response.status().is_success()
            && let Some(bytes) = read_limited(response, MAX_HTML_BYTES).await
        {
            let html = String::from_utf8_lossy(&bytes);
            candidates.extend(favicon_urls_from_html(&final_url, &html));
        }
        push_root_favicon(&mut candidates, &final_url);
    }
    push_root_favicon(&mut candidates, &requested_url);
    candidates.truncate(MAX_ICON_CANDIDATES);

    for icon_url in candidates {
        if let Some(data_url) = fetch_icon(&client, &icon_url).await {
            return Some(data_url);
        }
    }
    None
}

async fn fetch_icon(client: &Client, icon_url: &Url) -> Option<String> {
    let response = client
        .get(icon_url.clone())
        .header(
            ACCEPT,
            "image/avif,image/webp,image/svg+xml,image/png,image/*,*/*;q=0.1",
        )
        .header(RANGE, format!("bytes=0-{}", MAX_ICON_BYTES - 1))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let declared_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = read_limited(response, MAX_ICON_BYTES).await?;
    let mime = favicon_mime_type(declared_type.as_deref(), &bytes, icon_url)?;
    Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

async fn read_limited(mut response: Response, limit: usize) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return None;
    }
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if result.len() + chunk.len() > limit {
            return None;
        }
        result.extend_from_slice(&chunk);
    }
    (!result.is_empty()).then_some(result)
}

fn favicon_urls_from_html(page_url: &Url, html: &str) -> Vec<Url> {
    let base_url = document_base_url(page_url, html);
    let link_pattern = Regex::new(r"(?is)<link\b[^>]*>").expect("valid link regex");
    let mut scored = Vec::new();

    for (order, link_match) in link_pattern.find_iter(html).enumerate() {
        let attributes = html_attributes(link_match.as_str());
        let Some(rel) = attribute(&attributes, "rel") else {
            continue;
        };
        let rel = rel.to_ascii_lowercase();
        let rel_tokens = rel.split_ascii_whitespace().collect::<Vec<_>>();
        let is_standard_icon = rel_tokens.contains(&"icon");
        let is_alternate_icon = rel_tokens
            .iter()
            .any(|token| token.ends_with("-icon") || *token == "mask-icon");
        if !is_standard_icon && !is_alternate_icon {
            continue;
        }
        let Some(href) = attribute(&attributes, "href") else {
            continue;
        };
        let href = decode_html_url(href);
        let Ok(icon_url) = base_url.join(&href) else {
            continue;
        };
        if !is_safe_remote_url(&icon_url) {
            continue;
        }

        let mut score = if is_standard_icon { 100 } else { 40 };
        if attribute(&attributes, "type").is_some_and(|kind| {
            kind.eq_ignore_ascii_case("image/svg+xml") || kind.eq_ignore_ascii_case("image/png")
        }) {
            score += 30;
        }
        if let Some(sizes) = attribute(&attributes, "sizes") {
            if sizes.eq_ignore_ascii_case("any") {
                score += 40;
            } else {
                score += largest_declared_size(sizes).min(512) / 16;
            }
        }
        scored.push((score, order, icon_url));
    }

    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut result = Vec::new();
    for (_, _, icon_url) in scored {
        if !result.iter().any(|existing: &Url| existing == &icon_url) {
            result.push(icon_url);
        }
    }
    result
}

fn document_base_url(page_url: &Url, html: &str) -> Url {
    let base_pattern = Regex::new(r"(?is)<base\b[^>]*>").expect("valid base regex");
    base_pattern
        .find(html)
        .and_then(|base_match| {
            let attributes = html_attributes(base_match.as_str());
            attribute(&attributes, "href").map(decode_html_url)
        })
        .and_then(|href| page_url.join(&href).ok())
        .filter(is_safe_remote_url)
        .unwrap_or_else(|| page_url.clone())
}

fn html_attributes(tag: &str) -> Vec<(String, String)> {
    let attribute_pattern =
        Regex::new(r#"(?is)([a-z_:][a-z0-9_.:-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#)
            .expect("valid attribute regex");
    attribute_pattern
        .captures_iter(tag)
        .filter_map(|captures| {
            let name = captures.get(1)?.as_str().to_ascii_lowercase();
            let value = captures
                .get(2)
                .or_else(|| captures.get(3))
                .or_else(|| captures.get(4))?
                .as_str()
                .to_string();
            Some((name, value))
        })
        .collect()
}

fn attribute<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, value)| value.as_str())
}

fn decode_html_url(value: &str) -> String {
    value
        .trim()
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
}

fn largest_declared_size(sizes: &str) -> u32 {
    sizes
        .split_ascii_whitespace()
        .filter_map(|size| size.split_once('x'))
        .filter_map(|(width, height)| Some(width.parse::<u32>().ok()?.max(height.parse().ok()?)))
        .max()
        .unwrap_or(0)
}

fn push_root_favicon(candidates: &mut Vec<Url>, page_url: &Url) {
    if let Ok(icon_url) = page_url.join("/favicon.ico")
        && is_safe_remote_url(&icon_url)
        && !candidates.iter().any(|candidate| candidate == &icon_url)
    {
        candidates.push(icon_url);
    }
}

fn is_safe_remote_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn favicon_mime_type<'a>(declared: Option<&'a str>, bytes: &[u8], url: &Url) -> Option<&'a str> {
    if let Some(declared) = declared.map(|value| value.split(';').next().unwrap_or(value).trim()) {
        let normalized = match declared.to_ascii_lowercase().as_str() {
            "image/png" | "image/x-png" => Some("image/png"),
            "image/jpeg" | "image/jpg" => Some("image/jpeg"),
            "image/gif" => Some("image/gif"),
            "image/webp" => Some("image/webp"),
            "image/svg+xml" => Some("image/svg+xml"),
            "image/x-icon" | "image/vnd.microsoft.icon" => Some("image/x-icon"),
            "image/avif" => Some("image/avif"),
            _ => None,
        };
        if let Some(normalized) = normalized {
            return Some(normalized);
        }
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0, 0, 1, 0]) {
        return Some("image/x-icon");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if bytes
        .get(4..12)
        .is_some_and(|marker| marker.starts_with(b"ftypavif") || marker.starts_with(b"ftypavis"))
    {
        return Some("image/avif");
    }
    if String::from_utf8_lossy(&bytes[..bytes.len().min(1_024)]).contains("<svg") {
        return Some("image/svg+xml");
    }
    match url
        .path()
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "ico" => Some("image/x-icon"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "avif" => Some("image/avif"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_declared_icons_and_resolves_relative_urls() {
        let page = Url::parse("https://example.com/planner/today").unwrap();
        let icons = favicon_urls_from_html(
            &page,
            r#"<html><head><link href="../assets/icon.png?x=1&amp;y=2" rel="shortcut icon" sizes="64x64"></head></html>"#,
        );

        assert_eq!(
            icons[0].as_str(),
            "https://example.com/assets/icon.png?x=1&y=2"
        );
    }

    #[test]
    fn honors_document_base_and_prefers_scalable_icons() {
        let page = Url::parse("https://example.com/app/").unwrap();
        let icons = favicon_urls_from_html(
            &page,
            r#"
                <base href="https://cdn.example.com/ui/">
                <link rel="icon" href="small.ico" sizes="16x16">
                <link type="image/svg+xml" href='brand.svg' rel='icon' sizes='any'>
            "#,
        );

        assert_eq!(icons[0].as_str(), "https://cdn.example.com/ui/brand.svg");
        assert_eq!(icons[1].as_str(), "https://cdn.example.com/ui/small.ico");
    }

    #[test]
    fn ignores_non_web_icon_urls() {
        let page = Url::parse("https://example.com/").unwrap();
        let icons = favicon_urls_from_html(
            &page,
            r#"<link rel="icon" href="data:image/svg+xml,bad"><link rel="icon" href="javascript:bad">"#,
        );

        assert!(icons.is_empty());
    }

    #[test]
    fn recognizes_icon_formats_when_servers_use_generic_content_types() {
        let icon = Url::parse("https://example.com/favicon").unwrap();
        assert_eq!(
            favicon_mime_type(None, b"\x89PNG\r\n\x1a\nmore", &icon),
            Some("image/png")
        );
        assert_eq!(
            favicon_mime_type(Some("application/octet-stream"), &[0, 0, 1, 0, 1], &icon),
            Some("image/x-icon")
        );
    }
}
