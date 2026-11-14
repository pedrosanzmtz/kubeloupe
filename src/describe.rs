//! `kubeloupe describe <kind>/<name>`: a [`ResourceDetail`] printed the way
//! `kubectl describe` prints an object.

use crate::get::{object_namespace, resolve_kind_named};
use crate::resources::{
    self, align_columns, ContainerDetail, ContainerKind, PodExtras, ResourceDetail, StateInfo,
};
use anyhow::Result;
use kube::Client;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Width of the `Label:` column.
const LABEL_WIDTH: usize = 15;

/// Fetch one object's detail and print it.
pub async fn print_describe(
    client: &Client,
    kind: &str,
    name: &str,
    namespace: Option<&str>,
) -> Result<()> {
    let kind = resolve_kind_named(client, kind).await?;
    let ns = object_namespace(client, &kind.resource, namespace);
    let detail = resources::get_resource_detail(client, &kind.resource, &ns, name).await?;
    print!("{}", render_describe(&detail));
    Ok(())
}

/// The describe text: metadata, owners, labels and annotations, conditions,
/// Secret keys and events, one section after the other.
pub fn render_describe(d: &ResourceDetail) -> String {
    let mut out = String::new();
    field(&mut out, "Name", &d.name);
    if !d.namespace.is_empty() {
        field(&mut out, "Namespace", &d.namespace);
    }
    field(&mut out, "Kind", &d.kind);
    field(&mut out, "API Version", &d.api_version);
    field(&mut out, "UID", &d.uid);
    field(&mut out, "Created", &created_text(d.created, &d.age));
    field(&mut out, "Labels", &pairs(&d.labels));
    field(&mut out, "Annotations", &pairs(&d.annotations));
    field(
        &mut out,
        "Finalizers",
        &none_if_empty(d.finalizers.join("\n")),
    );
    let (controllers, others): (Vec<_>, Vec<_>) = d.owners.iter().partition(|o| o.controller);
    for o in controllers {
        field(&mut out, "Controlled By", &format!("{}/{}", o.kind, o.name));
    }
    if !others.is_empty() {
        let owners: Vec<_> = others
            .iter()
            .map(|o| format!("{}/{}", o.kind, o.name))
            .collect();
        field(&mut out, "Owners", &owners.join("\n"));
    }
    if d.kind == "Pod" {
        pod_section(&mut out, &d.pod);
    }
    // Related objects (#35): add a "Related:" section here (selected pods,
    // services, mounted config) once `ResourceDetail` carries them.
    if !d.conditions.is_empty() {
        let mut table = vec![row(&["Type", "Status", "Reason", "Age", "Message"])];
        table.extend(d.conditions.iter().map(|c| {
            row(&[
                &c.type_,
                &c.status,
                &dash(&c.reason),
                &dash(&c.age),
                &dash(&c.message),
            ])
        }));
        section(&mut out, "Conditions", &table);
    }
    if d.kind == "Secret" {
        if d.secret_keys.is_empty() {
            field(&mut out, "Data", "<none>");
        } else {
            out.push_str("Data:\n");
            for k in &d.secret_keys {
                let _ = writeln!(out, "  {}: {} bytes", k.key, k.bytes);
            }
        }
    }
    if d.events.is_empty() {
        field(&mut out, "Events", "<none>");
    } else {
        let mut table = vec![row(&["Type", "Reason", "Age", "Count", "Message"])];
        table.extend(d.events.iter().map(|e| {
            row(&[
                &e.type_,
                &e.reason,
                &e.age,
                &e.count.to_string(),
                &e.message,
            ])
        }));
        section(&mut out, "Events", &table);
    }
    out
}

/// Pod status, placement and one block per container (env names only: the
/// detail never carries values).
fn pod_section(out: &mut String, p: &PodExtras) {
    field(out, "Status", &none_if_empty(p.phase.clone()));
    if !p.reason.is_empty() {
        field(out, "Reason", &p.reason);
    }
    if !p.message.is_empty() {
        field(out, "Message", &p.message);
    }
    field(out, "Node", &none_if_empty(p.node.clone()));
    field(out, "IPs", &none_if_empty(p.pod_ips.join("\n")));
    field(out, "Host IP", &none_if_empty(p.host_ip.clone()));
    field(out, "QoS Class", &none_if_empty(p.qos_class.clone()));
    if !p.priority_class.is_empty() {
        field(out, "Priority Class", &p.priority_class);
    }
    field(
        out,
        "Service Account",
        &none_if_empty(p.service_account.clone()),
    );
    for (kind, title) in [
        (ContainerKind::Init, "Init Containers"),
        (ContainerKind::Regular, "Containers"),
        (ContainerKind::Ephemeral, "Ephemeral Containers"),
    ] {
        let containers: Vec<_> = p.containers.iter().filter(|c| c.kind == kind).collect();
        if containers.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{title}:");
        for c in containers {
            let _ = writeln!(out, "  {}:", c.name);
            let mut block = String::new();
            container_fields(&mut block, c);
            for line in block.lines() {
                let _ = writeln!(out, "    {line}");
            }
        }
    }
}

fn container_fields(out: &mut String, c: &ContainerDetail) {
    field(out, "Image", &c.image);
    field(out, "State", &state_text(&c.state));
    if !c.last_state.state.is_empty() {
        field(out, "Last State", &state_text(&c.last_state));
    }
    field(out, "Ready", if c.ready { "True" } else { "False" });
    field(out, "Restarts", &c.restarts.to_string());
    field(out, "Requests", &pairs(&c.requests));
    field(out, "Limits", &pairs(&c.limits));
    if !c.ports.is_empty() {
        let ports: Vec<_> = c
            .ports
            .iter()
            .map(|p| {
                let name = if p.name.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", p.name)
                };
                format!("{}/{}{name}", p.port, p.protocol)
            })
            .collect();
        field(out, "Ports", &ports.join("\n"));
    }
    for probe in &c.probes {
        field(out, &capitalized(&probe.kind), &probe.summary);
    }
    if !c.mounts.is_empty() {
        let mounts: Vec<_> = c
            .mounts
            .iter()
            .map(|m| {
                let sub = if m.sub_path.is_empty() {
                    String::new()
                } else {
                    format!(" (path {})", m.sub_path)
                };
                let ro = if m.read_only { " (ro)" } else { "" };
                format!("{} from {}{sub}{ro}", m.path, m.name)
            })
            .collect();
        field(out, "Mounts", &mounts.join("\n"));
    }
    let env: Vec<_> = c
        .env
        .iter()
        .map(|e| {
            if e.source.is_empty() {
                e.name.clone()
            } else {
                format!("{} ({})", e.name, e.source)
            }
        })
        .collect();
    field(out, "Environment", &none_if_empty(env.join("\n")));
}

/// `Running, started 15h ago`, `Waiting: CrashLoopBackOff`,
/// `Terminated: Error (exit code 1), 5m ago`, then the message if any.
fn state_text(s: &StateInfo) -> String {
    if s.state.is_empty() {
        return "<none>".into();
    }
    let mut text = capitalized(&s.state);
    if !s.reason.is_empty() {
        text = format!("{text}: {}", s.reason);
    }
    if s.state == "terminated" {
        text = format!("{text} (exit code {})", s.exit_code);
    }
    if !s.since.is_empty() {
        let verb = if s.state == "running" { "started " } else { "" };
        text = format!("{text}, {verb}{} ago", s.since);
    }
    if !s.message.is_empty() {
        text = format!("{text}\n{}", s.message);
    }
    text
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `Label:   value`, continuation lines of a multi-line value indented under
/// the first.
fn field(out: &mut String, label: &str, value: &str) {
    let mut lines = value.lines();
    let first = lines.next().unwrap_or("");
    let label = format!("{label}:");
    let _ = writeln!(
        out,
        "{}",
        format!("{label:<LABEL_WIDTH$} {first}").trim_end()
    );
    for line in lines {
        let _ = writeln!(out, "{:LABEL_WIDTH$} {line}", "");
    }
}

/// `Label:` and an indented table.
fn section(out: &mut String, label: &str, table: &[Vec<String>]) {
    let _ = writeln!(out, "{label}:");
    for line in align_columns(table).lines() {
        let _ = writeln!(out, "  {line}");
    }
}

fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|c| c.to_string()).collect()
}

fn pairs(map: &BTreeMap<String, String>) -> String {
    none_if_empty(
        map.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn none_if_empty(s: String) -> String {
    if s.is_empty() {
        "<none>".into()
    } else {
        s
    }
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "-".into()
    } else {
        s.to_string()
    }
}

fn created_text(secs: i64, age: &str) -> String {
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(t) if secs > 0 => format!("{} ({age} ago)", t.format("%Y-%m-%dT%H:%M:%SZ")),
        _ => "<unknown>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{
        ConditionInfo, EnvInfo, OwnerLink, PortInfo, ProbeInfo, RelatedEvent, SecretKey,
    };

    fn detail() -> ResourceDetail {
        ResourceDetail {
            name: "hungry".into(),
            namespace: "default".into(),
            uid: "u-1".into(),
            api_version: "v1".into(),
            kind: "Pod".into(),
            created: 1_700_000_000,
            age: "15h".into(),
            labels: BTreeMap::from([("app".into(), "hungry".into()), ("t".into(), "x".into())]),
            annotations: BTreeMap::new(),
            finalizers: vec![],
            owners: vec![OwnerLink {
                api_version: "apps/v1".into(),
                kind: "ReplicaSet".into(),
                name: "web-1".into(),
                controller: true,
            }],
            conditions: vec![ConditionInfo {
                type_: "PodScheduled".into(),
                status: "False".into(),
                reason: "Unschedulable".into(),
                message: "0/2 nodes are available".into(),
                age: "15h".into(),
            }],
            secret_keys: vec![],
            events: vec![RelatedEvent {
                type_: "Warning".into(),
                reason: "FailedScheduling".into(),
                message: "0/2 nodes are available: Insufficient memory.".into(),
                count: 3,
                age: "2m".into(),
            }],
            pod: PodExtras {
                phase: "Pending".into(),
                qos_class: "Burstable".into(),
                service_account: "default".into(),
                containers: vec![ContainerDetail {
                    name: "hungry".into(),
                    kind: ContainerKind::Regular,
                    image: "nginx:1.27".into(),
                    image_id: String::new(),
                    state: StateInfo {
                        state: "waiting".into(),
                        reason: "ContainerCreating".into(),
                        ..StateInfo::default()
                    },
                    last_state: StateInfo {
                        state: "terminated".into(),
                        reason: "Error".into(),
                        exit_code: 1,
                        since: "5m".into(),
                        ..StateInfo::default()
                    },
                    ready: false,
                    restarts: 2,
                    requests: BTreeMap::from([("memory".into(), "100Gi".into())]),
                    limits: BTreeMap::new(),
                    ports: vec![PortInfo {
                        name: "http".into(),
                        port: 80,
                        protocol: "TCP".into(),
                    }],
                    probes: vec![ProbeInfo {
                        kind: "liveness".into(),
                        summary: "http-get :80/ delay=0s".into(),
                    }],
                    mounts: vec![],
                    env: vec![
                        EnvInfo {
                            name: "MODE".into(),
                            source: String::new(),
                        },
                        EnvInfo {
                            name: "PASSWORD".into(),
                            source: "secret:creds/pw".into(),
                        },
                    ],
                }],
                ..PodExtras::default()
            },
        }
    }

    #[test]
    fn describe_prints_metadata_owners_conditions_and_events() {
        let out = render_describe(&detail());
        let expected = "\
Name:           hungry
Namespace:      default
Kind:           Pod
API Version:    v1
UID:            u-1
Created:        2023-11-14T22:13:20Z (15h ago)
Labels:         app=hungry
                t=x
Annotations:    <none>
Finalizers:     <none>
Controlled By:  ReplicaSet/web-1
Status:         Pending
Node:           <none>
IPs:            <none>
Host IP:        <none>
QoS Class:      Burstable
Service Account: default
Containers:
  hungry:
    Image:          nginx:1.27
    State:          Waiting: ContainerCreating
    Last State:     Terminated: Error (exit code 1), 5m ago
    Ready:          False
    Restarts:       2
    Requests:       memory=100Gi
    Limits:         <none>
    Ports:          80/TCP (http)
    Liveness:       http-get :80/ delay=0s
    Environment:    MODE
                    PASSWORD (secret:creds/pw)
Conditions:
  Type          Status  Reason         Age  Message
  PodScheduled  False   Unschedulable  15h  0/2 nodes are available
Events:
  Type     Reason            Age  Count  Message
  Warning  FailedScheduling  2m   3      0/2 nodes are available: Insufficient memory.
";
        assert_eq!(out, expected);
    }

    #[test]
    fn secret_keys_without_values_and_no_events() {
        let mut d = detail();
        d.kind = "Secret".into();
        d.pod = PodExtras::default();
        d.owners.clear();
        d.conditions.clear();
        d.events.clear();
        d.secret_keys = vec![SecretKey {
            key: "password".into(),
            bytes: 8,
        }];
        let out = render_describe(&d);
        assert!(out.contains("Data:\n  password: 8 bytes\n"), "{out}");
        assert!(out.ends_with("Events:         <none>\n"), "{out}");
        assert!(!out.contains("Controlled By"));
        assert!(!out.contains("Conditions"));
    }
}
