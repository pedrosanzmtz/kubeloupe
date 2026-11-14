//! What to forward, as typed on the command line or sent by the UI:
//! targets (`svc/api`), remote ports (`80`, `http`) and port specs
//! (`8080:80`, `:80`, `80`). Pure parsing, no cluster access.

use crate::WorkloadKind;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The kind of object a forward points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub enum ForwardKind {
    Pod,
    Service,
    Deployment,
    StatefulSet,
    ReplicaSet,
    DaemonSet,
}

impl ForwardKind {
    /// The short form used in `<kind>/<name>` (`svc/api`).
    pub fn short(&self) -> &'static str {
        match self {
            Self::Pod => "pod",
            Self::Service => "svc",
            Self::Deployment => "deploy",
            Self::StatefulSet => "sts",
            Self::ReplicaSet => "rs",
            Self::DaemonSet => "ds",
        }
    }

    /// The workload kind whose selector picks the pods, for workloads.
    pub(crate) fn workload(&self) -> Option<WorkloadKind> {
        Some(match self {
            Self::Deployment => WorkloadKind::Deployment,
            Self::StatefulSet => WorkloadKind::StatefulSet,
            Self::ReplicaSet => WorkloadKind::ReplicaSet,
            Self::DaemonSet => WorkloadKind::DaemonSet,
            Self::Pod | Self::Service => return None,
        })
    }

    /// Accepts kubectl-style names and aliases, any case.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "pod" | "pods" | "po" => Self::Pod,
            "service" | "services" | "svc" => Self::Service,
            "deployment" | "deployments" | "deploy" => Self::Deployment,
            "statefulset" | "statefulsets" | "sts" => Self::StatefulSet,
            "replicaset" | "replicasets" | "rs" => Self::ReplicaSet,
            "daemonset" | "daemonsets" | "ds" => Self::DaemonSet,
            other => {
                bail!("cannot port-forward to a '{other}' (use pod, svc, deploy, sts, rs or ds)")
            }
        })
    }
}

/// The object a forward points at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ForwardTarget {
    pub kind: ForwardKind,
    pub namespace: String,
    pub name: String,
}

impl ForwardTarget {
    /// Parse `name` (a pod), `pod/name`, `svc/name`, `deploy/name`, ...
    pub fn parse(s: &str, namespace: &str) -> Result<Self> {
        let (kind, name) = match s.split_once('/') {
            None => (ForwardKind::Pod, s),
            Some((kind, name)) => (ForwardKind::parse(kind)?, name),
        };
        if name.is_empty() || name.contains('/') {
            bail!("expected <name> or <kind>/<name>, got '{s}'");
        }
        Ok(Self {
            kind,
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }
}

impl fmt::Display for ForwardTarget {
    /// `svc/demo/api`: kind, namespace and name.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.kind.short(), self.namespace, self.name)
    }
}

/// The port on the remote side: a number, or a port name (a Service port
/// name, or a container port name for pods and workloads).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RemotePort {
    Number(u16),
    Name(String),
}

impl RemotePort {
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            bail!("missing remote port");
        }
        if s.bytes().all(|b| b.is_ascii_digit()) {
            return match s.parse::<u16>() {
                Ok(n) if n > 0 => Ok(Self::Number(n)),
                _ => bail!("remote port {s} is not between 1 and 65535"),
            };
        }
        // IANA_SVC_NAME, what Kubernetes allows as a port name.
        let valid = s.len() <= 15
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && s.bytes().any(|b| b.is_ascii_lowercase())
            && !s.starts_with('-')
            && !s.ends_with('-')
            && !s.contains("--");
        if !valid {
            bail!("'{s}' is neither a port number nor a valid port name");
        }
        Ok(Self::Name(s.to_string()))
    }
}

impl fmt::Display for RemotePort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Name(s) => f.write_str(s),
        }
    }
}

/// Which local port to listen on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPort {
    /// The remote port number if it is free and at least 1024, otherwise a
    /// random one.
    Auto,
    /// Any free port.
    Random,
    /// This port, or an error saying it is taken.
    Exactly(u16),
}

impl LocalPort {
    /// From an optional number: `None` is [`LocalPort::Auto`], `0` is
    /// [`LocalPort::Random`].
    pub fn from_option(port: Option<u16>) -> Self {
        match port {
            None => Self::Auto,
            Some(0) => Self::Random,
            Some(n) => Self::Exactly(n),
        }
    }

    /// The port to try first, given the remote port number. `None` means a
    /// random one.
    pub fn preferred(&self, remote: u16) -> Option<u16> {
        match self {
            Self::Auto if remote >= 1024 => Some(remote),
            Self::Auto | Self::Random => None,
            Self::Exactly(n) => Some(*n),
        }
    }
}

/// One `[local:]remote` argument, as kubectl takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub local: LocalPort,
    pub remote: RemotePort,
}

impl PortSpec {
    /// `80` (local chosen automatically), `8080:80`, `:80` or `0:80`
    /// (random local port), `8080:http`.
    pub fn parse(s: &str) -> Result<Self> {
        let Some((local, remote)) = s.split_once(':') else {
            return Ok(Self {
                local: LocalPort::Auto,
                remote: RemotePort::parse(s)?,
            });
        };
        let local = if local.is_empty() {
            LocalPort::Random
        } else {
            match local.parse::<u16>() {
                Ok(n) => LocalPort::from_option(Some(n)),
                Err(_) => bail!("local port '{local}' is not a number between 0 and 65535"),
            }
        };
        Ok(Self {
            local,
            remote: RemotePort::parse(remote)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        let t = ForwardTarget::parse("web-1", "demo").unwrap();
        assert_eq!((t.kind, t.name.as_str()), (ForwardKind::Pod, "web-1"));
        let t = ForwardTarget::parse("svc/api", "demo").unwrap();
        assert_eq!((t.kind, t.name.as_str()), (ForwardKind::Service, "api"));
        assert_eq!(t.to_string(), "svc/demo/api");
        for (s, k) in [
            ("service/a", ForwardKind::Service),
            ("pod/a", ForwardKind::Pod),
            ("po/a", ForwardKind::Pod),
            ("deploy/a", ForwardKind::Deployment),
            ("Deployment/a", ForwardKind::Deployment),
            ("sts/a", ForwardKind::StatefulSet),
            ("rs/a", ForwardKind::ReplicaSet),
            ("ds/a", ForwardKind::DaemonSet),
        ] {
            assert_eq!(ForwardTarget::parse(s, "ns").unwrap().kind, k, "{s}");
        }
        assert!(ForwardTarget::parse("svc/", "ns").is_err());
        assert!(ForwardTarget::parse("svc/a/b", "ns").is_err());
        let err = ForwardTarget::parse("configmap/a", "ns").unwrap_err();
        assert!(err
            .to_string()
            .contains("cannot port-forward to a 'configmap'"));
    }

    #[test]
    fn remote_ports() {
        assert_eq!(RemotePort::parse("80").unwrap(), RemotePort::Number(80));
        assert_eq!(
            RemotePort::parse("http").unwrap(),
            RemotePort::Name("http".into())
        );
        assert_eq!(
            RemotePort::parse("http-alt2").unwrap(),
            RemotePort::Name("http-alt2".into())
        );
        for bad in [
            "",
            "0",
            "65536",
            "HTTP",
            "-x",
            "x-",
            "a--b",
            "1234567890abcdef",
            "a_b",
        ] {
            assert!(RemotePort::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn port_specs() {
        let p = PortSpec::parse("8080:80").unwrap();
        assert_eq!(p.local, LocalPort::Exactly(8080));
        assert_eq!(p.remote, RemotePort::Number(80));
        assert_eq!(PortSpec::parse("80").unwrap().local, LocalPort::Auto);
        assert_eq!(PortSpec::parse(":80").unwrap().local, LocalPort::Random);
        assert_eq!(PortSpec::parse("0:80").unwrap().local, LocalPort::Random);
        assert_eq!(
            PortSpec::parse("9000:http").unwrap().remote,
            RemotePort::Name("http".into())
        );
        assert!(PortSpec::parse("x:80").is_err());
        assert!(PortSpec::parse("70000:80").is_err());
        assert!(PortSpec::parse("8080:").is_err());
    }

    #[test]
    fn preferred_local_port() {
        assert_eq!(LocalPort::Auto.preferred(8080), Some(8080));
        assert_eq!(LocalPort::Auto.preferred(80), None);
        assert_eq!(LocalPort::Random.preferred(8080), None);
        assert_eq!(LocalPort::Exactly(80).preferred(8080), Some(80));
        assert_eq!(LocalPort::from_option(None), LocalPort::Auto);
        assert_eq!(LocalPort::from_option(Some(0)), LocalPort::Random);
    }
}
