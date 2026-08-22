//! Publisher configuration: `publisher.toml` + environment variable overrides.
//!
//! # File lookup order
//! 1. Path supplied via `--config <path>` CLI flag (pass as `Some(path)` to [`load`])
//! 2. `publisher.toml` next to the running executable (`std::env::current_exe()`)
//! 3. `publisher.toml` in the current working directory
//!
//! # Environment variable overrides
//! Any field can be overridden by the matching env var (prefix `PUBLISHER_AUTH_`
//! or `PUBLISHER_`). Env vars take priority over the file.
//!
//! # Validation
//! `auth.tenant_id`, `auth.client_id`, and `auth.client_secret` are required.
//! A missing or empty value after env override causes [`load`] to return
//! [`ConfigError::Validation`] with a human-readable message.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── Public types ──────────────────────────────────────────────────────────────

/// Fully resolved and validated publisher configuration.
#[derive(Debug, Clone)]
pub struct PublisherConfig {
    pub auth:      AuthConfig,
    pub publisher: PublisherSection,
}

/// Azure AD client-credentials authentication parameters.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub tenant_id:     String,
    pub client_id:     String,
    pub client_secret: String,
    pub scope:         String,
    /// Optional Windows Certificate Store thumbprint (v1: documented, not implemented).
    pub cert_thumbprint: Option<String>,
}

/// Publisher-specific operational settings.
#[derive(Debug, Clone)]
pub struct PublisherSection {
    /// Base URL for the Race Control API (no trailing slash).
    pub rc_api_url:        String,
    /// Interval between batch POST calls, in milliseconds.
    pub batch_interval_ms: u64,
    /// Interval between PUBLISHER_HEARTBEAT events, in milliseconds. `0` disables.
    pub heartbeat_interval_ms: u64,
    /// Interval between DRIVER_MATERIAL events, in milliseconds. `0` disables.
    pub driver_material_interval_ms: u64,
}

impl Default for PublisherSection {
    fn default() -> Self {
        Self {
            rc_api_url:        "https://simracecenter.com".to_owned(),
            batch_interval_ms: 500,
            heartbeat_interval_ms: 15_000,
            driver_material_interval_ms: 25_000,
        }
    }
}

/// Errors produced by [`load`].
#[derive(Debug)]
pub enum ConfigError {
    /// Config file could not be read.
    Io(std::io::Error),
    /// Config file could not be parsed as TOML.
    Toml(toml::de::Error),
    /// A required field is missing or empty.
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e)         => write!(f, "config file read error: {e}"),
            ConfigError::Toml(e)       => write!(f, "config file parse error: {e}"),
            ConfigError::Validation(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(e)   => Some(e),
            ConfigError::Toml(e) => Some(e),
            _                    => None,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load, merge with env vars, and validate the publisher configuration.
///
/// `config_path` should be `Some(path)` when the caller supplies `--config`.
/// Pass `None` to use the default file-lookup order.
pub fn load(config_path: Option<&Path>) -> Result<PublisherConfig, ConfigError> {
    let toml_str = read_config_file(config_path)?;
    let mut raw: RawConfig = toml::from_str(&toml_str).map_err(ConfigError::Toml)?;
    apply_env_overrides(&mut raw);
    build_and_validate(raw)
}

// ── TOML raw types (allow missing required fields — env vars may supply them) ─

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    auth:      RawAuth,
    publisher: RawPublisher,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawAuth {
    tenant_id:       Option<String>,
    client_id:       Option<String>,
    client_secret:   Option<String>,
    scope:           Option<String>,
    cert_thumbprint: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPublisher {
    rc_api_url:            Option<String>,
    batch_interval_ms:     Option<u64>,
    heartbeat_interval_ms: Option<u64>,
    driver_material_interval_ms: Option<u64>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn read_config_file(config_path: Option<&Path>) -> Result<String, ConfigError> {
    // 1. Explicit path
    if let Some(path) = config_path {
        return fs::read_to_string(path).map_err(ConfigError::Io);
    }

    // 2. Next to the executable
    if let Ok(exe) = env::current_exe() {
        let candidate = exe.parent().unwrap_or(Path::new(".")).join("publisher.toml");
        if candidate.exists() {
            return fs::read_to_string(candidate).map_err(ConfigError::Io);
        }
    }

    // 3. Current working directory — return empty TOML if absent (env vars may be sufficient)
    let cwd_candidate = PathBuf::from("publisher.toml");
    if cwd_candidate.exists() {
        return fs::read_to_string(cwd_candidate).map_err(ConfigError::Io);
    }

    // No file found — return empty TOML; validation may fail if env vars are absent too
    Ok(String::new())
}

fn apply_env_overrides(raw: &mut RawConfig) {
    macro_rules! override_from_env {
        ($field:expr, $env_var:literal) => {
            if let Ok(v) = env::var($env_var) {
                if !v.is_empty() {
                    $field = Some(v);
                }
            }
        };
    }

    override_from_env!(raw.auth.tenant_id,       "PUBLISHER_AUTH_TENANT_ID");
    override_from_env!(raw.auth.client_id,        "PUBLISHER_AUTH_CLIENT_ID");
    override_from_env!(raw.auth.client_secret,    "PUBLISHER_AUTH_CLIENT_SECRET");
    override_from_env!(raw.auth.scope,            "PUBLISHER_AUTH_SCOPE");
    override_from_env!(raw.publisher.rc_api_url,  "PUBLISHER_RC_API_URL");

    if let Ok(v) = env::var("PUBLISHER_BATCH_INTERVAL_MS") {
        if let Ok(n) = v.parse::<u64>() {
            raw.publisher.batch_interval_ms = Some(n);
        }
    }

    if let Ok(v) = env::var("PUBLISHER_HEARTBEAT_INTERVAL_MS") {
        if let Ok(n) = v.parse::<u64>() {
            raw.publisher.heartbeat_interval_ms = Some(n);
        }
    }

    if let Ok(v) = env::var("PUBLISHER_DRIVER_MATERIAL_INTERVAL_MS") {
        if let Ok(n) = v.parse::<u64>() {
            raw.publisher.driver_material_interval_ms = Some(n);
        }
    }
}

fn build_and_validate(raw: RawConfig) -> Result<PublisherConfig, ConfigError> {
    let require = |field: Option<String>, name: &str| -> Result<String, ConfigError> {
        match field.filter(|s| !s.is_empty()) {
            Some(v) => Ok(v),
            None => Err(ConfigError::Validation(format!(
                "[publisher] ERROR: auth.{name} is required. \
                 Set it in publisher.toml or {}.\n\
                 See: https://simracecenter.com/docs/rig-setup for Azure AD provisioning steps.",
                env_var_name(name),
            ))),
        }
    };

    let defaults = PublisherSection::default();

    Ok(PublisherConfig {
        auth: AuthConfig {
            tenant_id:     require(raw.auth.tenant_id,     "tenant_id")?,
            client_id:     require(raw.auth.client_id,     "client_id")?,
            client_secret: require(raw.auth.client_secret, "client_secret")?,
            scope:         raw.auth.scope.filter(|s| !s.is_empty())
                               .unwrap_or_else(|| "api://racecontrol-api-a780e279-1cb6-4ed0-9ef6-49029aa50a42/.default".to_owned()),
            cert_thumbprint: raw.auth.cert_thumbprint.filter(|s| !s.is_empty()),
        },
        publisher: PublisherSection {
            rc_api_url: raw.publisher.rc_api_url
                .filter(|s| !s.is_empty())
                .unwrap_or(defaults.rc_api_url),
            batch_interval_ms: raw.publisher.batch_interval_ms
                .unwrap_or(defaults.batch_interval_ms),
            heartbeat_interval_ms: raw.publisher.heartbeat_interval_ms
                .unwrap_or(defaults.heartbeat_interval_ms),
            driver_material_interval_ms: raw.publisher.driver_material_interval_ms
                .unwrap_or(defaults.driver_material_interval_ms),
        },
    })
}

fn env_var_name(field: &str) -> String {
    let key = field.to_uppercase();
    format!("PUBLISHER_AUTH_{key}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> PublisherConfig {
        let raw: RawConfig = toml::from_str(toml).expect("valid toml");
        // No env vars in unit tests — isolate file parsing only
        build_and_validate(raw).expect("valid config")
    }

    #[test]
    fn parse_full_config() {
        let cfg = parse(r#"
[auth]
tenant_id     = "tenant-123"
client_id     = "client-456"
client_secret = "secret-789"
scope         = "api://rc/.default"

[publisher]
rc_api_url            = "https://api.example.com"
batch_interval_ms     = 250
heartbeat_interval_ms = 5000
"#);
        assert_eq!(cfg.auth.tenant_id,     "tenant-123");
        assert_eq!(cfg.auth.client_id,     "client-456");
        assert_eq!(cfg.auth.client_secret, "secret-789");
        assert_eq!(cfg.auth.scope,         "api://rc/.default");
        assert_eq!(cfg.publisher.rc_api_url,        "https://api.example.com");
        assert_eq!(cfg.publisher.batch_interval_ms, 250);
        assert_eq!(cfg.publisher.heartbeat_interval_ms, 5000);
    }

    #[test]
    fn defaults_applied_when_publisher_section_absent() {
        let cfg = parse(r#"
[auth]
tenant_id     = "t"
client_id     = "c"
client_secret = "s"
"#);
        assert_eq!(cfg.auth.scope,                   "api://racecontrol-api-a780e279-1cb6-4ed0-9ef6-49029aa50a42/.default");
        assert_eq!(cfg.publisher.rc_api_url,         "https://simracecenter.com");
        assert_eq!(cfg.publisher.batch_interval_ms,  500);
        assert_eq!(cfg.publisher.heartbeat_interval_ms, 15_000);
    }

    #[test]
    fn heartbeat_interval_zero_accepted() {
        let cfg = parse(r#"
[auth]
tenant_id     = "t"
client_id     = "c"
client_secret = "s"

[publisher]
heartbeat_interval_ms = 0
"#);
        assert_eq!(cfg.publisher.heartbeat_interval_ms, 0);
    }

    #[test]
    fn missing_client_id_returns_validation_error() {
        let mut raw = RawConfig::default();
        raw.auth.tenant_id     = Some("t".to_owned());
        raw.auth.client_secret = Some("s".to_owned());
        // client_id intentionally absent
        let err = build_and_validate(raw).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("client_id"), "error message should name the missing field");
        assert!(msg.contains("PUBLISHER_AUTH_CLIENT_ID"), "error message should name the env var");
    }

    #[test]
    fn env_override_applied() {
        let mut raw = RawConfig {
            auth: RawAuth {
                tenant_id:     Some("t".to_owned()),
                client_id:     Some("c".to_owned()),
                client_secret: Some("s".to_owned()),
                scope:         None,
                cert_thumbprint: None,
            },
            publisher: RawPublisher {
                rc_api_url:            Some("https://original.com".to_owned()),
                batch_interval_ms:     Some(500),
                heartbeat_interval_ms: Some(15_000),
                driver_material_interval_ms: Some(25_000),
            },
        };

        // Simulate env var override by directly modifying (avoids polluting test env)
        raw.publisher.rc_api_url = Some("https://override.com".to_owned());
        raw.publisher.batch_interval_ms = Some(100);

        let cfg = build_and_validate(raw).unwrap();
        assert_eq!(cfg.publisher.rc_api_url,        "https://override.com");
        assert_eq!(cfg.publisher.batch_interval_ms, 100);
    }
}
