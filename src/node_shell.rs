//! A root shell on a node (Lens's "Node shell"): a privileged pod pinned to
//! the node, sharing its PID, network and IPC namespaces, in which `nsenter`
//! enters the host's mount namespace. The pod is deleted when the shell
//! ends; [`leftover_node_shells`] finds any a crash left behind.

use crate::actions::{api_error, describe_status, random_suffix};
use crate::debug::wait_container;
use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{Api, AttachParams, AttachedProcess, DeleteParams, ListParams, PostParams};
use kube::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Pod Security Admission usually lets this namespace run privileged pods.
pub const NODE_SHELL_NAMESPACE: &str = "kube-system";
pub const NODE_SHELL_IMAGE: &str = "busybox:1.37";
/// Every node shell pod carries this label (value `true`).
pub const NODE_SHELL_LABEL: &str = "kubeloupe.dev/node-shell";
pub const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
/// The pod outlives a forgotten session by at most this long.
const KEEP_ALIVE_SECS: u32 = 14400;
const START_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_NAME: usize = 63;

/// Run in the pod: enter every namespace of the host's init process, then
/// start bash if the host has it, else sh.
pub fn node_shell_command() -> Vec<String> {
    [
        "nsenter",
        "-t",
        "1",
        "-m",
        "-u",
        "-i",
        "-n",
        "-p",
        "--",
        "sh",
        "-c",
        "if command -v bash >/dev/null 2>&1; then exec bash -l; else exec sh -l; fi",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `node-shell-<node>-<suffix>`, shortening the node part to fit 63
/// characters. Pure.
pub fn node_shell_name(node: &str, suffix: &str) -> String {
    let tail = format!("-{suffix}");
    let head = format!("node-shell-{node}");
    let keep = MAX_NAME.saturating_sub(tail.len());
    let head: String = head.chars().take(keep).collect();
    format!("{}{tail}", head.trim_end_matches(['-', '.']))
}

/// The privileged pod for a shell on `node`. Pure.
pub fn node_shell_pod(name: &str, node: &str, image: &str) -> Pod {
    serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": name,
            "labels": {
                MANAGED_BY: "kubeloupe",
                NODE_SHELL_LABEL: "true",
            },
        },
        "spec": {
            "nodeName": node,
            "hostPID": true,
            "hostNetwork": true,
            "hostIPC": true,
            "restartPolicy": "Never",
            "terminationGracePeriodSeconds": 0,
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            // Run on any node, whatever its taints.
            "tolerations": [{ "operator": "Exists" }],
            "containers": [{
                "name": "shell",
                "image": image,
                "command": ["sleep", KEEP_ALIVE_SECS.to_string()],
                "securityContext": { "privileged": true },
            }],
        },
    }))
    .expect("node shell pod is valid")
}

/// Plain words for a failure to create the node shell pod in `namespace`.
pub fn describe_node_shell_status(code: u16, message: &str, namespace: &str) -> String {
    let hint = "Try a namespace whose Pod Security level allows privileged pods.";
    if message.contains("PodSecurity") {
        return format!(
            "Pod Security Admission does not allow a privileged pod in {namespace}. {hint} {message}"
        );
    }
    match code {
        403 => format!(
            "Not allowed: a node shell needs permission to create pods (and pods/exec) in {namespace}. {hint} {message}"
        ),
        _ => describe_status(code, message),
    }
}

pub fn describe_node_shell_error(err: &anyhow::Error, namespace: &str) -> String {
    match api_error(err) {
        Some((code, msg)) => describe_node_shell_status(code, &msg, namespace),
        None => format!("{err:#}"),
    }
}

/// A fresh name for a node shell pod on `node`.
pub fn new_node_shell_name(node: &str) -> String {
    node_shell_name(node, &random_suffix())
}

/// Create the node shell pod `name` on `node` and wait until it runs. On
/// any failure after creating it, the pod is deleted again. The caller
/// picks the name first, so it can delete the pod even if this is cancelled.
pub async fn create_node_shell(
    client: &Client,
    node: &str,
    namespace: &str,
    name: &str,
    image: &str,
    progress: impl FnMut(&str),
) -> Result<()> {
    // A pod bound to a missing node would wait forever.
    let nodes: Api<Node> = Api::all(client.clone());
    // (Users who may not read nodes skip the check.)
    if let Ok(None) = nodes.get_opt(node).await {
        anyhow::bail!("there is no node named {node}");
    }
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let image = if image.trim().is_empty() {
        NODE_SHELL_IMAGE
    } else {
        image.trim()
    };
    pods.create(&PostParams::default(), &node_shell_pod(name, node, image))
        .await?;
    let waited = wait_container(
        client,
        namespace,
        name,
        START_TIMEOUT,
        |p| {
            p.status
                .as_ref()?
                .container_statuses
                .as_ref()?
                .first()?
                .state
                .clone()
        },
        progress,
    )
    .await;
    if let Err(e) = waited {
        let _ = delete_node_shell(client, namespace, name).await;
        return Err(e.context(format!("node shell pod {namespace}/{name}")));
    }
    Ok(())
}

/// Open the shell: `nsenter` into the host, with a TTY or with plain pipes.
pub async fn exec_node_shell(
    client: &Client,
    namespace: &str,
    pod: &str,
    tty: bool,
) -> Result<AttachedProcess> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = AttachParams {
        container: Some("shell".to_string()),
        stdin: true,
        stdout: true,
        stderr: !tty,
        tty,
        ..AttachParams::default()
    };
    Ok(pods.exec(pod, node_shell_command(), &params).await?)
}

/// Delete a node shell pod right away. Already gone is fine.
pub async fn delete_node_shell(client: &Client, namespace: &str, pod: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    match pods
        .delete(pod, &DeleteParams::default().grace_period(0))
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(r)) if r.code == 404 => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot delete node shell pod {namespace}/{pod}")),
    }
}

/// A node shell pod still in the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct NodeShellPod {
    pub namespace: String,
    pub name: String,
    pub node: String,
}

/// Node shell pods in any namespace (left behind by a crash, or open in
/// another window).
pub async fn leftover_node_shells(client: &Client) -> Result<Vec<NodeShellPod>> {
    let pods: Api<Pod> = Api::all(client.clone());
    let list = pods
        .list(&ListParams::default().labels(&format!("{NODE_SHELL_LABEL}=true")))
        .await?;
    Ok(list
        .items
        .into_iter()
        .map(|p| NodeShellPod {
            namespace: p.metadata.namespace.unwrap_or_default(),
            name: p.metadata.name.unwrap_or_default(),
            node: p.spec.and_then(|s| s.node_name).unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_shell_pod_is_privileged_on_the_node() {
        let pod = node_shell_pod("node-shell-w-abcde", "w", "busybox:1.37");
        let v = serde_json::to_value(&pod).unwrap();
        let labels = &v["metadata"]["labels"];
        assert_eq!(labels[MANAGED_BY], "kubeloupe");
        assert_eq!(labels[NODE_SHELL_LABEL], "true");
        let spec = &v["spec"];
        assert_eq!(spec["nodeName"], "w");
        for key in ["hostPID", "hostNetwork", "hostIPC"] {
            assert_eq!(spec[key], true, "{key}");
        }
        assert_eq!(
            spec["tolerations"],
            serde_json::json!([{ "operator": "Exists" }])
        );
        assert_eq!(spec["restartPolicy"], "Never");
        let c = &spec["containers"][0];
        assert_eq!(c["image"], "busybox:1.37");
        assert_eq!(c["securityContext"]["privileged"], true);
        assert_eq!(c["command"], serde_json::json!(["sleep", "14400"]));
    }

    #[test]
    fn node_shell_name_fits_63_chars() {
        assert_eq!(node_shell_name("w", "abcde"), "node-shell-w-abcde");
        let long = "a".repeat(80);
        let n = node_shell_name(&long, "abcde");
        assert_eq!(n.len(), 63);
        assert!(n.ends_with("a-abcde"));
        let dotted = format!("{}.b", "x".repeat(46));
        assert!(!node_shell_name(&dotted, "abcde").contains(".-"));
    }

    #[test]
    fn node_shell_enters_the_host() {
        let cmd = node_shell_command();
        assert_eq!(
            &cmd[..9],
            ["nsenter", "-t", "1", "-m", "-u", "-i", "-n", "-p", "--"]
        );
        assert!(cmd.last().unwrap().contains("exec bash"));
    }

    #[test]
    fn forbidden_messages_suggest_another_namespace() {
        let psa = describe_node_shell_status(
            403,
            "pods \"x\" is forbidden: violates PodSecurity \"baseline:latest\": privileged",
            "team-a",
        );
        assert!(
            psa.starts_with("Pod Security Admission does not allow"),
            "{psa}"
        );
        assert!(psa.contains("team-a"));
        let rbac = describe_node_shell_status(403, "pods is forbidden: User \"u\"", "kube-system");
        assert!(rbac.starts_with("Not allowed"), "{rbac}");
        assert!(rbac.contains("privileged pods"));
    }
}
