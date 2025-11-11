use std::collections::HashMap;
use std::fmt;

use anyhow::{Result, anyhow};
use serde_yaml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkloadKind {
    Deployment,
    StatefulSet,
    DaemonSet,
    Custom(String),
}

impl WorkloadKind {
    pub fn from_str(kind: &str) -> Self {
        match kind.trim() {
            k if k.eq_ignore_ascii_case("Deployment") => WorkloadKind::Deployment,
            k if k.eq_ignore_ascii_case("StatelessWorkload") => WorkloadKind::Deployment,
            k if k.eq_ignore_ascii_case("StatefulSet") => WorkloadKind::StatefulSet,
            k if k.eq_ignore_ascii_case("StatefulWorkload") => WorkloadKind::StatefulSet,
            k if k.eq_ignore_ascii_case("DaemonSet") => WorkloadKind::DaemonSet,
            other => WorkloadKind::Custom(other.to_string()),
        }
    }

    pub fn matches(&self, other: &WorkloadKind) -> bool {
        match (self, other) {
            (WorkloadKind::Custom(a), WorkloadKind::Custom(b)) => a.eq_ignore_ascii_case(b),
            _ => self == other,
        }
    }

    pub fn task_kind(&self) -> &str {
        match self {
            WorkloadKind::Deployment => "StatelessWorkload",
            WorkloadKind::StatefulSet => "StatefulWorkload",
            WorkloadKind::DaemonSet => "DaemonSet",
            WorkloadKind::Custom(_) => "CustomWorkload",
        }
    }

    pub fn is_stateful(&self) -> bool {
        matches!(self, WorkloadKind::StatefulSet)
    }
}

impl fmt::Display for WorkloadKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkloadKind::Deployment => write!(f, "Deployment"),
            WorkloadKind::StatefulSet => write!(f, "StatefulSet"),
            WorkloadKind::DaemonSet => write!(f, "DaemonSet"),
            WorkloadKind::Custom(value) => write!(f, "{}", value),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HttpProbe {
    pub path: String,
    pub port: u16,
    pub scheme: String,
    pub host: Option<String>,
    pub initial_delay_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct WorkloadManifest {
    pub kind: WorkloadKind,
    pub name: String,
    pub replicas: usize,
    pub liveness_http: Vec<HttpProbe>,
    pub readiness_http: Vec<HttpProbe>,
}

pub fn parse_workload_manifest(manifest: &[u8]) -> Result<WorkloadManifest> {
    parse_workload_manifest_filtered(manifest, None)
}

pub fn parse_manifest_name(manifest: &[u8], kind: &str) -> Result<String> {
    if manifest.is_empty() {
        return Err(anyhow!("manifest content is empty"));
    }
    let target_kind = WorkloadKind::from_str(kind);
    let spec = parse_workload_manifest_filtered(manifest, Some(&target_kind))?;
    Ok(spec.name)
}

pub fn detect_workload_kind(manifest: &[u8]) -> Result<String> {
    let spec = parse_workload_manifest(manifest)?;
    Ok(spec.kind.to_string())
}

fn parse_workload_manifest_filtered(
    manifest: &[u8],
    filter: Option<&WorkloadKind>,
) -> Result<WorkloadManifest> {
    if manifest.is_empty() {
        return Err(anyhow!("manifest content is empty"));
    }

    let docs = split_yaml_documents(std::str::from_utf8(manifest)?);
    for doc in docs {
        let value: Value = serde_yaml::from_str(&doc)?;
        let Some(kind_value) = value.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let kind = WorkloadKind::from_str(kind_value);
        if let Some(filter_kind) = filter {
            if !kind.matches(filter_kind) {
                continue;
            }
        }

        let name = value
            .get("metadata")
            .and_then(|meta| meta.get("name"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("manifest missing metadata.name"))?;

        let replicas = value
            .get("spec")
            .and_then(|spec| spec.get("replicas"))
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(1);

        let container_specs = value
            .get("spec")
            .and_then(|spec| spec.get("template"))
            .and_then(|template| template.get("spec"))
            .and_then(|spec| spec.get("containers"))
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();

        let mut liveness_http = Vec::new();
        let mut readiness_http = Vec::new();

        for container in container_specs {
            let ports = collect_ports(&container);
            if let Some(probe) = container
                .get("livenessProbe")
                .and_then(|probe| parse_http_probe(probe, &ports))
            {
                liveness_http.push(probe);
            }
            if let Some(probe) = container
                .get("readinessProbe")
                .and_then(|probe| parse_http_probe(probe, &ports))
            {
                readiness_http.push(probe);
            }
        }

        return Ok(WorkloadManifest {
            kind,
            name,
            replicas,
            liveness_http,
            readiness_http,
        });
    }

    Err(anyhow!(
        "could not locate workload manifest in provided YAML"
    ))
}

fn collect_ports(container: &Value) -> HashMap<String, u16> {
    let mut ports = HashMap::new();
    if let Some(sequence) = container.get("ports").and_then(Value::as_sequence) {
        for item in sequence {
            if let (Some(name), Some(port)) = (
                item.get("name").and_then(Value::as_str),
                item.get("containerPort").and_then(Value::as_u64),
            ) {
                if port <= u16::MAX as u64 {
                    ports.insert(name.to_string(), port as u16);
                }
            }
        }
    }
    ports
}

fn parse_http_probe(value: &Value, ports: &HashMap<String, u16>) -> Option<HttpProbe> {
    let http = value.get("httpGet")?;
    let path = http
        .get("path")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "/".to_string());
    let scheme = http
        .get("scheme")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "HTTP".to_string());
    let host = http
        .get("host")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    let port_value = http.get("port")?;
    let port = resolve_port(port_value, ports)?;

    let initial_delay_seconds = value.get("initialDelaySeconds").and_then(Value::as_u64);
    let timeout_seconds = value.get("timeoutSeconds").and_then(Value::as_u64);

    Some(HttpProbe {
        path,
        port,
        scheme,
        host,
        initial_delay_seconds,
        timeout_seconds,
    })
}

fn resolve_port(port_value: &Value, ports: &HashMap<String, u16>) -> Option<u16> {
    match port_value {
        Value::Number(n) => n
            .as_u64()
            .and_then(|p| (p <= u16::MAX as u64).then_some(p as u16)),
        Value::String(s) => {
            if let Ok(number) = s.parse::<u16>() {
                Some(number)
            } else {
                ports.get(s).copied()
            }
        }
        _ => None,
    }
}

fn split_yaml_documents(input: &str) -> Vec<String> {
    input
        .split("---")
        .map(|doc| doc.trim().to_string())
        .filter(|doc| !doc.is_empty())
        .collect()
}
