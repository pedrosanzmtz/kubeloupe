//! CLI output: `kubeloupe helm list` and `kubeloupe helm get`.

use super::{
    get_helm_release, list_helm_releases, release_charts, HelmRelease, HelmReleaseRow,
    HelmRevision, HelmRevisionKey,
};
use crate::resources::align_columns;
use anyhow::Result;
use kube::Client;

/// What `kubeloupe helm get` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum HelmSection {
    /// Status, chart, history and notes
    #[default]
    All,
    /// The user-supplied values (with --all, merged with the chart defaults)
    Values,
    /// The rendered manifest (Secret values hidden unless --reveal)
    Manifest,
    /// The chart's notes
    Notes,
    /// The stored revisions
    History,
}

fn chart_label(name: &str, version: &str) -> String {
    match (name.is_empty(), version.is_empty()) {
        (true, _) => String::new(),
        (false, true) => name.to_string(),
        (false, false) => format!("{name}-{version}"),
    }
}

fn list_table(rows: &[HelmReleaseRow], charts: &[(String, String)]) -> String {
    let mut table = vec![[
        "NAME",
        "NAMESPACE",
        "REVISION",
        "STATUS",
        "CHART",
        "APP VERSION",
        "UPDATED",
    ]
    .map(String::from)
    .to_vec()];
    for (r, (chart, app)) in rows.iter().zip(charts) {
        table.push(vec![
            r.name.clone(),
            r.namespace.clone(),
            r.revision.to_string(),
            r.status.clone(),
            chart.clone(),
            app.clone(),
            crate::age_of_secs(r.updated),
        ]);
    }
    align_columns(&table)
}

fn history_table(history: &[HelmRevision]) -> String {
    let mut table = vec![vec![
        "REVISION".to_string(),
        "STATUS".to_string(),
        "UPDATED".to_string(),
    ]];
    for h in history.iter().rev() {
        table.push(vec![
            h.revision.to_string(),
            h.status.clone(),
            crate::age_of_secs(h.updated),
        ]);
    }
    align_columns(&table)
}

/// `kubeloupe helm list [-n ns]`: all namespaces by default.
pub async fn print_helm_list(client: &Client, namespace: Option<&str>) -> Result<()> {
    let namespaces: Vec<String> = namespace.map(str::to_string).into_iter().collect();
    let list = list_helm_releases(client, &namespaces).await?;
    for w in &list.warnings {
        eprintln!("warning: {w}");
    }
    let keys: Vec<HelmRevisionKey> = list
        .releases
        .iter()
        .map(|r| HelmRevisionKey {
            namespace: r.namespace.clone(),
            name: r.name.clone(),
            revision: r.revision,
            storage: r.storage,
        })
        .collect();
    let charts: Vec<(String, String)> = release_charts(client, &keys)
        .await
        .into_iter()
        .map(|c| {
            if !c.error.is_empty() {
                eprintln!("warning: {}/{}: {}", c.namespace, c.name, c.error);
            }
            (chart_label(&c.chart, &c.chart_version), c.app_version)
        })
        .collect();
    if list.releases.is_empty() {
        eprintln!("No Helm releases found.");
        return Ok(());
    }
    print!("{}", list_table(&list.releases, &charts));
    Ok(())
}

fn summary(r: &HelmRelease) -> String {
    let mut out = String::new();
    for (k, v) in [
        ("NAME", r.name.clone()),
        ("NAMESPACE", r.namespace.clone()),
        ("REVISION", r.revision.to_string()),
        ("STATUS", r.status.clone()),
        ("CHART", chart_label(&r.chart, &r.chart_version)),
        ("APP VERSION", r.app_version.clone()),
        ("FIRST DEPLOYED", r.first_deployed.clone()),
        ("LAST DEPLOYED", r.last_deployed.clone()),
        ("DESCRIPTION", r.description.clone()),
    ] {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push_str("\nHISTORY:\n");
    out.push_str(&history_table(&r.history));
    if !r.notes.trim().is_empty() {
        out.push_str("\nNOTES:\n");
        out.push_str(r.notes.trim_end());
        out.push('\n');
    }
    out
}

/// What `kubeloupe helm get` takes besides the client.
#[derive(Debug, Clone, Default)]
pub struct HelmGetOptions {
    pub namespace: String,
    pub revision: Option<u32>,
    pub section: HelmSection,
    /// Values: the chart defaults merged in.
    pub all_values: bool,
    /// Manifest: show Secret values.
    pub reveal: bool,
}

/// `kubeloupe helm get <release> [section]`.
pub async fn print_helm_get(client: &Client, name: &str, opts: &HelmGetOptions) -> Result<()> {
    let r = get_helm_release(client, &opts.namespace, name, opts.revision, opts.reveal).await?;
    match opts.section {
        HelmSection::All => print!("{}", summary(&r)),
        HelmSection::Values => {
            let values = if opts.all_values {
                &r.computed_values
            } else {
                &r.values
            };
            if values.is_empty() {
                eprintln!("No user-supplied values (use --all to include the chart defaults).");
            }
            print!("{values}");
        }
        HelmSection::Manifest => {
            print!("{}", r.manifest);
            if r.secrets_hidden {
                eprintln!("Secret values are hidden: add --reveal to show them.");
            }
        }
        HelmSection::Notes => print!("{}", r.notes),
        HelmSection::History => print!("{}", history_table(&r.history)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helm::HelmStorage;

    #[test]
    fn tables() {
        let rows = vec![HelmReleaseRow {
            name: "web".into(),
            namespace: "apps".into(),
            revision: 2,
            status: "deployed".into(),
            updated: 0,
            storage: HelmStorage::Secret,
            history: vec![],
        }];
        let t = list_table(&rows, &[("demo-0.1.0".into(), "1.0".into())]);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("NAME") && lines[0].contains("APP VERSION"));
        assert!(lines[1].starts_with("web") && lines[1].contains("demo-0.1.0"));
        let h = history_table(&[
            HelmRevision {
                revision: 2,
                status: "deployed".into(),
                updated: 0,
            },
            HelmRevision {
                revision: 1,
                status: "superseded".into(),
                updated: 0,
            },
        ]);
        let lines: Vec<&str> = h.lines().collect();
        assert!(
            lines[1].starts_with("1 ") && lines[2].starts_with("2 "),
            "{h}"
        );
        assert_eq!(chart_label("demo", ""), "demo");
        assert_eq!(chart_label("", "1"), "");
    }
}
