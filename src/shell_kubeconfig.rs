//! Point a local shell at a context and namespace without touching the
//! user's kubeconfig.
//!
//! The shell gets `KUBECONFIG=<temp>:<the user's files>` (`;` on Windows).
//! The temp file only holds one `contexts` entry, with the same cluster and
//! user as the real one but the chosen namespace, and `current-context`.
//! kubectl (and kube-rs) merge kubeconfig files "first file wins" for
//! `current-context` and for entries with the same name, so the temp file
//! picks the context and namespace while clusters, users and credentials
//! still come from the user's files. `kubectl config set-context` or
//! `use-context` in that shell write to the temp file too.

use anyhow::{anyhow, Context as _, Result};
use kube::config::Kubeconfig;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A temp kubeconfig for one shell: which context and namespace it selects,
/// and its YAML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellKubeconfig {
    pub context: String,
    /// The namespace kubectl uses in the shell (`default` if neither the
    /// caller nor the context sets one).
    pub namespace: String,
    pub yaml: String,
}

#[derive(Serialize)]
struct File<'a> {
    #[serde(rename = "apiVersion")]
    api_version: &'static str,
    kind: &'static str,
    contexts: [NamedContext<'a>; 1],
    #[serde(rename = "current-context")]
    current_context: &'a str,
}

#[derive(Serialize)]
struct NamedContext<'a> {
    name: &'a str,
    context: ContextEntry<'a>,
}

#[derive(Serialize)]
struct ContextEntry<'a> {
    cluster: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<&'a str>,
    namespace: &'a str,
}

/// The temp kubeconfig selecting `context` (`None` = the kubeconfig's
/// current-context) with `namespace` (`None` = the context's own, or
/// `default`). `kubeconfig` is the user's merged kubeconfig.
pub fn shell_kubeconfig(
    kubeconfig: &Kubeconfig,
    context: Option<&str>,
    namespace: Option<&str>,
) -> Result<ShellKubeconfig> {
    let name = match context {
        Some(name) => name,
        None => kubeconfig
            .current_context
            .as_deref()
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow!("the kubeconfig has no current context to open a shell in"))?,
    };
    let entry = kubeconfig
        .contexts
        .iter()
        .find(|c| c.name == name)
        .and_then(|c| c.context.as_ref())
        .ok_or_else(|| anyhow!("context {name:?} is not in the kubeconfig"))?;
    let namespace = namespace
        .filter(|ns| !ns.is_empty())
        .or(entry.namespace.as_deref())
        .unwrap_or("default");
    let file = File {
        api_version: "v1",
        kind: "Config",
        contexts: [NamedContext {
            name,
            context: ContextEntry {
                cluster: &entry.cluster,
                user: entry.user.as_deref(),
                namespace,
            },
        }],
        current_context: name,
    };
    Ok(ShellKubeconfig {
        context: name.to_string(),
        namespace: namespace.to_string(),
        yaml: serde_yaml::to_string(&file)?,
    })
}

/// The `KUBECONFIG` value for the shell: `temp`, then the user's
/// `KUBECONFIG` (`existing`) if it names any file, otherwise
/// `default_path` (`~/.kube/config`). `windows` picks `;` as the separator.
pub fn shell_kubeconfig_env(
    temp: &Path,
    existing: Option<&OsStr>,
    default_path: Option<&Path>,
    windows: bool,
) -> OsString {
    let sep = if windows { ";" } else { ":" };
    let mut value = OsString::from(temp);
    let existing = existing.filter(|v| {
        v.to_string_lossy()
            .split(sep)
            .any(|part| !part.trim().is_empty())
    });
    let rest = existing.or(default_path.map(Path::as_os_str));
    if let Some(rest) = rest {
        value.push(sep);
        value.push(rest);
    }
    value
}

/// [`shell_kubeconfig_env`] for this process: its `KUBECONFIG` and home.
pub fn shell_kubeconfig_env_here(temp: &Path) -> OsString {
    let existing = std::env::var_os("KUBECONFIG");
    let default_path = home::home_dir().map(|h| h.join(".kube").join("config"));
    shell_kubeconfig_env(
        temp,
        existing.as_deref(),
        default_path.as_deref(),
        cfg!(windows),
    )
}

static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

/// A temp kubeconfig file, readable only by the user (`0600`), deleted when
/// this is dropped.
#[derive(Debug)]
pub struct TempKubeconfig {
    path: PathBuf,
}

impl TempKubeconfig {
    /// Write `yaml` to a new file in `dir`. Never overwrites a file.
    pub fn create_in(dir: &Path, yaml: &str) -> Result<Self> {
        let pid = std::process::id();
        for _ in 0..100 {
            let n = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("kubeloupe-shell-{pid}-{n}.yaml"));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = match options.open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("cannot create a temp kubeconfig in {}", dir.display())
                    })
                }
            };
            let temp = TempKubeconfig { path };
            file.write_all(yaml.as_bytes())
                .and_then(|()| file.sync_all())
                .with_context(|| format!("cannot write {}", temp.path.display()))?;
            return Ok(temp);
        }
        Err(anyhow!(
            "cannot create a temp kubeconfig in {}: too many files",
            dir.display()
        ))
    }

    /// [`Self::create_in`] the system temp directory.
    pub fn create(yaml: &str) -> Result<Self> {
        Self::create_in(&std::env::temp_dir(), yaml)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempKubeconfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"
apiVersion: v1
kind: Config
current-context: one
clusters:
- name: c1
  cluster: {server: "https://one.example"}
- name: c2
  cluster: {server: "https://two.example"}
users:
- name: u1
  user: {token: secret-one}
- name: u2
  user: {token: secret-two}
contexts:
- name: one
  context: {cluster: c1, user: u1, namespace: orig}
- name: two
  context: {cluster: c2, user: u2}
"#;

    fn real() -> Kubeconfig {
        Kubeconfig::from_yaml(REAL).unwrap()
    }

    #[test]
    fn temp_file_holds_only_the_context() {
        let kc = shell_kubeconfig(&real(), Some("two"), Some("team-a")).unwrap();
        assert_eq!(kc.context, "two");
        assert_eq!(kc.namespace, "team-a");
        assert_eq!(
            kc.yaml,
            "apiVersion: v1\nkind: Config\ncontexts:\n- name: two\n  context:\n    \
             cluster: c2\n    user: u2\n    namespace: team-a\ncurrent-context: two\n"
        );
        assert!(!kc.yaml.contains("secret"));
        assert!(!kc.yaml.contains("server"));
    }

    #[test]
    fn namespace_falls_back_to_the_context_then_default() {
        let kc = shell_kubeconfig(&real(), None, None).unwrap();
        assert_eq!(
            (kc.context.as_str(), kc.namespace.as_str()),
            ("one", "orig")
        );
        let kc = shell_kubeconfig(&real(), Some("two"), Some("")).unwrap();
        assert_eq!(kc.namespace, "default");
    }

    #[test]
    fn unknown_or_missing_context_is_an_error() {
        let err = shell_kubeconfig(&real(), Some("nope"), None).unwrap_err();
        assert_eq!(err.to_string(), "context \"nope\" is not in the kubeconfig");
        let mut kc = real();
        kc.current_context = None;
        assert!(shell_kubeconfig(&kc, None, None).is_err());
    }

    #[test]
    fn names_are_quoted_when_needed() {
        let mut kc = real();
        kc.contexts[1].name = "arn:aws:eks:us-east-1:1:cluster/x: y".to_string();
        let name = kc.contexts[1].name.clone();
        let out = shell_kubeconfig(&kc, Some(&name), Some("ns")).unwrap();
        let parsed = Kubeconfig::from_yaml(&out.yaml).unwrap();
        assert_eq!(parsed.current_context.as_deref(), Some(name.as_str()));
        assert_eq!(parsed.contexts[0].name, name);
    }

    /// The merge the shell relies on, as kube-rs does it (kubectl was
    /// checked by hand with `kubectl config view --minify`).
    #[test]
    fn temp_file_first_wins_the_merge() {
        let out = shell_kubeconfig(&real(), Some("two"), Some("team-a")).unwrap();
        let merged = Kubeconfig::from_yaml(&out.yaml)
            .unwrap()
            .merge(real())
            .unwrap();
        assert_eq!(merged.current_context.as_deref(), Some("two"));
        let two = merged.contexts.iter().find(|c| c.name == "two").unwrap();
        assert_eq!(
            two.context.as_ref().unwrap().namespace.as_deref(),
            Some("team-a")
        );
        // Clusters and users still come from the real file.
        assert_eq!(merged.clusters.len(), 2);
        assert_eq!(merged.auth_infos.len(), 2);
        assert_eq!(merged.contexts.len(), 2);
    }

    #[test]
    fn env_value_on_unix() {
        let temp = Path::new("/tmp/k.yaml");
        let home = Path::new("/home/u/.kube/config");
        let v = |existing: Option<&str>| {
            shell_kubeconfig_env(temp, existing.map(OsStr::new), Some(home), false)
        };
        assert_eq!(v(None), "/tmp/k.yaml:/home/u/.kube/config");
        assert_eq!(v(Some("")), "/tmp/k.yaml:/home/u/.kube/config");
        assert_eq!(v(Some(":")), "/tmp/k.yaml:/home/u/.kube/config");
        assert_eq!(v(Some("/a:/b")), "/tmp/k.yaml:/a:/b");
        assert_eq!(shell_kubeconfig_env(temp, None, None, false), "/tmp/k.yaml");
    }

    #[test]
    fn env_value_on_windows() {
        let temp = Path::new(r"C:\Temp\k.yaml");
        let home = Path::new(r"C:\Users\u\.kube\config");
        let v = |existing: Option<&str>| {
            shell_kubeconfig_env(temp, existing.map(OsStr::new), Some(home), true)
        };
        assert_eq!(v(None), r"C:\Temp\k.yaml;C:\Users\u\.kube\config");
        assert_eq!(v(Some(r"C:\a;D:\b")), r"C:\Temp\k.yaml;C:\a;D:\b");
        assert_eq!(v(Some(";")), r"C:\Temp\k.yaml;C:\Users\u\.kube\config");
    }

    #[test]
    fn temp_file_is_private_and_removed_on_drop() {
        let dir = std::env::temp_dir().join(format!("kubeloupe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let temp = TempKubeconfig::create_in(&dir, "kind: Config\n").unwrap();
        let path = temp.path().to_path_buf();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kind: Config\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let other = TempKubeconfig::create_in(&dir, "x").unwrap();
        assert_ne!(other.path(), path);
        drop(temp);
        assert!(!path.exists());
        drop(other);
        std::fs::remove_dir(&dir).unwrap();
    }
}
