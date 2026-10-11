//! Native address-bar input only. CLI navigation keeps its absolute-URL policy.

use std::{fmt, net::Ipv6Addr};

const MAX_URL_BYTES: usize = 8192;
const SEARCH_URL: &str = "https://www.google.com/search?q=";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressError {
    UnsupportedScheme,
    Credentials,
    InvalidAddress,
    TooLong,
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedScheme => "http 또는 https 주소만 열 수 있습니다.",
            Self::Credentials => "사용자 이름이나 비밀번호가 포함된 주소는 열 수 없습니다.",
            Self::InvalidAddress => "주소 형식을 확인해 주세요.",
            Self::TooLong => "주소나 검색어가 너무 깁니다.",
        })
    }
}

impl std::error::Error for AddressError {}

/// Empty input does nothing. A recognizable host opens directly; other plain
/// text becomes a Google search. Schemes and credentials never become searches.
pub fn resolve(input: &str) -> Result<Option<String>, AddressError> {
    let value = input.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_URL_BYTES {
        return Err(AddressError::TooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(AddressError::InvalidAddress);
    }

    let authority = authority(value);
    if authority
        .split_once('@')
        .is_some_and(|(userinfo, _)| userinfo.contains(':'))
    {
        return Err(AddressError::Credentials);
    }

    // These familiar search operators contain a colon but are not URI schemes.
    if value.split_once(':').is_some_and(|(operator, query)| {
        !query.trim().is_empty()
            && !query.starts_with("//")
            && ["site", "filetype", "intitle"]
                .iter()
                .any(|supported| operator.eq_ignore_ascii_case(supported))
    }) {
        return search(value).map(Some);
    }

    if let Some((scheme, _)) = value.split_once("://") {
        if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http") {
            return normalize_url(value).map(Some);
        }
        if is_scheme(scheme) {
            return Err(AddressError::UnsupportedScheme);
        }
    }

    // A dotted host or localhost followed by a port is an address, although
    // `host:port` also resembles a URI scheme. Validate its port below.
    if looks_like_host(authority) {
        let host = authority.split(':').next().unwrap_or(authority);
        let local = host.eq_ignore_ascii_case("localhost")
            || host.parse::<std::net::Ipv4Addr>().is_ok()
            || authority.starts_with('[');
        return web_url(if local { "http" } else { "https" }, value).map(Some);
    }

    if value
        .split_once(':')
        .is_some_and(|(scheme, _)| is_scheme(scheme))
    {
        return Err(AddressError::UnsupportedScheme);
    }

    search(value).map(Some)
}

/// Normalize an explicit HTTP(S) URL without host completion or search fallback.
/// Bookmark storage uses this path; CLI URL acceptance remains unchanged.
pub fn normalize_url(input: &str) -> Result<String, AddressError> {
    let value = input.trim();
    if value.is_empty() {
        return Err(AddressError::InvalidAddress);
    }
    if value.len() > MAX_URL_BYTES {
        return Err(AddressError::TooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(AddressError::InvalidAddress);
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(AddressError::UnsupportedScheme);
    };
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return Err(AddressError::UnsupportedScheme);
    }
    web_url(&scheme.to_ascii_lowercase(), rest)
}

fn search(value: &str) -> Result<String, AddressError> {
    // An operator or other search text can precede a pasted URL. Never send
    // that URL's userinfo to the search provider.
    if value
        .match_indices("://")
        .any(|(index, _)| authority(&value[index + 3..]).contains('@'))
    {
        return Err(AddressError::Credentials);
    }
    let mut url = String::from(SEARCH_URL);
    encode_component(value, &mut url, false);
    bounded(url)
}

fn authority(value: &str) -> &str {
    value.split(['/', '?', '#']).next().unwrap_or("")
}

fn is_scheme(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte))
}

fn looks_like_host(authority: &str) -> bool {
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return false;
    }
    if authority.starts_with('[') {
        return true;
    }
    let host = authority.split(':').next().unwrap_or(authority);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::Ipv4Addr>().is_ok()
        || (host.contains('.') && host.is_ascii())
}

fn web_url(scheme: &str, rest: &str) -> Result<String, AddressError> {
    let authority = authority(rest);
    if authority.is_empty() || rest.contains('\\') {
        return Err(AddressError::InvalidAddress);
    }
    if authority.contains('@') {
        return Err(AddressError::Credentials);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or(AddressError::InvalidAddress)?;
        host.parse::<Ipv6Addr>()
            .map_err(|_| AddressError::InvalidAddress)?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or(AddressError::InvalidAddress)?,
            )
        };
        (None, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        (Some(host), port)
    };
    if let Some(host) = host {
        let host = host.strip_suffix('.').unwrap_or(host);
        if host.is_empty()
            || host.split('.').any(|label| {
                label.is_empty()
                    || !label
                        .chars()
                        .all(|character| character.is_alphanumeric() || "-_".contains(character))
            })
        {
            return Err(AddressError::InvalidAddress);
        }
    }
    if port.is_some_and(|port| {
        port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || port.parse::<u16>().is_err()
    }) {
        return Err(AddressError::InvalidAddress);
    }

    let mut url = format!("{scheme}://{authority}");
    // Preserve URL separators and valid existing escapes, but encode spaces and
    // UTF-8 path/query bytes so the browser's whitespace-free validator accepts it.
    encode_component(&rest[authority.len()..], &mut url, true);
    bounded(url)
}

fn encode_component(value: &str, output: &mut String, url_tail: bool) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let bytes = value.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        let unreserved = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
        let separator = url_tail && b"/?#[]@!$&'()*+,;=:".contains(&byte);
        let escape = url_tail
            && byte == b'%'
            && bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
            && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit);
        if unreserved || separator || escape {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
}

fn bounded(url: String) -> Result<String, AddressError> {
    if url.len() > MAX_URL_BYTES {
        Err(AddressError::TooLong)
    } else {
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(value: &str) -> String {
        resolve(value).unwrap().unwrap()
    }

    #[test]
    fn empty_input_is_not_navigation() {
        assert_eq!(resolve(""), Ok(None));
        assert_eq!(resolve(" \n\t "), Ok(None));
    }

    #[test]
    fn explicit_url_normalization_never_searches_or_completes_a_host() {
        assert_eq!(
            normalize_url(" HTTPS://Example.com/a b?q=한 ").unwrap(),
            "https://Example.com/a%20b?q=%ED%95%9C"
        );
        for input in [
            "example.com",
            "localhost:3000",
            "한글 검색",
            "site:example.com",
            "about:blank",
        ] {
            assert!(normalize_url(input).is_err(), "{input}");
        }
        assert_eq!(normalize_url(""), Err(AddressError::InvalidAddress));
        assert_eq!(
            normalize_url("http://user:secret@example.com"),
            Err(AddressError::Credentials)
        );
    }

    #[test]
    fn recognizes_web_addresses_and_local_development_hosts() {
        for (input, expected) in [
            ("  HTTPS://example.com/path  ", "https://example.com/path"),
            ("http://intranet/path", "http://intranet/path"),
            ("example.com/path", "https://example.com/path"),
            ("example.com:8443/path", "https://example.com:8443/path"),
            ("localhost:3000/a", "http://localhost:3000/a"),
            ("127.0.0.1", "http://127.0.0.1"),
            ("127.0.0.1:8123/?x=1", "http://127.0.0.1:8123/?x=1"),
            ("[::1]:3000/a", "http://[::1]:3000/a"),
            ("https://[2001:db8::1]/", "https://[2001:db8::1]/"),
        ] {
            assert_eq!(resolved(input), expected, "{input}");
        }
    }

    #[test]
    fn searches_plain_text_with_utf8_percent_encoding() {
        assert_eq!(
            resolved("한글 검색"),
            format!("{SEARCH_URL}%ED%95%9C%EA%B8%80%20%EA%B2%80%EC%83%89")
        );
        assert_eq!(
            resolved("rust & c++"),
            format!("{SEARCH_URL}rust%20%26%20c%2B%2B")
        );
        assert_eq!(
            resolved("example.com tutorial"),
            format!("{SEARCH_URL}example.com%20tutorial")
        );
        assert_eq!(resolved("100%"), format!("{SEARCH_URL}100%25"));
    }

    #[test]
    fn recognizes_only_supported_search_operator_prefixes() {
        for (input, query) in [
            ("site:github.com rust", "site%3Agithub.com%20rust"),
            ("filetype:pdf budget", "filetype%3Apdf%20budget"),
            ("intitle:rust", "intitle%3Arust"),
            ("SITE:example.com", "SITE%3Aexample.com"),
        ] {
            assert_eq!(resolved(input), format!("{SEARCH_URL}{query}"));
        }
        assert_eq!(resolve("site:"), Err(AddressError::UnsupportedScheme));
        assert_eq!(
            resolve("site://user:secret@example.com"),
            Err(AddressError::UnsupportedScheme)
        );
        assert_eq!(resolve("foo:bar"), Err(AddressError::UnsupportedScheme));
        assert_eq!(
            resolve("site:user:secret@example.com"),
            Err(AddressError::Credentials)
        );
    }

    #[test]
    fn rejects_credentials_in_urls_embedded_in_searches() {
        for operator in ["site", "intitle", "filetype"] {
            for url in [
                "https://user:secret@example.com",
                "http://user@example.com/path",
                "HTTPS://user:secret@example.com:8443/",
            ] {
                let input = format!("{operator}:{url}");
                assert_eq!(resolve(&input), Err(AddressError::Credentials), "{input}");
            }
        }
        assert_eq!(
            resolve("find https://user:secret@example.com"),
            Err(AddressError::Credentials)
        );
        assert_eq!(
            resolve("find https://example.com/,https://user:secret@example.org/"),
            Err(AddressError::Credentials)
        );
        assert_eq!(
            resolved("site:https://example.com/@name?q=user@example.com"),
            format!(
                "{SEARCH_URL}site%3Ahttps%3A%2F%2Fexample.com%2F%40name%3Fq%3Duser%40example.com"
            )
        );
    }

    #[test]
    fn encodes_url_tail_without_changing_existing_escapes_or_separators() {
        assert_eq!(
            resolved("name.com?q=a b&x=c+d#part one"),
            "https://name.com?q=a%20b&x=c+d#part%20one"
        );
        assert_eq!(
            resolved("https://example.com/한글/%20/%zz"),
            "https://example.com/%ED%95%9C%EA%B8%80/%20/%25zz"
        );
    }

    #[test]
    fn never_turns_schemes_or_credentials_into_search_queries() {
        for input in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///tmp/a",
            "custom:secret",
            "custom://host",
            "about:blank",
        ] {
            assert_eq!(
                resolve(input),
                Err(AddressError::UnsupportedScheme),
                "{input}"
            );
        }
        for input in [
            "user:pass@example.com",
            "https://user:pass@example.com/",
            "http://user@example.com/",
        ] {
            assert_eq!(resolve(input), Err(AddressError::Credentials), "{input}");
        }
    }

    #[test]
    fn rejects_malformed_hosts_ports_and_embedded_controls() {
        for input in [
            "https://",
            "https:///example.com",
            "https://?q=a",
            "http://exa mple.com",
            "http://x%40example.com",
            "http://example.com\\@evil.com",
            "localhost:",
            "localhost:65536",
            "example.com:abc",
            "[not-ipv6]:80",
            "[::1]junk",
            "https://example.com/a\nb",
        ] {
            assert_eq!(resolve(input), Err(AddressError::InvalidAddress), "{input}");
        }
    }

    #[test]
    fn caps_the_final_encoded_url() {
        assert_eq!(resolve(&"x".repeat(8193)), Err(AddressError::TooLong));
        assert_eq!(resolve(&"한".repeat(1000)), Err(AddressError::TooLong));
    }
}
