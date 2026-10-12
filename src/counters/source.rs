// SPDX-License-Identifier: GPL-3.0-or-later
use std::{str::FromStr, time::Duration};

use reqwest::{Client, Url, header};
use scraper::{Html, Selector};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Stats {
    pub online: u64,
    pub registered: Option<u64>,
}

#[derive(Clone, Copy)]
pub enum Format {
    Json,
    Skhid,
}

impl FromStr for Format {
    type Err = &'static str;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "json" => Ok(Self::Json),
            "skhid" => Ok(Self::Skhid),
            _ => Err("SRP_SOURCE_FORMAT must be json or skhid"),
        }
    }
}

impl Format {
    fn decode(self, body: &[u8]) -> Result<Stats, &'static str> {
        match self {
            Self::Json => serde_json::from_slice(body)
                .map_err(|_| "online response must contain a nonnegative integer online count"),
            Self::Skhid => decode_page(body),
        }
    }

    fn max_bytes(self) -> usize {
        match self {
            Self::Json => 4096,
            Self::Skhid => 256 * 1024,
        }
    }
}

fn decode_page(body: &[u8]) -> Result<Stats, &'static str> {
    let html = std::str::from_utf8(body).map_err(|_| "status page must be UTF-8")?;
    let document = Html::parse_document(html);
    let entries = Selector::parse(".stats > div").unwrap();
    let label = Selector::parse("span").unwrap();
    let value = Selector::parse("b").unwrap();
    let mut online = None;
    let mut registered = None;
    for entry in document.select(&entries) {
        let Some(label) = entry.select(&label).next() else {
            continue;
        };
        let label = label.text().collect::<String>();
        let normalized = label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let slot = match normalized.as_str() {
            "players online" => &mut online,
            "registered" => &mut registered,
            _ => continue,
        };
        if slot.is_some() {
            return Err("status page contains duplicate counters");
        }
        let text = entry
            .select(&value)
            .next()
            .ok_or("status page counter has no value")?
            .text()
            .collect::<String>();
        let text = text.trim();
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("status page counter must be a nonnegative integer");
        }
        *slot = Some(
            text.parse::<u64>()
                .map_err(|_| "status page counter is too large")?,
        );
    }
    Ok(Stats {
        online: online.ok_or("status page has no players online counter; use ?lang=en")?,
        registered: Some(registered.ok_or("status page has no registered counter; use ?lang=en")?),
    })
}

pub struct Endpoint(Url);
impl FromStr for Endpoint {
    type Err = &'static str;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(raw).map_err(|_| "SRP_ONLINE_URL must be a valid HTTP(S) URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("SRP_ONLINE_URL must be HTTP(S), without credentials or a fragment");
        }
        if url.scheme() == "http"
            && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        {
            return Err("SRP_ONLINE_URL must use HTTPS outside localhost");
        }
        Ok(Self(url))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollInterval(Duration);
impl FromStr for PollInterval {
    type Err = &'static str;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let seconds = raw.parse::<u64>().ok().filter(|s| (15..=86400).contains(s));
        seconds
            .map(|s| Self(Duration::from_secs(s)))
            .ok_or("expected an integer from 15 to 86400")
    }
}
impl PollInterval {
    pub fn duration(self) -> Duration {
        self.0
    }
}

pub struct Source {
    client: Client,
    url: Endpoint,
    authorization: Option<header::HeaderValue>,
    format: Format,
}

impl Source {
    pub fn new(url: Endpoint, token: Option<&str>, format: Format) -> Result<Self, &'static str> {
        let authorization = token
            .filter(|t| !t.is_empty())
            .map(|token| -> Result<_, &'static str> {
                let mut value = header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| "SRP_ACCOUNT_TOKEN is not a valid HTTP credential")?;
                value.set_sensitive(true);
                Ok(value)
            })
            .transpose()?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "could not initialize the online HTTP client")?;
        Ok(Self {
            client,
            url,
            authorization,
            format,
        })
    }

    pub async fn fetch(&self) -> Result<Stats, &'static str> {
        let mut request = self.client.get(self.url.0.clone()).header(
            header::ACCEPT,
            match self.format {
                Format::Json => "application/json",
                Format::Skhid => "text/html",
            },
        );
        if let Some(authorization) = &self.authorization {
            request = request.header(header::AUTHORIZATION, authorization.clone());
        }
        let mut response = request.send().await.map_err(|_| "HTTP request failed")?;
        if !response.status().is_success() {
            return Err("HTTP source returned an unsuccessful status");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "HTTP response interrupted")?
        {
            if body.len() + chunk.len() > self.format.max_bytes() {
                return Err("online response exceeds size limit");
            }
            body.extend_from_slice(&chunk);
        }
        self.format.decode(&body)
    }
}

pub fn activity_name(count: Option<u64>) -> String {
    match count {
        None => "Online count unavailable".into(),
        Some(1) => "1 player online".into(),
        Some(count) => format!("{count} players online"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn decode_count(body: &[u8]) -> Result<u64, &'static str> {
        Format::Json.decode(body).map(|stats| stats.online)
    }

    #[test]
    fn reads_labelled_website_stats_ignoring_other_numbers() {
        let page = br#"<title>Survarium 0.26g</title><b>999</b>
            <div class="card stats">
              <div><b>30</b><span> registered </span></div>
              <div><b>0</b><span>players\n online</span></div>
            </div><p>20 players: start at once</p>"#;
        let page = String::from_utf8(page.to_vec())
            .unwrap()
            .replace("\\n", "\n");
        assert_eq!(
            Format::Skhid.decode(page.as_bytes()),
            Ok(Stats {
                online: 0,
                registered: Some(30)
            })
        );
        assert_eq!(
            Format::Skhid.decode(include_bytes!("../../tests/fixtures/skhid-stats.html")),
            Ok(Stats {
                online: 10,
                registered: Some(30)
            })
        );
        assert_eq!(
            Format::Json.decode(br#"{"online":4,"registered":12}"#),
            Ok(Stats {
                online: 4,
                registered: Some(12)
            })
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_invalid_website_counters() {
        let fixture = include_str!("../../tests/fixtures/skhid-stats.html");
        for page in [
            "<html>maintenance</html>".to_owned(),
            fixture.replace("players online", "unknown"),
            fixture.replace("registered", "unknown"),
            fixture.replace("<b>10</b>", "<b>-1</b>"),
            fixture.replace("<b>10</b>", "<b>1.5</b>"),
            fixture.replace("<b>10</b>", "<b>many</b>"),
            fixture.replace("<b>10</b>", "<b>18446744073709551616</b>"),
            fixture.replace("<b>10</b>", ""),
            format!("{fixture}{fixture}"),
        ] {
            assert!(Format::Skhid.decode(page.as_bytes()).is_err(), "{page}");
        }
        assert!(Format::Skhid.decode(&[0xff]).is_err());
        assert!("unknown".parse::<Format>().is_err());
    }

    #[test]
    fn validates_configuration_and_count_contract() {
        for url in [
            "http://localhost/online",
            "http://127.0.0.1/online",
            "http://[::1]/online",
            "https://example.com/online",
        ] {
            assert!(url.parse::<Endpoint>().is_ok(), "{url}");
        }
        for url in [
            "file:///tmp/count",
            "http://example.com/online",
            "https://user:secret@example.com",
            "https://example.com/#fragment",
        ] {
            assert!(url.parse::<Endpoint>().is_err(), "{url}");
        }
        for interval in ["0", "14", "NaN", "inf", "-1", "86401"] {
            assert!(interval.parse::<PollInterval>().is_err());
        }
        assert_eq!(
            "30".parse::<PollInterval>().unwrap().duration(),
            Duration::from_secs(30)
        );
        assert_eq!(decode_count(br#"{"online":0}"#), Ok(0));
        assert_eq!(decode_count(br#"{"online":12}"#), Ok(12));
        for body in [
            "null",
            "[]",
            "{}",
            "{\"online\":true}",
            "{\"online\":-1}",
            "{\"online\":1.5}",
            "{\"online\":\"2\"}",
            "not json",
        ] {
            assert!(decode_count(body.as_bytes()).is_err(), "{body}");
        }
    }

    async fn request(
        status: &str,
        body: &str,
        extra_headers: &str,
    ) -> (Result<u64, &'static str>, String) {
        let (result, headers) = request_format(status, body, extra_headers, Format::Json).await;
        (result.map(|stats| stats.online), headers)
    }

    async fn request_format(
        status: &str,
        body: &str,
        extra_headers: &str,
        format: Format,
    ) -> (Result<Stats, &'static str>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/online", listener.local_addr().unwrap())
            .parse::<Endpoint>()
            .unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let _ = stream.write_all(response.as_bytes());
            String::from_utf8(request).unwrap()
        });
        let result = Source::new(url, Some("test-token"), format)
            .unwrap()
            .fetch()
            .await;
        (result, server.join().unwrap())
    }

    #[tokio::test]
    async fn http_count_auth_errors_redirects_and_oversized_body() {
        let (result, headers) = request("200 OK", r#"{"online":3}"#, "").await;
        assert_eq!(result, Ok(3));
        assert!(
            headers
                .to_lowercase()
                .contains("authorization: bearer test-token\r\n")
        );
        assert!(request("401 Unauthorized", "{}", "").await.0.is_err());
        assert!(request("503 Unavailable", "{}", "").await.0.is_err());
        assert!(
            request(
                "302 Found",
                r#"{"online":3}"#,
                "Location: http://127.0.0.1:1/\r\n"
            )
            .await
            .0
            .is_err()
        );
        assert!(request("200 OK", &" ".repeat(4097), "").await.0.is_err());
        assert!(request("200 OK", "not json", "").await.0.is_err());
    }

    #[tokio::test]
    async fn http_website_stats_and_size_limit() {
        let page = format!(
            "{}{}",
            " ".repeat(8000),
            include_str!("../../tests/fixtures/skhid-stats.html")
        );
        let (result, headers) = request_format(
            "200 OK",
            &page,
            "Content-Type: text/html\r\n",
            Format::Skhid,
        )
        .await;
        assert_eq!(
            result,
            Ok(Stats {
                online: 10,
                registered: Some(30)
            })
        );
        assert!(headers.contains("accept: text/html"));
        assert!(
            request_format("200 OK", &" ".repeat(256 * 1024 + 1), "", Format::Skhid)
                .await
                .0
                .is_err()
        );
        assert!(
            request_format("503 Unavailable", &page, "", Format::Skhid)
                .await
                .0
                .is_err()
        );
    }
}
