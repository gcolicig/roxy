use std::net::Ipv4Addr;
use std::process::Command;

use serde::Deserialize;
use thiserror::Error;

/// Default name of the Apple Container binary, resolved via `PATH`.
const DEFAULT_BINARY: &str = "container";

#[derive(Debug, Error)]
pub enum ContainerCliError {
    #[error("Apple Container CLI '{binary}' not found in PATH")]
    NotFound { binary: String },

    #[error("Apple Container CLI failed (exit {code}): {stderr}")]
    CommandFailed { code: String, stderr: String },

    #[error("Failed to run Apple Container CLI: {0}")]
    Io(#[source] std::io::Error),

    #[error("Failed to parse Apple Container CLI output: {0}")]
    Parse(#[source] serde_json::Error),
}

/// Lifecycle state of a container as reported by the CLI.
///
/// Only `running` matters for routing; every other value is kept verbatim
/// so callers can log why a container was skipped instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Stopped,
    Other(String),
}

impl ContainerState {
    pub fn is_running(&self) -> bool {
        matches!(self, ContainerState::Running)
    }
}

impl From<&str> for ContainerState {
    fn from(s: &str) -> Self {
        match s {
            "running" => ContainerState::Running,
            "stopped" => ContainerState::Stopped,
            other => ContainerState::Other(other.to_string()),
        }
    }
}

/// A port published to the host via `container run -p`.
///
/// Apple Container assigns every container its own IP, so publishing is
/// optional — unlike Docker, where it is the only way to reach a container
/// from the host. An empty list is therefore normal, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPort {
    pub container_port: u16,
    pub host_port: u16,
    pub proto: String,
}

/// The subset of `container ls --format json` that routing needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub labels: std::collections::HashMap<String, String>,
    pub state: ContainerState,
    /// First IPv4 address, with the CIDR suffix stripped. `None` while the
    /// container is not running — the CLI reports no networks in that case.
    pub ipv4: Option<Ipv4Addr>,
    pub published_ports: Vec<PublishedPort>,
}

impl Container {
    /// Look up a label, returning `None` for absent or empty values.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}

// ---------------------------------------------------------------------------
// Wire format
//
// Mirrors `container ls --format json` from CLI 1.2.0. Unknown fields are
// ignored, so added keys upstream do not break parsing.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ContainerDto {
    id: String,
    configuration: ConfigurationDto,
    status: StatusDto,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigurationDto {
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
    #[serde(default)]
    published_ports: Vec<PublishedPortDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishedPortDto {
    container_port: u16,
    host_port: u16,
    #[serde(default)]
    proto: String,
}

#[derive(Debug, Deserialize)]
struct StatusDto {
    #[serde(default)]
    state: String,
    #[serde(default)]
    networks: Vec<NetworkDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NetworkDto {
    #[serde(default)]
    ipv4_address: Option<String>,
}

/// Strip the CIDR suffix and parse: `"192.168.64.2/24"` -> `192.168.64.2`.
///
/// Malformed addresses yield `None` rather than an error — a single odd
/// container must not fail the whole listing.
fn parse_ipv4(raw: &str) -> Option<Ipv4Addr> {
    raw.split('/').next()?.parse().ok()
}

impl From<ContainerDto> for Container {
    fn from(dto: ContainerDto) -> Self {
        let ipv4 = dto
            .status
            .networks
            .iter()
            .filter_map(|n| n.ipv4_address.as_deref())
            .find_map(parse_ipv4);

        Self {
            id: dto.id,
            labels: dto.configuration.labels,
            state: ContainerState::from(dto.status.state.as_str()),
            ipv4,
            published_ports: dto
                .configuration
                .published_ports
                .into_iter()
                .map(|p| PublishedPort {
                    container_port: p.container_port,
                    host_port: p.host_port,
                    proto: p.proto,
                })
                .collect(),
        }
    }
}

/// Parse the JSON array emitted by `container ls --format json`.
///
/// Separated from process execution so it can be tested against recorded
/// output without invoking the CLI.
pub fn parse_containers(json: &str) -> Result<Vec<Container>, ContainerCliError> {
    let dtos: Vec<ContainerDto> = serde_json::from_str(json).map_err(ContainerCliError::Parse)?;
    Ok(dtos.into_iter().map(Container::from).collect())
}

/// Thin wrapper around the Apple Container CLI.
///
/// Apple Container exposes no daemon socket and no event stream, so every
/// query is a process invocation. Callers poll; see the watcher.
pub struct ContainerCli {
    binary: String,
}

impl Default for ContainerCli {
    fn default() -> Self {
        Self::new(DEFAULT_BINARY)
    }
}

impl ContainerCli {
    pub fn new(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// List all containers, including stopped ones.
    ///
    /// Stopped containers are included deliberately: the caller needs to see
    /// them disappear from the running set to retract their registrations.
    pub fn list_all(&self) -> Result<Vec<Container>, ContainerCliError> {
        let output = Command::new(&self.binary)
            .args(["ls", "--all", "--format", "json"])
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ContainerCliError::NotFound {
                    binary: self.binary.clone(),
                },
                _ => ContainerCliError::Io(e),
            })?;

        if !output.status.success() {
            return Err(ContainerCliError::CommandFailed {
                code: output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |c| c.to_string()),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        parse_containers(&String::from_utf8_lossy(&output.stdout))
    }

    /// Whether the CLI is present and callable.
    pub fn is_available(&self) -> bool {
        Command::new(&self.binary)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded from `container ls --all --format json` (CLI 1.2.0) with two
    /// containers: one running with a published port, one stopped with labels.
    const FIXTURE: &str = include_str!("fixtures/container_ls.json");

    fn fixture_by_id(id: &str) -> Container {
        parse_containers(FIXTURE)
            .unwrap()
            .into_iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("fixture is missing container '{id}'"))
    }

    #[test]
    fn parses_all_containers_from_fixture() {
        let containers = parse_containers(FIXTURE).unwrap();
        assert_eq!(containers.len(), 2);
    }

    #[test]
    fn running_container_exposes_ipv4_without_cidr_suffix() {
        let container = fixture_by_id("omniroute");
        assert!(container.state.is_running());
        assert_eq!(container.ipv4, Some(Ipv4Addr::new(192, 168, 64, 2)));
    }

    #[test]
    fn running_container_exposes_published_ports() {
        let container = fixture_by_id("omniroute");
        assert_eq!(
            container.published_ports,
            vec![PublishedPort {
                container_port: 20128,
                host_port: 20128,
                proto: "tcp".to_string(),
            }]
        );
    }

    #[test]
    fn stopped_container_has_no_ipv4() {
        let container = fixture_by_id("roxy-fixture-noports");
        assert_eq!(container.state, ContainerState::Stopped);
        assert_eq!(container.ipv4, None);
        assert!(container.published_ports.is_empty());
    }

    #[test]
    fn labels_are_preserved() {
        let container = fixture_by_id("roxy-fixture-noports");
        assert_eq!(container.label("roxy.enable"), Some("true"));
        assert_eq!(container.label("roxy.domain"), Some("fixture.roxy"));
        assert_eq!(container.label("roxy.port"), None);
    }

    #[test]
    fn empty_label_value_reads_as_absent() {
        let container = fixture_by_id("omniroute");
        assert_eq!(container.label("roxy.enable"), None);
    }

    #[test]
    fn parses_empty_listing() {
        assert!(parse_containers("[]").unwrap().is_empty());
    }

    #[test]
    fn unknown_state_is_preserved_verbatim() {
        let json = r#"[{"id":"c","configuration":{},"status":{"state":"stopping"}}]"#;
        let containers = parse_containers(json).unwrap();
        assert_eq!(
            containers[0].state,
            ContainerState::Other("stopping".to_string())
        );
        assert!(!containers[0].state.is_running());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"[{"id":"c","configuration":{},"status":{"state":"running"},"newKey":1}]"#;
        assert_eq!(parse_containers(json).unwrap().len(), 1);
    }

    #[test]
    fn malformed_ipv4_is_dropped_not_fatal() {
        let json = r#"[{"id":"c","configuration":{},
            "status":{"state":"running","networks":[{"ipv4Address":"not-an-ip"}]}}]"#;
        assert_eq!(parse_containers(json).unwrap()[0].ipv4, None);
    }

    #[test]
    fn first_usable_ipv4_wins_when_multiple_networks() {
        let json = r#"[{"id":"c","configuration":{},"status":{"state":"running","networks":[
            {"ipv6Address":"fd00::1/64"},
            {"ipv4Address":"192.168.64.7/24"},
            {"ipv4Address":"10.0.0.1/24"}]}}]"#;
        assert_eq!(
            parse_containers(json).unwrap()[0].ipv4,
            Some(Ipv4Addr::new(192, 168, 64, 7))
        );
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let err = parse_containers("not json").unwrap_err();
        assert!(matches!(err, ContainerCliError::Parse(_)));
    }

    /// Exercises the real CLI to catch argument-spelling drift, which
    /// fixtures cannot. Ignored by default: needs Apple Container installed.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn list_all_against_real_cli() {
        let cli = ContainerCli::default();
        assert!(cli.is_available(), "Apple Container CLI not installed");
        cli.list_all().expect("listing containers failed");
    }

    #[test]
    fn missing_binary_reports_not_found() {
        let cli = ContainerCli::new("roxy-nonexistent-binary");
        let err = cli.list_all().unwrap_err();
        assert!(matches!(err, ContainerCliError::NotFound { .. }));
        assert!(!cli.is_available());
    }
}
