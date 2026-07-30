use std::{env, fmt};

use anyhow::{anyhow, Result};
use reqwest::{ClientBuilder, NoProxy, Proxy, Url};

#[derive(Clone, Default, Eq, PartialEq)]
struct ProxySettings {
    http: Option<String>,
    https: Option<String>,
    no_proxy: Option<String>,
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxySettings")
            .field("http", &self.http.as_ref().map(|_| "[redacted]"))
            .field("https", &self.https.as_ref().map(|_| "[redacted]"))
            .field("no_proxy", &self.no_proxy)
            .finish()
    }
}

impl ProxySettings {
    fn from_env() -> Self {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let all = first_non_empty(&mut lookup, &["ALL_PROXY", "all_proxy"]);
        let http =
            first_non_empty(&mut lookup, &["HTTP_PROXY", "http_proxy"]).or_else(|| all.clone());
        let https = first_non_empty(&mut lookup, &["HTTPS_PROXY", "https_proxy"]).or(all);
        let no_proxy = first_set(&mut lookup, &["NO_PROXY", "no_proxy"]);
        Self {
            http,
            https,
            no_proxy,
        }
    }

    fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none()
    }
}

pub fn configure_from_env(builder: ClientBuilder) -> Result<ClientBuilder> {
    configure(builder, &ProxySettings::from_env())
}

pub fn configured_source_for_url(endpoint: &str) -> Option<&'static str> {
    let url = Url::parse(endpoint).ok()?;
    proxy_source_from_lookup(url.scheme(), |name| env::var(name).ok())
}

fn proxy_source_from_lookup(
    scheme: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Option<&'static str> {
    let protocol_names = match scheme {
        "http" => &["HTTP_PROXY", "http_proxy"][..],
        "https" => &["HTTPS_PROXY", "https_proxy"][..],
        _ => return None,
    };
    first_non_empty_name(&mut lookup, protocol_names)
        .or_else(|| first_non_empty_name(&mut lookup, &["ALL_PROXY", "all_proxy"]))
}

fn configure(mut builder: ClientBuilder, settings: &ProxySettings) -> Result<ClientBuilder> {
    if settings.is_empty() {
        return Ok(builder);
    }

    builder = builder.no_proxy();
    let no_proxy = settings.no_proxy.as_deref().and_then(NoProxy::from_string);

    if let Some(proxy_url) = settings.http.as_deref() {
        let proxy = Proxy::http(proxy_url)
            .map_err(|_| anyhow!("HTTP proxy environment variable contains an invalid URL"))?
            .no_proxy(no_proxy.clone());
        builder = builder.proxy(proxy);
    }
    if let Some(proxy_url) = settings.https.as_deref() {
        let proxy = Proxy::https(proxy_url)
            .map_err(|_| anyhow!("HTTPS proxy environment variable contains an invalid URL"))?
            .no_proxy(no_proxy);
        builder = builder.proxy(proxy);
    }

    Ok(builder)
}

fn first_non_empty(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    names: &[&str],
) -> Option<String> {
    names.iter().find_map(|name| {
        lookup(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn first_set(lookup: &mut impl FnMut(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| lookup(name))
}

fn first_non_empty_name(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    names: &[&'static str],
) -> Option<&'static str> {
    names
        .iter()
        .copied()
        .find(|name| lookup(name).is_some_and(|value| !value.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn settings(values: &[(&str, &str)]) -> ProxySettings {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        ProxySettings::from_lookup(|name| values.get(name).cloned())
    }

    #[test]
    fn reads_protocol_proxies_and_no_proxy() {
        let parsed = settings(&[
            ("HTTP_PROXY", "http://127.0.0.1:8118"),
            ("HTTPS_PROXY", "http://127.0.0.1:8118"),
            ("NO_PROXY", "localhost,127.0.0.1,::1"),
        ]);
        assert_eq!(
            parsed,
            ProxySettings {
                http: Some("http://127.0.0.1:8118".into()),
                https: Some("http://127.0.0.1:8118".into()),
                no_proxy: Some("localhost,127.0.0.1,::1".into()),
            }
        );
    }

    #[test]
    fn supports_lowercase_and_all_proxy_fallback() {
        let parsed = settings(&[
            ("all_proxy", "http://proxy:9000"),
            ("http_proxy", "http://http-proxy:8000"),
            ("no_proxy", ".internal"),
        ]);
        assert_eq!(parsed.http.as_deref(), Some("http://http-proxy:8000"));
        assert_eq!(parsed.https.as_deref(), Some("http://proxy:9000"));
        assert_eq!(parsed.no_proxy.as_deref(), Some(".internal"));
    }

    #[test]
    fn accepts_socks5_and_socks5h_proxies() {
        for proxy_url in [
            "socks5://127.0.0.1:1080",
            "socks5h://user:password@127.0.0.1:1080",
        ] {
            let settings = ProxySettings {
                http: Some(proxy_url.into()),
                https: Some(proxy_url.into()),
                no_proxy: Some("localhost,127.0.0.1,::1".into()),
            };
            assert!(
                configure(reqwest::Client::builder(), &settings).is_ok(),
                "{proxy_url} should be accepted"
            );
        }
    }

    #[test]
    fn uppercase_proxy_has_priority() {
        let parsed = settings(&[
            ("HTTP_PROXY", "http://upper:8000"),
            ("http_proxy", "http://lower:8000"),
        ]);
        assert_eq!(parsed.http.as_deref(), Some("http://upper:8000"));
    }

    #[test]
    fn reports_the_proxy_source_selected_for_the_url_scheme() {
        let values = HashMap::from([
            ("ALL_PROXY", "socks5h://127.0.0.1:1080"),
            ("HTTPS_PROXY", "http://127.0.0.1:8118"),
        ]);
        let source =
            proxy_source_from_lookup("https", |name| values.get(name).map(ToString::to_string));
        assert_eq!(source, Some("HTTPS_PROXY"));
    }

    #[test]
    fn reports_all_proxy_when_no_protocol_proxy_is_set() {
        let values = HashMap::from([("ALL_PROXY", "socks5h://127.0.0.1:1080")]);
        let source =
            proxy_source_from_lookup("https", |name| values.get(name).map(ToString::to_string));
        assert_eq!(source, Some("ALL_PROXY"));
    }

    #[test]
    fn invalid_proxy_error_does_not_echo_proxy_url() {
        let settings = ProxySettings {
            http: Some("file://user:secret@proxy.invalid".into()),
            https: None,
            no_proxy: None,
        };
        let error = match configure(reqwest::Client::builder(), &settings) {
            Ok(_) => panic!("invalid proxy URL was accepted"),
            Err(error) => error.to_string(),
        };
        assert_eq!(
            error,
            "HTTP proxy environment variable contains an invalid URL"
        );
        assert!(!error.contains("secret"));
    }
}
