//! Ephemeral debug containers (`kubectl debug`): add a container with a
//! shell to a running pod, sharing the process namespace of one of its
//! containers, and attach to it. For images without a shell (distroless,
//! scratch, pause).
//!
//! An ephemeral container cannot be removed again: it stays in the pod spec
//! until the pod is deleted. Its process ends when the shell exits.

use crate::actions::{api_error, describe_status, random_suffix};
use anyhow::{bail, Context, Result};
use k8s_openapi::api::core::v1::{ContainerState, ContainerStatus, EphemeralContainer, Pod};
use kube::api::{Api, AttachParams, AttachedProcess, Patch, PatchParams};
use kube::Client;
use std::time::Duration;

/// The default debug image: small, and has `sh`, `ps`, `wget`, `nslookup`.
pub const DEFAULT_DEBUG_IMAGE: &str = "busybox:1.37";
/// How long the container may take to start (image pulls included).
pub const START_TIMEOUT: Duration = Duration::from_secs(180);
/// Shown before a debug session starts, in the UI and the CLI.
pub const EPHEMERAL_NOTE: &str =
    "Ephemeral containers cannot be removed: the debug container stays in the pod until the pod is deleted.";

/// `debugger-<suffix>`.
pub fn debug_container_name(suffix: &str) -> String {
    format!("debugger-{suffix}")
}

/// The ephemeral container added to the pod. `target` is the container whose
/// process namespace it joins, so `ps` lists that container's processes.
/// `tty` is false for a CLI run with piped stdin. Pure.
pub fn debug_container_spec(
    name: &str,
    target: &str,
    image: &str,
    tty: bool,
) -> EphemeralContainer {
    EphemeralContainer {
        name: name.to_string(),
        image: Some(image.to_string()),
        stdin: Some(true),
        tty: Some(tty),
        target_container_name: (!target.is_empty()).then(|| target.to_string()),
        termination_message_policy: Some("File".to_string()),
        ..EphemeralContainer::default()
    }
}

/// Where a container is on its way to running. Pure helper of the waits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartState {
    Running,
    /// Still starting; the waiting reason, if any (`ContainerCreating`, ...).
    Waiting(String),
    /// Will not start: the reason in plain words.
    Failed(String),
}

/// Waiting reasons that mean the container will not start without a change.
const FATAL_WAITING: &[&str] = &[
    "ErrImagePull",
    "ImagePullBackOff",
    "InvalidImageName",
    "ErrImageNeverPull",
    "CreateContainerError",
    "CreateContainerConfigError",
    "RunContainerError",
];

/// Classify a container's state. Pure.
pub fn start_state(state: Option<&ContainerState>) -> StartState {
    let Some(state) = state else {
        return StartState::Waiting(String::new());
    };
    if state.running.is_some() {
        return StartState::Running;
    }
    if let Some(t) = &state.terminated {
        let reason = t.reason.clone().unwrap_or_else(|| "Terminated".into());
        let msg = t.message.as_deref().map(str::trim).unwrap_or("");
        let mut text = format!("the container exited ({reason}, exit code {})", t.exit_code);
        if !msg.is_empty() {
            text.push_str(&format!(": {msg}"));
        }
        return StartState::Failed(text);
    }
    let w = state.waiting.as_ref();
    let reason = w.and_then(|w| w.reason.clone()).unwrap_or_default();
    if FATAL_WAITING.contains(&reason.as_str()) {
        let msg = w.and_then(|w| w.message.as_deref()).unwrap_or("").trim();
        let what = if reason.contains("Image") {
            "the image could not be pulled"
        } else {
            "the container could not be created"
        };
        return StartState::Failed(if msg.is_empty() {
            format!("{what} ({reason})")
        } else {
            format!("{what} ({reason}): {msg}")
        });
    }
    StartState::Waiting(reason)
}

/// The status of `container` among the pod's ephemeral container statuses.
fn ephemeral_status<'a>(pod: &'a Pod, container: &str) -> Option<&'a ContainerStatus> {
    pod.status
        .as_ref()?
        .ephemeral_container_statuses
        .as_ref()?
        .iter()
        .find(|s| s.name == container)
}

/// Poll the pod until `status_of` says the container runs. Reports each new
/// waiting reason through `progress`.
pub(crate) async fn wait_container<F>(
    client: &Client,
    namespace: &str,
    pod: &str,
    timeout: Duration,
    status_of: F,
    mut progress: impl FnMut(&str),
) -> Result<()>
where
    F: Fn(&Pod) -> Option<ContainerState>,
{
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = String::new();
    loop {
        let p = pods.get(pod).await?;
        match start_state(status_of(&p).as_ref()) {
            StartState::Running => return Ok(()),
            StartState::Failed(text) => bail!("{text}"),
            StartState::Waiting(reason) => {
                if !reason.is_empty() && reason != last {
                    progress(&reason);
                    last = reason;
                }
            }
        }
        if tokio::time::Instant::now() > deadline {
            bail!(
                "the container did not start within {}s{}",
                timeout.as_secs(),
                if last.is_empty() {
                    String::new()
                } else {
                    format!(" (still {last})")
                }
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Plain words for a failure to add the debug container.
pub fn describe_debug_status(code: u16, message: &str) -> String {
    match code {
        // No such subresource: the server predates ephemeral containers or
        // has them turned off. (A missing pod also answers 404, with a
        // message naming the pod.)
        404 if !message.contains("pods \"") => format!(
            "Ephemeral containers are not available on this cluster (the pods/ephemeralcontainers subresource is missing). {message}"
        ),
        403 => format!(
            "Not allowed: adding a debug container needs permission to patch pods/ephemeralcontainers. {message}"
        ),
        _ => describe_status(code, message),
    }
}

/// Like [`crate::actions::describe_error`], for debug containers.
pub fn describe_debug_error(err: &anyhow::Error) -> String {
    match api_error(err) {
        Some((code, msg)) => describe_debug_status(code, &msg),
        None => format!("{err:#}"),
    }
}

/// Which container to target: `requested`, or the pod's only (or first)
/// regular container. Errors name the containers there are.
pub fn pick_target(pod: &Pod, requested: Option<&str>) -> Result<String> {
    let names: Vec<String> = pod
        .spec
        .as_ref()
        .map(|s| s.containers.iter().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    match requested {
        Some(c) if names.iter().any(|n| n == c) => Ok(c.to_string()),
        Some(c) => bail!(
            "the pod has no container named {c} (it has: {})",
            names.join(", ")
        ),
        None => names
            .into_iter()
            .next()
            .context("the pod has no containers"),
    }
}

/// Add a debug container to `pod`, targeting `target` (empty: the first
/// container), and wait until it runs. Returns the container's name.
pub async fn add_debug_container(
    client: &Client,
    namespace: &str,
    pod: &str,
    target: &str,
    image: &str,
    tty: bool,
    progress: impl FnMut(&str),
) -> Result<String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let current = pods.get(pod).await?;
    let target = pick_target(&current, (!target.is_empty()).then_some(target))?;
    if current.status.as_ref().and_then(|s| s.phase.as_deref()) != Some("Running") {
        bail!("the pod is not running: a debug container can only join a running pod");
    }
    let image = if image.trim().is_empty() {
        DEFAULT_DEBUG_IMAGE
    } else {
        image.trim()
    };
    let name = debug_container_name(&random_suffix());
    let spec = debug_container_spec(&name, &target, image, tty);
    let patch = serde_json::json!({ "spec": { "ephemeralContainers": [spec] } });
    pods.patch_ephemeral_containers(
        pod,
        &PatchParams {
            field_manager: Some("kubeloupe".to_string()),
            ..PatchParams::default()
        },
        &Patch::Strategic(patch),
    )
    .await?;
    let n = name.clone();
    wait_container(
        client,
        namespace,
        pod,
        START_TIMEOUT,
        move |p| ephemeral_status(p, &n).and_then(|s| s.state.clone()),
        progress,
    )
    .await
    .with_context(|| format!("debug container {name}"))?;
    Ok(name)
}

/// Attach to a running container's stdin/stdout (`kubectl attach -it`).
pub async fn attach_container(
    client: &Client,
    namespace: &str,
    pod: &str,
    container: &str,
    tty: bool,
) -> Result<AttachedProcess> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let params = AttachParams {
        container: Some(container.to_string()),
        stdin: true,
        stdout: true,
        // A TTY merges stderr into stdout; the API refuses both.
        stderr: !tty,
        tty,
        ..AttachParams::default()
    };
    Ok(pods.attach(pod, &params).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{ContainerStateTerminated, ContainerStateWaiting};

    #[test]
    fn debug_container_targets_the_chosen_container() {
        let c = debug_container_spec("debugger-abcde", "app", "busybox:1.37", true);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["name"], "debugger-abcde");
        assert_eq!(v["image"], "busybox:1.37");
        assert_eq!(v["stdin"], true);
        assert_eq!(v["tty"], true);
        assert_eq!(v["targetContainerName"], "app");
        assert!(v.get("command").is_none(), "the image's own shell runs");
        let c = debug_container_spec("d", "", "x", false);
        assert_eq!(c.target_container_name, None);
        assert_eq!(c.tty, Some(false));
        assert!(debug_container_name("abcde").len() <= 63);
        assert_eq!(debug_container_name("abcde"), "debugger-abcde");
    }

    fn waiting(reason: &str, message: &str) -> ContainerState {
        ContainerState {
            waiting: Some(ContainerStateWaiting {
                reason: Some(reason.into()),
                message: Some(message.into()),
            }),
            ..ContainerState::default()
        }
    }

    #[test]
    fn start_state_reports_image_pull_failures() {
        assert_eq!(start_state(None), StartState::Waiting(String::new()));
        assert_eq!(
            start_state(Some(&waiting("ContainerCreating", ""))),
            StartState::Waiting("ContainerCreating".into())
        );
        let StartState::Failed(text) = start_state(Some(&waiting(
            "ImagePullBackOff",
            "Back-off pulling image \"nope:1\"",
        ))) else {
            panic!("not failed");
        };
        assert!(text.starts_with("the image could not be pulled (ImagePullBackOff)"));
        assert!(text.contains("nope:1"));
        let running = ContainerState {
            running: Some(Default::default()),
            ..ContainerState::default()
        };
        assert_eq!(start_state(Some(&running)), StartState::Running);
        let done = ContainerState {
            terminated: Some(ContainerStateTerminated {
                exit_code: 127,
                reason: Some("Error".into()),
                ..ContainerStateTerminated::default()
            }),
            ..ContainerState::default()
        };
        assert_eq!(
            start_state(Some(&done)),
            StartState::Failed("the container exited (Error, exit code 127)".into())
        );
    }

    #[test]
    fn debug_errors_are_plain_words() {
        assert!(describe_debug_status(403, "pods \"p\" is forbidden")
            .contains("pods/ephemeralcontainers"));
        assert!(
            describe_debug_status(404, "the server could not find the requested resource")
                .starts_with("Ephemeral containers are not available")
        );
        assert!(describe_debug_status(404, "pods \"p\" not found").starts_with("Already gone"));
    }

    #[test]
    fn target_defaults_to_the_first_container() {
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "p" },
            "spec": { "containers": [{ "name": "app" }, { "name": "side" }] }
        }))
        .unwrap();
        assert_eq!(pick_target(&pod, None).unwrap(), "app");
        assert_eq!(pick_target(&pod, Some("side")).unwrap(), "side");
        let err = pick_target(&pod, Some("x")).unwrap_err().to_string();
        assert!(err.contains("it has: app, side"), "{err}");
    }
}
