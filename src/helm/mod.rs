//! Helm 3 releases, read natively from the cluster (no `helm` binary), and
//! rollback/uninstall through the `helm` CLI ([`command`]).
//!
//! Helm keeps one object per release revision: a Secret of type
//! `helm.sh/release.v1` named `sh.helm.release.v1.<release>.v<revision>`
//! (or a ConfigMap with `HELM_DRIVER=configmap`), labelled `owner=helm`,
//! `name`, `status`, `version` and `modifiedAt`. Listing reads only that
//! metadata; one revision's payload is downloaded only when it is shown.

mod command;
mod decode;
mod manifest;
mod print;

pub use command::{helm_rollback, helm_uninstall, helm_version, HelmTarget};
pub use manifest::{manifest_resources, redact_manifest, HelmResource};
pub use print::{print_helm_get, print_helm_list, HelmGetOptions, HelmSection};

use crate::is_forbidden;
use anyhow::{bail, Context, Result};
use futures::{stream, StreamExt};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::api::{Api, ListParams, ObjectMeta};
use kube::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How many release payloads [`release_charts`] downloads at once.
const CHART_CONCURRENCY: usize = 6;

/// Where a release is stored: Helm's default Secret driver or the
/// ConfigMap one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum HelmStorage {
    Secret,
    ConfigMap,
}

/// One revision, from its labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmRevision {
    pub revision: u32,
    /// `deployed`, `superseded`, `failed`, `pending-upgrade`, ...
    pub status: String,
    /// Unix seconds of the last change (the `modifiedAt` label, else the
    /// object's creation); 0 if unknown.
    pub updated: i64,
}

/// A release in a list: its newest revision and all revisions' labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmReleaseRow {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    pub status: String,
    pub updated: i64,
    pub storage: HelmStorage,
    /// Every stored revision, newest first.
    pub history: Vec<HelmRevision>,
}

/// The releases in the asked namespaces, and the namespaces that could not
/// be read, in plain words.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmReleaseList {
    /// Sorted by namespace, then name.
    pub releases: Vec<HelmReleaseRow>,
    pub warnings: Vec<String>,
}

/// Which revision to fetch for [`release_charts`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmRevisionKey {
    pub namespace: String,
    pub name: String,
    pub revision: u32,
    pub storage: HelmStorage,
}

/// The chart of one revision, for the list's chart columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmChartInfo {
    pub namespace: String,
    pub name: String,
    pub revision: u32,
    pub chart: String,
    pub chart_version: String,
    pub app_version: String,
    /// Why the payload could not be read; empty on success.
    pub error: String,
}

/// One revision of a release, decoded.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct HelmRelease {
    pub name: String,
    pub namespace: String,
    pub revision: u32,
    pub status: String,
    pub storage: HelmStorage,
    pub chart: String,
    pub chart_version: String,
    pub app_version: String,
    pub description: String,
    pub notes: String,
    /// As Helm stores them (RFC 3339); empty if unknown.
    pub first_deployed: String,
    pub last_deployed: String,
    /// The user-supplied values as YAML; empty if there are none.
    pub values: String,
    /// The chart defaults with the user values on top, as YAML.
    pub computed_values: String,
    /// The rendered manifest; Secret values hidden unless revealed.
    pub manifest: String,
    /// Whether `manifest` has hidden Secret values.
    pub secrets_hidden: bool,
    pub resources: Vec<HelmResource>,
    /// Every stored revision, newest first.
    pub history: Vec<HelmRevision>,
}

const OWNER_SELECTOR: &str = "owner=helm";

/// A release name as Helm allows it, so it is safe in a label selector and
/// as a command-line argument.
pub(crate) fn check_release_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 53
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && name.starts_with(|c: char| c.is_ascii_alphanumeric());
    if !ok {
        bail!("{name:?} is not a valid Helm release name");
    }
    Ok(())
}

/// One stored revision, from an object's metadata.
#[derive(Debug, Clone)]
struct Stored {
    release: String,
    namespace: String,
    storage: HelmStorage,
    rev: HelmRevision,
}

fn stored_from_meta(meta: &ObjectMeta, storage: HelmStorage) -> Option<Stored> {
    let labels = meta.labels.as_ref();
    let label = |k: &str| labels.and_then(|l| l.get(k)).cloned();
    let object_name = meta.name.clone().unwrap_or_default();
    // `sh.helm.release.v1.<release>.v<revision>`, if the labels are missing.
    let from_name = object_name
        .strip_prefix("sh.helm.release.v1.")
        .and_then(|rest| rest.rsplit_once(".v"));
    let release = label("name").or_else(|| from_name.map(|(r, _)| r.to_string()))?;
    let revision = label("version")
        .or_else(|| from_name.map(|(_, v)| v.to_string()))?
        .parse()
        .ok()?;
    let updated = label("modifiedAt")
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| crate::created_secs(&meta.creation_timestamp));
    Some(Stored {
        release,
        namespace: meta.namespace.clone().unwrap_or_default(),
        storage,
        rev: HelmRevision {
            revision,
            status: label("status").unwrap_or_default(),
            updated,
        },
    })
}

/// Group revisions into releases, newest revision first. A release stored
/// by both drivers (rare) is shown from the Secrets.
fn group(stored: Vec<Stored>) -> Vec<HelmReleaseRow> {
    let mut by_release: BTreeMap<(String, String, HelmStorage), Vec<HelmRevision>> =
        BTreeMap::new();
    for s in stored {
        by_release
            .entry((s.namespace, s.release, s.storage))
            .or_default()
            .push(s.rev);
    }
    let mut out: Vec<HelmReleaseRow> = Vec::new();
    for ((namespace, name, storage), mut history) in by_release {
        if out
            .last()
            .is_some_and(|r| r.namespace == namespace && r.name == name)
        {
            continue;
        }
        history.sort_by_key(|h| std::cmp::Reverse(h.revision));
        let newest = history[0].clone();
        out.push(HelmReleaseRow {
            name,
            namespace,
            revision: newest.revision,
            status: newest.status,
            updated: newest.updated,
            storage,
            history,
        });
    }
    out
}

fn scope_api<K>(client: &Client, namespace: Option<&str>) -> Api<K>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope>,
    K::DynamicType: Default,
{
    match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    }
}

/// The revisions stored in one scope (`None` = all namespaces) matching
/// `selector`, metadata only, from both drivers. ConfigMaps the user may
/// not list are skipped quietly: few clusters use that driver.
async fn list_stored(
    client: &Client,
    namespace: Option<&str>,
    selector: &str,
) -> Result<Vec<Stored>> {
    let lp = ListParams::default().labels(selector);
    let secrets = scope_api::<Secret>(client, namespace);
    let configmaps = scope_api::<ConfigMap>(client, namespace);
    let (secrets, configmaps) =
        futures::join!(secrets.list_metadata(&lp), configmaps.list_metadata(&lp));
    let mut out: Vec<Stored> = secrets?
        .items
        .iter()
        .filter_map(|o| stored_from_meta(&o.metadata, HelmStorage::Secret))
        .collect();
    match configmaps {
        Ok(list) => out.extend(
            list.items
                .iter()
                .filter_map(|o| stored_from_meta(&o.metadata, HelmStorage::ConfigMap)),
        ),
        Err(e) => {
            let e = anyhow::Error::from(e);
            if !is_forbidden(&e) {
                return Err(e);
            }
        }
    }
    Ok(out)
}

/// The Helm releases in `namespaces` (empty = all namespaces), from the
/// storage objects' labels only: no release payload is downloaded.
///
/// A namespace whose Secrets the user may not list becomes a warning
/// instead of an error, so the others still show.
pub async fn list_helm_releases(client: &Client, namespaces: &[String]) -> Result<HelmReleaseList> {
    let scopes: Vec<Option<&str>> = if namespaces.is_empty() {
        vec![None]
    } else {
        namespaces.iter().map(|n| Some(n.as_str())).collect()
    };
    let results = futures::future::join_all(
        scopes
            .iter()
            .map(|ns| list_stored(client, *ns, OWNER_SELECTOR)),
    )
    .await;
    let mut stored = Vec::new();
    let mut warnings = Vec::new();
    for (ns, result) in scopes.iter().zip(results) {
        match result {
            Ok(list) => stored.extend(list),
            Err(e) if is_forbidden(&e) => warnings.push(match ns {
                Some(ns) => format!(
                    "Not allowed to list Secrets in namespace {ns}, where Helm keeps its releases: its releases are not shown."
                ),
                None => "Not allowed to list Secrets across all namespaces, where Helm keeps its releases: pick a namespace.".to_string(),
            }),
            Err(e) => return Err(e),
        }
    }
    Ok(HelmReleaseList {
        releases: group(stored),
        warnings,
    })
}

/// The raw `release` payload of one revision.
async fn fetch_payload(client: &Client, key: &HelmRevisionKey) -> Result<Vec<u8>> {
    let object = format!("sh.helm.release.v1.{}.v{}", key.name, key.revision);
    let what = match key.storage {
        HelmStorage::Secret => "Secret",
        HelmStorage::ConfigMap => "ConfigMap",
    };
    let fetched: Result<Option<Vec<u8>>, kube::Error> = match key.storage {
        HelmStorage::Secret => Api::<Secret>::namespaced(client.clone(), &key.namespace)
            .get(&object)
            .await
            .map(|s| s.data.and_then(|mut d| d.remove("release")).map(|b| b.0)),
        HelmStorage::ConfigMap => Api::<ConfigMap>::namespaced(client.clone(), &key.namespace)
            .get(&object)
            .await
            .map(|c| {
                c.data
                    .and_then(|mut d| d.remove("release"))
                    .map(String::into_bytes)
            }),
    };
    match fetched {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => bail!("{what} {}/{object} has no release data", key.namespace),
        Err(e) => {
            let e = anyhow::Error::from(e);
            if is_forbidden(&e) {
                bail!(
                    "Not allowed to read {what}s in namespace {}, where Helm keeps release {}.",
                    key.namespace,
                    key.name
                );
            }
            Err(e.context(format!("cannot read {what} {}/{object}", key.namespace)))
        }
    }
}

/// The chart of each revision, downloading a few payloads at a time.
/// Failures are reported per release in [`HelmChartInfo::error`].
pub async fn release_charts(client: &Client, keys: &[HelmRevisionKey]) -> Vec<HelmChartInfo> {
    stream::iter(keys.iter().cloned())
        .map(|key| async move {
            let decoded = async {
                check_release_name(&key.name)?;
                decode::decode_release(&fetch_payload(client, &key).await?)
            }
            .await;
            let (chart, error) = match decoded {
                Ok(r) => (r.chart.metadata, String::new()),
                Err(e) => (Default::default(), format!("{e:#}")),
            };
            HelmChartInfo {
                namespace: key.namespace,
                name: key.name,
                revision: key.revision,
                chart: chart.name,
                chart_version: chart.version,
                app_version: chart.app_version,
                error,
            }
        })
        .buffered(CHART_CONCURRENCY)
        .collect()
        .await
}

/// One release revision (`None` = the newest), decoded: chart, status,
/// notes, values, manifest (Secret values hidden unless `reveal`) and the
/// list of revisions.
pub async fn get_helm_release(
    client: &Client,
    namespace: &str,
    name: &str,
    revision: Option<u32>,
    reveal: bool,
) -> Result<HelmRelease> {
    check_release_name(name)?;
    let selector = format!("{OWNER_SELECTOR},name={name}");
    let stored = match list_stored(client, Some(namespace), &selector).await {
        Err(e) if is_forbidden(&e) => bail!(
            "Not allowed to list Secrets in namespace {namespace}, where Helm keeps release {name}."
        ),
        other => other?,
    };
    let Some(row) = group(stored).into_iter().next() else {
        bail!("No Helm release named {name} in namespace {namespace}");
    };
    let revision = revision.unwrap_or(row.revision);
    let Some(meta) = row.history.iter().find(|r| r.revision == revision) else {
        let have: Vec<String> = row.history.iter().map(|r| r.revision.to_string()).collect();
        bail!(
            "Release {name} has no revision {revision} (it has {})",
            have.join(", ")
        );
    };
    let key = HelmRevisionKey {
        namespace: namespace.to_string(),
        name: name.to_string(),
        revision,
        storage: row.storage,
    };
    let r = decode::decode_release(&fetch_payload(client, &key).await?)
        .with_context(|| format!("cannot decode revision {revision} of release {name}"))?;
    let (manifest, secrets_hidden) = if reveal {
        (r.manifest.clone(), false)
    } else {
        redact_manifest(&r.manifest)
    };
    let computed = decode::computed_values(r.chart.values.as_ref(), r.config.as_ref());
    Ok(HelmRelease {
        name: name.to_string(),
        namespace: namespace.to_string(),
        revision,
        // The label follows later changes (an upgrade supersedes it); the
        // payload keeps the status it was written with.
        status: if meta.status.is_empty() {
            r.info.status
        } else {
            meta.status.clone()
        },
        storage: row.storage,
        chart: r.chart.metadata.name,
        chart_version: r.chart.metadata.version,
        app_version: r.chart.metadata.app_version,
        description: r.info.description,
        notes: r.info.notes,
        first_deployed: r.info.first_deployed,
        last_deployed: r.info.last_deployed,
        values: decode::values_yaml(r.config.as_ref())?,
        computed_values: decode::values_yaml(Some(&computed))?,
        resources: manifest_resources(&r.manifest, namespace),
        manifest,
        secrets_hidden,
        history: row.history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    fn meta(name: &str, ns: &str, labels: &[(&str, &str)]) -> ObjectMeta {
        ObjectMeta {
            name: Some(name.into()),
            namespace: Some(ns.into()),
            labels: Some(
                labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            creation_timestamp: Some(Time(chrono::DateTime::from_timestamp(100, 0).unwrap())),
            ..Default::default()
        }
    }

    fn stored(name: &str, ns: &str, labels: &[(&str, &str)], storage: HelmStorage) -> Stored {
        stored_from_meta(&meta(name, ns, labels), storage).unwrap()
    }

    #[test]
    fn reads_revisions_from_labels_or_the_object_name() {
        let s = stored(
            "sh.helm.release.v1.web.v3",
            "apps",
            &[
                ("name", "web"),
                ("version", "3"),
                ("status", "deployed"),
                ("modifiedAt", "1700"),
            ],
            HelmStorage::Secret,
        );
        assert_eq!((s.release.as_str(), s.namespace.as_str()), ("web", "apps"));
        assert_eq!(
            s.rev,
            HelmRevision {
                revision: 3,
                status: "deployed".into(),
                updated: 1700
            }
        );
        let s = stored(
            "sh.helm.release.v1.my.app.v12",
            "x",
            &[],
            HelmStorage::ConfigMap,
        );
        assert_eq!((s.release.as_str(), s.rev.revision), ("my.app", 12));
        assert_eq!((s.rev.status.as_str(), s.rev.updated), ("", 100));
        assert!(stored_from_meta(&meta("random", "x", &[]), HelmStorage::Secret).is_none());
    }

    #[test]
    fn groups_by_release_with_the_newest_revision_first() {
        let rev = |ns: &str, name: &str, v: &str, status: &str, storage| {
            stored(
                &format!("sh.helm.release.v1.{name}.v{v}"),
                ns,
                &[("name", name), ("version", v), ("status", status)],
                storage,
            )
        };
        use HelmStorage::*;
        let rows = group(vec![
            rev("b", "web", "1", "superseded", Secret),
            rev("b", "web", "10", "deployed", Secret),
            rev("b", "web", "2", "superseded", Secret),
            rev("a", "db", "1", "failed", Secret),
            rev("b", "web", "1", "deployed", ConfigMap),
            rev("c", "cm", "1", "deployed", ConfigMap),
        ]);
        let short: Vec<_> = rows
            .iter()
            .map(|r| {
                format!(
                    "{}/{} v{} {} {:?}",
                    r.namespace, r.name, r.revision, r.status, r.storage
                )
            })
            .collect();
        assert_eq!(
            short,
            [
                "a/db v1 failed Secret",
                "b/web v10 deployed Secret",
                "c/cm v1 deployed ConfigMap"
            ]
        );
        let revs: Vec<u32> = rows[1].history.iter().map(|h| h.revision).collect();
        assert_eq!(revs, [10, 2, 1]);
    }

    #[test]
    fn release_names_are_checked() {
        for ok in ["web", "my-app.v2", "0x"] {
            assert!(check_release_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-rf", "Web", "a,b=c", "a b", &"x".repeat(54)] {
            assert!(check_release_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn storage_serializes_in_lowercase() {
        assert_eq!(
            serde_json::to_string(&HelmStorage::ConfigMap).unwrap(),
            "\"configmap\""
        );
        assert_eq!(
            serde_json::to_string(&HelmStorage::Secret).unwrap(),
            "\"secret\""
        );
    }
}
