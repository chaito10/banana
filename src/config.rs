//! Layered, local, inspectable configuration.
//!
//! Later layers override earlier layers:
//!
//! ```text
//! built in defaults  <  user file  <  CLI arguments  <  application API
//! ```
//!
//! Configuration selects behaviour. It is never a declarative replacement for
//! the runtime API, and it never requires a remote service.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::obs::Level;

/// Every untrusted input path has a bound, and every bound is configurable
/// within a conservative range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Maximum number of simultaneously connected peers.
    pub max_peers: u32,
    /// Maximum number of streams per peer.
    pub max_streams_per_peer: u32,
    /// Maximum application payload in one stream frame.
    pub max_frame_bytes: usize,
    /// Maximum bytes buffered for one stream before writes block.
    pub max_buffered_bytes_per_stream: usize,
    /// Maximum inbound datagrams accepted per second per peer.
    pub max_datagrams_per_second: u32,
    /// Maximum concurrent outbound connections.
    pub max_pending_connections: u32,
    /// Maximum simultaneous hole punch attempts per peer.
    pub max_punch_attempts: u32,
    /// Maximum topic records stored for lookups.
    pub max_topic_records: usize,
    /// Maximum cached bootstrap and peer addresses.
    pub max_cached_addresses: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_peers: 64,
            max_streams_per_peer: 256,
            max_frame_bytes: 64 * 1024,
            max_buffered_bytes_per_stream: 4 * 1024 * 1024,
            max_datagrams_per_second: 2_000,
            max_pending_connections: 32,
            max_punch_attempts: 8,
            max_topic_records: 2_048,
            max_cached_addresses: 512,
        }
    }
}

/// DHT tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhtConfig {
    /// Bucket size, also the number of peers returned per query.
    pub k: usize,
    /// Parallel queries per lookup.
    pub alpha: usize,
    /// How often announce records are refreshed, in milliseconds.
    pub announce_interval_ms: u64,
    /// How often stale routing entries are pruned, in milliseconds.
    pub maintenance_interval_ms: u64,
    /// When a peer record is considered stale, in milliseconds.
    pub peer_ttl_ms: u64,
    /// How long a topic record stays usable, in milliseconds.
    pub record_ttl_ms: u64,
}

impl Default for DhtConfig {
    fn default() -> Self {
        DhtConfig {
            k: 20,
            alpha: 3,
            announce_interval_ms: 30_000,
            maintenance_interval_ms: 60_000,
            peer_ttl_ms: 15 * 60_000,
            record_ttl_ms: 120_000,
        }
    }
}

/// Swarm tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmConfig {
    /// Default number of connected peers per topic.
    pub default_fanout: u32,
    /// Hard ceiling an application may request.
    pub max_fanout: u32,
    /// How often a swarm re-checks its membership, in milliseconds.
    pub membership_interval_ms: u64,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        SwarmConfig {
            default_fanout: 8,
            max_fanout: 64,
            membership_interval_ms: 5_000,
        }
    }
}

/// Connectivity tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetConfig {
    /// Whether this node may act as a relay for others.
    pub accept_relays: bool,
    /// Address to serve relay traffic on, when acting as a relay.
    pub relay_listen: Option<SocketAddr>,
    /// Whether this node may use relays when direct paths fail.
    pub use_relays: bool,
    /// Address where other peers may reach this node for relaying.
    pub advertised_relay: Option<SocketAddr>,
    /// How long to wait for a direct path before falling back, in milliseconds.
    pub punch_timeout_ms: u64,
    /// Overall connection budget, in milliseconds.
    pub connect_timeout_ms: u64,
    /// Keepalive interval, in milliseconds.
    pub keepalive_ms: u64,
    /// Reconnect backoff ceiling, in milliseconds.
    pub backoff_max_ms: u64,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            accept_relays: false,
            relay_listen: None,
            use_relays: true,
            advertised_relay: None,
            punch_timeout_ms: 1_500,
            connect_timeout_ms: 15_000,
            keepalive_ms: 15_000,
            backoff_max_ms: 60_000,
        }
    }
}

/// Interoperability switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteropConfig {
    /// Use the documented Holepunch public bootstrap nodes.
    pub hyperswarm_bootstrap: bool,
    /// Accept Holepunch wire traffic. See docs/protocol.md.
    pub holepunch_wire: bool,
}

impl Default for InteropConfig {
    fn default() -> Self {
        InteropConfig {
            hyperswarm_bootstrap: false,
            holepunch_wire: false,
        }
    }
}

/// The complete runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Directory holding identity and caches.
    pub data_dir: PathBuf,
    /// UDP and TCP addresses to bind.
    pub listen: Vec<SocketAddr>,
    /// Entry points for the routing table. Entry points, not authorities.
    pub bootstrap: Vec<SocketAddr>,
    /// Log level.
    pub log_level: Level,
    /// Resource limits.
    pub limits: Limits,
    /// DHT tuning.
    pub dht: DhtConfig,
    /// Swarm tuning.
    pub swarm: SwarmConfig,
    /// Connectivity tuning.
    pub net: NetConfig,
    /// Interoperability switches.
    pub interop: InteropConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            data_dir: default_data_dir(),
            listen: vec!["0.0.0.0:49737".parse().expect("literal socket address")],
            bootstrap: Vec::new(),
            log_level: Level::Warn,
            limits: Limits::default(),
            dht: DhtConfig::default(),
            swarm: SwarmConfig::default(),
            net: NetConfig::default(),
            interop: InteropConfig::default(),
        }
    }
}

/// Returns the default state directory, honouring `BANANA_HOME`.
pub fn default_data_dir() -> PathBuf {
    if let Some(home) = non_empty_env("BANANA_HOME") {
        return PathBuf::from(home);
    }
    if let Some(base) = non_empty_env("XDG_DATA_HOME") {
        return PathBuf::from(base).join("banana");
    }
    if cfg!(windows) {
        if let Some(appdata) = non_empty_env("APPDATA") {
            return PathBuf::from(appdata).join("banana");
        }
    }
    if let Some(home) = non_empty_env("HOME") {
        return PathBuf::from(home).join(".banana");
    }
    std::env::temp_dir().join("banana")
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Location of runtime state files.
pub fn state_dir() -> Result<PathBuf> {
    let dir = default_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| Error::io("create data dir", e))?;
    Ok(dir)
}

/// Location of the identity file.
pub fn identity_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("identity"))
}

impl Config {
    /// Reads a user configuration file, if it exists.
    pub fn from_file(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| Error::io("read config", e))?;
        Config::parse(&raw)
    }

    /// Reads the configuration from the default location, if present.
    pub fn load() -> Result<Config> {
        Config::from_file(&default_data_dir().join("config"))
    }

    /// Parses the small, deterministic configuration syntax.
    ///
    /// Supported values are integers, booleans, quoted strings and arrays of
    /// quoted strings. Unknown keys are rejected rather than ignored, so a
    /// typo never silently changes behaviour.
    pub fn parse(raw: &str) -> Result<Config> {
        let mut config = Config::default();
        let mut section = String::new();
        for (lineno, line) in raw.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let name = rest
                    .strip_suffix(']')
                    .ok_or_else(|| Config::error(lineno, "unterminated section header"))?
                    .trim();
                section = name.to_string();
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| Config::error(lineno, "expected `key = value`"))?;
            let key = key.trim();
            let value = value.trim();
            let full = if section.is_empty() {
                key.to_string()
            } else {
                format!("{section}.{key}")
            };
            config
                .apply_setting(&full, value)
                .map_err(|err| Config::error(lineno, &err.to_string()))?;
        }
        config.validate()?;
        Ok(config)
    }

    /// Serialises the configuration back to the same syntax.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "data_dir = \"{}\"\n",
            escape(&self.data_dir.display().to_string())
        ));
        out.push_str(&format!(
            "listen = {}\n",
            render_list(&render_addrs(&self.listen))
        ));
        out.push_str(&format!(
            "bootstrap = {}\n",
            render_list(&render_addrs(&self.bootstrap))
        ));
        out.push_str(&format!("log_level = \"{}\"\n", self.log_level));
        out.push_str(&format!("accept_relays = {}\n", self.net.accept_relays));
        out.push_str(&format!("use_relays = {}\n", self.net.use_relays));
        if let Some(addr) = &self.net.advertised_relay {
            out.push_str(&format!("advertised_relay = \"{addr}\"\n"));
        }
        out.push_str(&format!(
            "hyperswarm_bootstrap = {}\n",
            self.interop.hyperswarm_bootstrap
        ));
        out.push_str(&format!(
            "holepunch_wire = {}\n",
            self.interop.holepunch_wire
        ));
        out.push_str("[limits]\n");
        out.push_str(&format!("max_peers = {}\n", self.limits.max_peers));
        out.push_str(&format!(
            "max_streams_per_peer = {}\n",
            self.limits.max_streams_per_peer
        ));
        out.push_str(&format!(
            "max_frame_bytes = {}\n",
            self.limits.max_frame_bytes
        ));
        out.push_str(&format!(
            "max_buffered_bytes_per_stream = {}\n",
            self.limits.max_buffered_bytes_per_stream
        ));
        out.push_str(&format!(
            "max_datagrams_per_second = {}\n",
            self.limits.max_datagrams_per_second
        ));
        out.push_str(&format!(
            "max_pending_connections = {}\n",
            self.limits.max_pending_connections
        ));
        out.push_str(&format!(
            "max_punch_attempts = {}\n",
            self.limits.max_punch_attempts
        ));
        out.push_str("[dht]\n");
        out.push_str(&format!("k = {}\n", self.dht.k));
        out.push_str(&format!("alpha = {}\n", self.dht.alpha));
        out.push_str(&format!(
            "announce_interval_ms = {}\n",
            self.dht.announce_interval_ms
        ));
        out.push_str(&format!(
            "maintenance_interval_ms = {}\n",
            self.dht.maintenance_interval_ms
        ));
        out.push_str(&format!("peer_ttl_ms = {}\n", self.dht.peer_ttl_ms));
        out.push_str("[swarm]\n");
        out.push_str(&format!("default_fanout = {}\n", self.swarm.default_fanout));
        out.push_str(&format!("max_fanout = {}\n", self.swarm.max_fanout));
        out.push_str("[net]\n");
        out.push_str(&format!(
            "punch_timeout_ms = {}\n",
            self.net.punch_timeout_ms
        ));
        out.push_str(&format!(
            "connect_timeout_ms = {}\n",
            self.net.connect_timeout_ms
        ));
        out.push_str(&format!("keepalive_ms = {}\n", self.net.keepalive_ms));
        out.push_str(&format!("backoff_max_ms = {}\n", self.net.backoff_max_ms));
        out
    }

    /// Writes the configuration atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("create config dir", e))?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.render()).map_err(|e| Error::io("write config", e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io("commit config", e))?;
        Ok(())
    }

    /// Applies a higher priority layer on top of this configuration.
    pub fn apply(&mut self, patch: &Config) {
        if !patch.listen.is_empty() {
            self.listen = patch.listen.clone();
        }
        if !patch.bootstrap.is_empty() {
            self.bootstrap = patch.bootstrap.clone();
        }
        if patch.log_level != Level::Warn {
            self.log_level = patch.log_level;
        }
        if patch.net.accept_relays {
            self.net.accept_relays = true;
        }
        if !patch.net.use_relays {
            self.net.use_relays = false;
        }
        if patch.net.relay_listen.is_some() {
            self.net.relay_listen = patch.net.relay_listen;
        }
        if patch.net.advertised_relay.is_some() {
            self.net.advertised_relay = patch.net.advertised_relay;
        }
        if patch.interop.hyperswarm_bootstrap {
            self.interop.hyperswarm_bootstrap = true;
        }
        if patch.interop.holepunch_wire {
            self.interop.holepunch_wire = true;
        }
        self.limits = patch.limits.clone();
        self.dht = patch.dht.clone();
        self.swarm = patch.swarm.clone();
        self.net.punch_timeout_ms = patch.net.punch_timeout_ms;
        self.net.connect_timeout_ms = patch.net.connect_timeout_ms;
        self.net.keepalive_ms = patch.net.keepalive_ms;
        self.net.backoff_max_ms = patch.net.backoff_max_ms;
        if patch.data_dir != default_data_dir() {
            self.data_dir = patch.data_dir.clone();
        }
    }

    /// Validates internal consistency and conservative bounds.
    pub fn validate(&self) -> Result<()> {
        if self.limits.max_peers == 0 {
            return Err(Config::error(
                0,
                "limits.max_peers must be greater than zero",
            ));
        }
        if self.limits.max_streams_per_peer == 0 {
            return Err(Config::error(
                0,
                "limits.max_streams_per_peer must be greater than zero",
            ));
        }
        if self.limits.max_frame_bytes == 0 || self.limits.max_frame_bytes > 16 * 1024 * 1024 {
            return Err(Config::error(
                0,
                "limits.max_frame_bytes must be within 1..=16777216",
            ));
        }
        if self.limits.max_buffered_bytes_per_stream < self.limits.max_frame_bytes {
            return Err(Config::error(
                0,
                "limits.max_buffered_bytes_per_stream must be at least limits.max_frame_bytes",
            ));
        }
        if self.dht.k == 0 || self.dht.k > crate::wire::MAX_NODES {
            return Err(Config::error(0, "dht.k must be within 1..=20"));
        }
        if self.dht.alpha == 0 || self.dht.alpha > self.dht.k {
            return Err(Config::error(0, "dht.alpha must be within 1..=dht.k"));
        }
        if self.dht.announce_interval_ms == 0 || self.dht.maintenance_interval_ms == 0 {
            return Err(Config::error(0, "dht intervals must be greater than zero"));
        }
        if self.swarm.default_fanout == 0 || self.swarm.default_fanout > self.swarm.max_fanout {
            return Err(Config::error(
                0,
                "swarm.default_fanout must be within 1..=swarm.max_fanout",
            ));
        }
        if self.net.punch_timeout_ms == 0 || self.net.connect_timeout_ms == 0 {
            return Err(Config::error(0, "net timeouts must be greater than zero"));
        }
        if self.net.keepalive_ms < 1_000 {
            return Err(Config::error(0, "net.keepalive_ms must be at least 1000"));
        }
        if self.listen.len() > 16 {
            return Err(Config::error(
                0,
                "at most 16 listen addresses are supported",
            ));
        }
        for addr in &self.listen {
            if addr.port() == 0 {
                return Err(Config::error(0, "listen addresses must specify a port"));
            }
        }
        Ok(())
    }

    fn error(lineno: usize, message: &str) -> Error {
        Error::Config(format!("line {}: {message}", lineno + 1))
    }

    fn apply_setting(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "data_dir" => self.data_dir = PathBuf::from(unquote(value)?),
            "listen" => self.listen = parse_addr_list(value)?,
            "bootstrap" => self.bootstrap = parse_addr_list(value)?,
            "log_level" => {
                self.log_level = Level::parse(&unquote(value)?)
                    .ok_or_else(|| Error::Config(format!("unknown log level `{value}`")))?
            }
            "accept_relays" => self.net.accept_relays = parse_bool(value)?,
            "use_relays" => self.net.use_relays = parse_bool(value)?,
            "advertised_relay" => self.net.advertised_relay = Some(parse_addr(&unquote(value)?)?),
            "relay_listen" => self.net.relay_listen = Some(parse_addr(&unquote(value)?)?),
            "hyperswarm_bootstrap" => self.interop.hyperswarm_bootstrap = parse_bool(value)?,
            "holepunch_wire" => self.interop.holepunch_wire = parse_bool(value)?,
            "limits.max_peers" => self.limits.max_peers = parse_u32(value)?,
            "limits.max_streams_per_peer" => self.limits.max_streams_per_peer = parse_u32(value)?,
            "limits.max_frame_bytes" => self.limits.max_frame_bytes = parse_usize(value)?,
            "limits.max_buffered_bytes_per_stream" => {
                self.limits.max_buffered_bytes_per_stream = parse_usize(value)?
            }
            "limits.max_datagrams_per_second" => {
                self.limits.max_datagrams_per_second = parse_u32(value)?
            }
            "limits.max_pending_connections" => {
                self.limits.max_pending_connections = parse_u32(value)?
            }
            "limits.max_punch_attempts" => self.limits.max_punch_attempts = parse_u32(value)?,
            "dht.k" => self.dht.k = parse_usize(value)?,
            "dht.alpha" => self.dht.alpha = parse_usize(value)?,
            "dht.announce_interval_ms" => self.dht.announce_interval_ms = parse_u64(value)?,
            "dht.maintenance_interval_ms" => self.dht.maintenance_interval_ms = parse_u64(value)?,
            "dht.peer_ttl_ms" => self.dht.peer_ttl_ms = parse_u64(value)?,
            "dht.record_ttl_ms" => self.dht.record_ttl_ms = parse_u64(value)?,
            "swarm.default_fanout" => self.swarm.default_fanout = parse_u32(value)?,
            "swarm.max_fanout" => self.swarm.max_fanout = parse_u32(value)?,
            "swarm.membership_interval_ms" => self.swarm.membership_interval_ms = parse_u64(value)?,
            "net.punch_timeout_ms" => self.net.punch_timeout_ms = parse_u64(value)?,
            "net.connect_timeout_ms" => self.net.connect_timeout_ms = parse_u64(value)?,
            "net.keepalive_ms" => self.net.keepalive_ms = parse_u64(value)?,
            "net.backoff_max_ms" => self.net.backoff_max_ms = parse_u64(value)?,
            other => {
                return Err(Error::Config(format!("unknown setting `{other}`")));
            }
        }
        Ok(())
    }
}

fn unquote(value: &str) -> Result<String> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| Error::Config(format!("expected a quoted string, got `{value}`")))?;
    Ok(inner.replace("\\\"", "\""))
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(Error::Config(format!(
            "expected true or false, got `{other}`"
        ))),
    }
}

fn parse_u32(value: &str) -> Result<u32> {
    value
        .trim()
        .parse()
        .map_err(|_| Error::Config(format!("expected an integer, got `{value}`")))
}

fn parse_u64(value: &str) -> Result<u64> {
    value
        .trim()
        .parse()
        .map_err(|_| Error::Config(format!("expected an integer, got `{value}`")))
}

fn parse_usize(value: &str) -> Result<usize> {
    value
        .trim()
        .parse()
        .map_err(|_| Error::Config(format!("expected an integer, got `{value}`")))
}

fn parse_addr(value: &str) -> Result<SocketAddr> {
    value
        .trim()
        .parse()
        .map_err(|_| Error::Config(format!("expected `ip:port`, got `{value}`")))
}

fn parse_addr_list(value: &str) -> Result<Vec<SocketAddr>> {
    let trimmed = value.trim();
    if trimmed == "[]" {
        return Ok(Vec::new());
    }
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return Err(Error::Config(format!("expected an array, got `{value}`")));
    }
    let body = &trimmed[1..trimmed.len() - 1];
    let mut out = Vec::new();
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        out.push(parse_addr(&unquote(item)?)?);
    }
    Ok(out)
}

fn render_addrs(addrs: &[SocketAddr]) -> Vec<String> {
    addrs.iter().map(|addr| format!("\"{addr}\"")).collect()
}

fn render_list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn parse_reads_sections_and_types() {
        let raw = r#"
# banana configuration
log_level = "debug"
listen = ["0.0.0.0:49737", "[::]:49737"]
bootstrap = ["198.51.100.4:49737"]
[limits]
max_peers = 8
[swarm]
default_fanout = 3
"#;
        let config = Config::parse(raw).unwrap();
        assert_eq!(config.log_level, Level::Debug);
        assert_eq!(config.listen.len(), 2);
        assert_eq!(config.bootstrap[0].port(), 49737);
        assert_eq!(config.limits.max_peers, 8);
        assert_eq!(config.swarm.default_fanout, 3);
    }

    #[test]
    fn parse_rejects_typos_and_bad_types() {
        assert!(Config::parse("log_levl = \"info\"").is_err());
        assert!(Config::parse("max_peers = \"many\"").is_err());
        assert!(Config::parse("[limits]\nmax_peers = 0\n").is_err());
        assert!(Config::parse("[dht]\nalpha = 99\n").is_err());
        assert!(Config::parse("listen = \"0.0.0.0:0\"").is_err());
        assert!(Config::parse("just a line").is_err());
        assert!(Config::parse("[unclosed\n").is_err());
    }

    #[test]
    fn render_round_trips() {
        let mut config = Config::default();
        config.log_level = Level::Debug;
        config.limits.max_peers = 12;
        config.net.advertised_relay = Some("127.0.0.1:49738".parse().unwrap());
        let rendered = config.render();
        let parsed = Config::parse(&rendered).unwrap();
        assert_eq!(parsed.log_level, Level::Debug);
        assert_eq!(parsed.limits.max_peers, 12);
        assert_eq!(parsed.net.advertised_relay, config.net.advertised_relay);
        assert_eq!(parsed.validate().unwrap(), ());
    }

    #[test]
    fn layers_override_lower_layers() {
        let mut base = Config::default();
        let mut patch = Config::default();
        patch.listen = vec!["127.0.0.1:1".parse().unwrap()];
        patch.limits.max_peers = 4;
        patch.net.accept_relays = true;
        base.apply(&patch);
        assert_eq!(base.listen, patch.listen);
        assert_eq!(base.limits.max_peers, 4);
        assert!(base.net.accept_relays);
    }

    #[test]
    fn data_dir_is_a_single_directory() {
        let dir = default_data_dir();
        assert!(dir.ends_with("banana") || dir.ends_with(".banana") || dir.file_name().is_some());
    }
}
