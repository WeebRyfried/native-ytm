//! The InnerTube client: YouTube Music's own JSON API, as the web app speaks
//! it (the `WEB_REMIX` client), plus the player request the stream resolver
//! makes as another client.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use parking_lot::RwLock;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

pub const ORIGIN: &str = "https://music.youtube.com";
const API: &str = "https://music.youtube.com/youtubei/v1";
/// Updated from the web app's own page at start; this is the fallback.
const DEFAULT_CLIENT_VERSION: &str = "1.20260707.12.00";
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// The signed-in account, as browser cookies.
#[derive(Clone)]
pub struct Auth {
    pub cookies: Vec<(String, String)>,
    /// Which signed-in Google account in the browser session (0 for the first).
    pub auth_user: u32,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("cookies", &self.cookies.len())
            .finish_non_exhaustive()
    }
}

impl Auth {
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The `Authorization` header Google's web apps send with cookies.
    pub fn sapisid_hash(&self) -> Option<String> {
        let sapisid = self
            .get("SAPISID")
            .or_else(|| self.get("__Secure-3PAPISID"))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut hasher = Sha1::new();
        hasher.update(format!("{now} {sapisid} {ORIGIN}"));
        let digest = hasher.finalize();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        Some(format!("SAPISIDHASH {now}_{hex}"))
    }

    pub fn is_usable(&self) -> bool {
        self.sapisid_hash().is_some()
    }
}

/// Parses whatever the person pasted: a `Cookie:` header value, a whole
/// copied request (headers or a cURL command), or a Netscape cookies.txt.
pub fn parse_cookies(text: &str) -> Vec<(String, String)> {
    let text = text.trim();
    let mut cookies: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, value: &str| {
        let name = name.trim();
        let value = value.trim().trim_matches('"');
        if name.is_empty() || name.contains(' ') {
            return;
        }
        if let Some(slot) = cookies.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value.to_owned();
        } else {
            cookies.push((name.to_owned(), value.to_owned()));
        }
    };
    // Netscape cookies.txt: seven tab-separated fields.
    let netscape: Vec<&str> = text
        .lines()
        .filter(|line| {
            let fields = line.split('\t').count();
            fields == 7 && !line.starts_with('#') || line.starts_with("#HttpOnly_")
        })
        .collect();
    if !netscape.is_empty() {
        for line in netscape {
            let line = line.trim_start_matches("#HttpOnly_");
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() == 7 && fields[0].contains("youtube.com") {
                add(fields[5], fields[6]);
            }
        }
        return cookies;
    }
    // A cookie line inside copied headers or a cURL command.
    let mut header = None;
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches("-H ").trim_matches(|c| c == '\'' || c == '"' || c == '^');
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("cookie:") {
            let start = trimmed.len() - rest.len();
            header = Some(trimmed[start..].trim().to_owned());
            break;
        }
        if let Some(index) = lower.find("-b '").or_else(|| lower.find("--cookie '")) {
            let rest = &trimmed[index..];
            if let Some(open) = rest.find('\'') {
                let inner = &rest[open + 1..];
                if let Some(close) = inner.find('\'') {
                    header = Some(inner[..close].to_owned());
                    break;
                }
            }
        }
    }
    let header = header.unwrap_or_else(|| text.replace(['\n', '\r'], ""));
    for pair in header.split(';') {
        if let Some((name, value)) = pair.split_once('=') {
            add(name, value);
        }
    }
    cookies
}

#[derive(Clone, Debug)]
struct Config {
    client_version: String,
    visitor_data: Option<String>,
    hl: String,
    gl: Option<String>,
}

pub struct Innertube {
    pub http: reqwest::Client,
    auth: RwLock<Option<Auth>>,
    config: RwLock<Config>,
}

impl Innertube {
    pub fn new(http: reqwest::Client, language: &str) -> Self {
        Self {
            http,
            auth: RwLock::new(None),
            config: RwLock::new(Config {
                client_version: DEFAULT_CLIENT_VERSION.to_owned(),
                visitor_data: None,
                hl: language.to_owned(),
                gl: None,
            }),
        }
    }

    pub fn set_auth(&self, auth: Option<Auth>) {
        *self.auth.write() = auth;
    }

    pub fn auth(&self) -> Option<Auth> {
        self.auth.read().clone()
    }

    pub fn signed_in(&self) -> bool {
        self.auth.read().as_ref().is_some_and(Auth::is_usable)
    }

    pub fn set_language(&self, hl: &str) {
        self.config.write().hl = hl.to_owned();
    }

    pub fn set_region(&self, gl: Option<String>) {
        self.config.write().gl = gl;
    }

    pub fn visitor_data(&self) -> Option<String> {
        self.config.read().visitor_data.clone()
    }

    /// Reads the current client version and a visitor id from the web app's
    /// page. Best effort: the defaults work when this fails.
    pub async fn bootstrap(&self) {
        let mut request = self
            .http
            .get(ORIGIN)
            .header("User-Agent", BROWSER_UA)
            // Skips the consent interstitial in regions that show it.
            .header("Cookie", self.cookie_with_consent());
        request = request.timeout(Duration::from_secs(10));
        match request.send().await {
            Ok(response) => match response.text().await {
                Ok(page) => {
                    let mut config = self.config.write();
                    if let Some(version) = find_quoted(&page, "\"INNERTUBE_CLIENT_VERSION\":\"") {
                        config.client_version = version;
                    }
                    if let Some(visitor) = find_quoted(&page, "\"VISITOR_DATA\":\"") {
                        config.visitor_data = Some(visitor);
                    }
                    if config.gl.is_none()
                        && let Some(gl) = find_quoted(&page, "\"GL\":\"")
                    {
                        config.gl = Some(gl);
                    }
                    log::info!(
                        "InnerTube client {}, region {:?}",
                        config.client_version,
                        config.gl
                    );
                }
                Err(error) => log::warn!("bootstrap page: {error}"),
            },
            Err(error) => log::warn!("bootstrap: {error}"),
        }
        if self.config.read().visitor_data.is_none() {
            match self.fetch_visitor_data().await {
                Ok(visitor) => self.config.write().visitor_data = Some(visitor),
                Err(error) => log::warn!("visitor data: {error}"),
            }
        }
    }

    fn cookie_with_consent(&self) -> String {
        match &*self.auth.read() {
            Some(auth) => {
                let mut header = auth.cookie_header();
                if !auth.cookies.iter().any(|(k, _)| k == "SOCS") {
                    header.push_str("; SOCS=CAI");
                }
                header
            }
            None => "SOCS=CAI".to_owned(),
        }
    }

    /// A visitor id from the service worker's data, the lightest page that
    /// carries one.
    pub async fn fetch_visitor_data(&self) -> Result<String> {
        let text = self
            .http
            .get("https://www.youtube.com/sw.js_data")
            .header("User-Agent", BROWSER_UA)
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .text()
            .await?;
        let json: Value = serde_json::from_str(text.trim_start_matches(")]}'").trim())?;
        json.pointer("/0/2/0/0/13")
            .and_then(Value::as_str)
            .map(|s| percent_decode(s))
            .ok_or_else(|| anyhow!("no visitor data in sw.js_data"))
    }

    pub fn context(&self) -> Value {
        let config = self.config.read();
        let mut client = json!({
            "clientName": "WEB_REMIX",
            "clientVersion": config.client_version,
            "hl": config.hl,
            "platform": "DESKTOP",
            "userAgent": BROWSER_UA,
            "originalUrl": ORIGIN,
        });
        if let Some(gl) = &config.gl {
            client["gl"] = json!(gl);
        }
        if let Some(visitor) = &config.visitor_data {
            client["visitorData"] = json!(visitor);
        }
        json!({ "client": client, "user": {} })
    }

    /// POSTs `body` (with the context added) to a WEB_REMIX endpoint.
    pub async fn post(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        body["context"] = self.context();
        self.post_raw(endpoint, "", body).await
    }

    /// The same, with extra query parameters (continuation tokens).
    pub async fn post_query(&self, endpoint: &str, query: &str, mut body: Value) -> Result<Value> {
        body["context"] = self.context();
        self.post_raw(endpoint, query, body).await
    }

    async fn post_raw(&self, endpoint: &str, query: &str, body: Value) -> Result<Value> {
        let url = format!("{API}/{endpoint}?prettyPrint=false{query}");
        let (version, visitor) = {
            let config = self.config.read();
            (config.client_version.clone(), config.visitor_data.clone())
        };
        let mut request = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(20))
            .header("User-Agent", BROWSER_UA)
            .header("Origin", ORIGIN)
            .header("X-Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"))
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", version)
            .header("Cookie", self.cookie_with_consent())
            .json(&body);
        if let Some(visitor) = visitor {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        if let Some(auth) = self.auth.read().clone()
            && let Some(hash) = auth.sapisid_hash()
        {
            request = request
                .header("Authorization", hash)
                .header("X-Goog-AuthUser", auth.auth_user.to_string());
        }
        let response = request.send().await.with_context(|| format!("{endpoint} request"))?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| text.chars().take(200).collect());
            bail!("{endpoint}: HTTP {status}: {message}");
        }
        let value: Value =
            serde_json::from_str(&text).with_context(|| format!("{endpoint}: not JSON"))?;
        // Continuations only answer within the visitor session that made
        // them, so keep the first visitor id the API hands out.
        if self.config.read().visitor_data.is_none()
            && let Some(visitor) = value
                .pointer("/responseContext/visitorData")
                .and_then(Value::as_str)
        {
            self.config.write().visitor_data = Some(visitor.to_owned());
        }
        Ok(value)
    }

    /// The player request as another client, for stream addresses that
    /// need no JavaScript challenge. The client is configurable since
    /// YouTube changes which clients work.
    pub async fn player_as(&self, client: &PlayerClient, video_id: &str) -> Result<Value> {
        let visitor = match self.visitor_data() {
            Some(visitor) => visitor,
            None => {
                let visitor = self.fetch_visitor_data().await?;
                self.config.write().visitor_data = Some(visitor.clone());
                visitor
            }
        };
        let hl = self.config.read().hl.clone();
        let mut context_client = client.context.clone();
        context_client["clientName"] = json!(client.name);
        context_client["clientVersion"] = json!(client.version);
        context_client["hl"] = json!(hl);
        context_client["visitorData"] = json!(visitor);
        let body = json!({
            "context": { "client": context_client },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        let response = self
            .http
            .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .timeout(Duration::from_secs(15))
            .header("User-Agent", &client.user_agent)
            .header("X-YouTube-Client-Name", client.id.to_string())
            .header("X-YouTube-Client-Version", &client.version)
            .header("X-Goog-Visitor-Id", visitor)
            .header("Origin", "https://www.youtube.com")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            bail!("player: HTTP {status}");
        }
        Ok(response.json().await?)
    }
}

/// An InnerTube client identity for the player request.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PlayerClient {
    pub name: String,
    pub version: String,
    pub id: u32,
    pub user_agent: String,
    /// Extra context fields (device make, OS...).
    pub context: Value,
}

impl PlayerClient {
    /// The clients tried in order. visionOS needs no JavaScript player and
    /// no proof-of-origin token for its audio, as of 2026.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self {
                name: "VISIONOS".into(),
                version: "1.02".into(),
                id: 101,
                user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15".into(),
                context: json!({
                    "deviceMake": "Apple",
                    "deviceModel": "RealityDevice17,1",
                    "osName": "visionOS",
                    "osVersion": "26.5.23O471",
                }),
            },
            Self {
                name: "ANDROID_VR".into(),
                version: "1.65.10".into(),
                id: 28,
                user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip".into(),
                context: json!({
                    "deviceMake": "Oculus",
                    "deviceModel": "Quest 3",
                    "androidSdkVersion": 32,
                    "osName": "Android",
                    "osVersion": "12L",
                }),
            },
        ]
    }
}

fn find_quoted(page: &str, key: &str) -> Option<String> {
    let start = page.find(key)? + key.len();
    let end = page[start..].find('"')? + start;
    Some(page[start..end].replace("\\u003d", "=").replace("%3D", "="))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_header_value() {
        let cookies = parse_cookies("SID=abc; SAPISID=xyz/1; PREF=f6=40000000&tz=UTC");
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies[1], ("SAPISID".into(), "xyz/1".into()));
        assert_eq!(cookies[2].1, "f6=40000000&tz=UTC");
    }

    #[test]
    fn copied_request_headers() {
        let text = "accept: */*\ncookie: SAPISID=one; HSID=two\nuser-agent: x";
        let cookies = parse_cookies(text);
        assert_eq!(cookies, vec![("SAPISID".into(), "one".into()), ("HSID".into(), "two".into())]);
    }

    #[test]
    fn curl_command() {
        let text = "curl 'https://music.youtube.com/youtubei/v1/browse' \\\n  -H 'accept: */*' \\\n  -b 'SAPISID=one; SID=two' \\\n";
        let cookies = parse_cookies(text);
        assert_eq!(cookies.len(), 2);
    }

    #[test]
    fn netscape_file() {
        let text = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tone\n.google.com\tTRUE\t/\tTRUE\t0\tNID\tx\n";
        assert_eq!(parse_cookies(text), vec![("SAPISID".into(), "one".into())]);
    }

    #[test]
    fn percent() {
        assert_eq!(percent_decode("ab%3D%3D"), "ab==");
    }
}
