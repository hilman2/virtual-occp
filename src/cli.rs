use anyhow::{bail, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "virtual-occp",
    version,
    about = "Virtual OCPP charging station simulator (1.6-J & 2.0.1)"
)]
pub struct Args {
    /// Station definition: id:port:version:csms-url  (version: 1.6 or 2.0.1). Repeatable.
    #[arg(long = "station", num_args = 1..)]
    pub station: Vec<String>,

    /// Enable the Station Manager on this port (separate Web UI for spawning/controlling stations).
    #[arg(long)]
    pub manager_port: Option<u16>,

    /// Directory for persistent JSON data
    #[arg(long, default_value = "data")]
    pub data_dir: String,
}

impl Args {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.station.is_empty() && self.manager_port.is_none() {
            anyhow::bail!(
                "Provide at least --station or --manager-port. Examples:\n  virtual-occp --manager-port 8000\n  virtual-occp --station cp1:8080:1.6:ws://localhost:9000/ocpp"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct StationConfig {
    pub id: String,
    pub http_port: u16,
    pub version: OcppVersion,
    pub csms_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Extracts user:pass from a URL (wss://user:pass@host/path) and returns
/// (url_without_credentials, username_opt, password_opt).
pub fn split_credentials(raw: &str) -> (String, Option<String>, Option<String>) {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            let user = if u.username().is_empty() {
                None
            } else {
                Some(percent_decode(u.username()))
            };
            let pass = u.password().map(percent_decode);
            let _ = u.set_username("");
            let _ = u.set_password(None);
            (u.to_string(), user, pass)
        }
        Err(_) => (raw.to_string(), None, None),
    }
}

fn percent_decode(s: &str) -> String {
    // url::Url returns username/password URL-encoded — decode them here.
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OcppVersion {
    #[serde(rename = "1.6")]
    V16,
    #[serde(rename = "2.0.1")]
    V201,
}

impl OcppVersion {
    pub fn subprotocol(&self) -> &'static str {
        match self {
            OcppVersion::V16 => "ocpp1.6",
            OcppVersion::V201 => "ocpp2.0.1",
        }
    }
}

pub fn parse_stations(raw: &[String]) -> Result<Vec<StationConfig>> {
    let mut out = Vec::new();
    for s in raw {
        out.push(parse_one(s)?);
    }
    Ok(out)
}

fn parse_one(s: &str) -> Result<StationConfig> {
    // Format: id:port:version:csms-url
    // csms-url contains ':' → splitn(4)
    let parts: Vec<&str> = s.splitn(4, ':').collect();
    if parts.len() != 4 {
        bail!("Station must be in the format id:port:version:csms-url, got: {s}");
    }
    let id = parts[0].to_string();
    if id.is_empty() {
        bail!("Station id must not be empty");
    }
    let http_port: u16 = parts[1]
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid port '{}': {e}", parts[1]))?;
    let version = match parts[2] {
        "1.6" => OcppVersion::V16,
        "2.0.1" => OcppVersion::V201,
        v => bail!("Version must be '1.6' or '2.0.1', got: {v}"),
    };
    let raw_url = parts[3].to_string();
    if !(raw_url.starts_with("ws://") || raw_url.starts_with("wss://")) {
        bail!("CSMS URL must start with ws:// or wss://: {raw_url}");
    }
    let (csms_url, username, password) = split_credentials(&raw_url);
    Ok(StationConfig {
        id,
        http_port,
        version,
        csms_url,
        username,
        password,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_station_valid_16() {
        let c = parse_one("cp1:8080:1.6:ws://localhost:9000/ocpp").unwrap();
        assert_eq!(c.id, "cp1");
        assert_eq!(c.http_port, 8080);
        assert_eq!(c.version, OcppVersion::V16);
        assert_eq!(c.csms_url, "ws://localhost:9000/ocpp");
        assert!(c.username.is_none());
        assert!(c.password.is_none());
    }

    #[test]
    fn parse_station_valid_201_wss() {
        let c = parse_one("cp2:8081:2.0.1:wss://host.example.com/path").unwrap();
        assert_eq!(c.version, OcppVersion::V201);
        assert!(c.csms_url.starts_with("wss://"));
    }

    #[test]
    fn parse_station_extracts_credentials() {
        let c = parse_one("cp1:8080:1.6:wss://admin:secret@csms.example.com/ocpp").unwrap();
        assert_eq!(c.username.as_deref(), Some("admin"));
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(c.csms_url, "wss://csms.example.com/ocpp");
    }

    #[test]
    fn parse_station_rejects_non_ws_scheme() {
        assert!(parse_one("cp1:8080:1.6:http://csms/ocpp").is_err());
    }

    #[test]
    fn parse_station_rejects_bad_version() {
        assert!(parse_one("cp1:8080:9.9:ws://host").is_err());
    }

    #[test]
    fn parse_station_rejects_bad_port() {
        assert!(parse_one("cp1:foo:1.6:ws://host").is_err());
    }

    #[test]
    fn parse_station_rejects_empty_id() {
        assert!(parse_one(":8080:1.6:ws://host").is_err());
    }

    #[test]
    fn parse_station_rejects_wrong_field_count() {
        assert!(parse_one("cp1:8080:1.6").is_err());
    }

    #[test]
    fn parse_station_preserves_url_port_and_path() {
        // Inner colons in the URL must not be split.
        let c = parse_one("cp1:8080:2.0.1:ws://csms:9001/sub/path").unwrap();
        assert_eq!(c.csms_url, "ws://csms:9001/sub/path");
    }

    #[test]
    fn split_credentials_noop_without_userinfo() {
        let (u, user, pass) = split_credentials("wss://csms.example.com/ocpp");
        assert_eq!(u, "wss://csms.example.com/ocpp");
        assert!(user.is_none() && pass.is_none());
    }

    #[test]
    fn split_credentials_url_encoded() {
        // Pa$$w0rd! → Pa%24%24w0rd%21
        let (u, user, pass) = split_credentials("ws://foo:Pa%24%24w0rd%21@host/p");
        assert_eq!(user.as_deref(), Some("foo"));
        assert_eq!(pass.as_deref(), Some("Pa$$w0rd!"));
        assert_eq!(u, "ws://host/p");
    }

    #[test]
    fn subprotocol_matches_spec() {
        assert_eq!(OcppVersion::V16.subprotocol(), "ocpp1.6");
        assert_eq!(OcppVersion::V201.subprotocol(), "ocpp2.0.1");
    }
}
