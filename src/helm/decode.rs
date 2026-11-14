//! Decoding one stored Helm 3 release revision.
//!
//! The `release` key of a `helm.sh/release.v1` Secret (or of a ConfigMap,
//! with `HELM_DRIVER=configmap`) holds base64 text of a gzipped JSON
//! document. For a Secret, the API's own base64 layer is already removed by
//! the time the bytes reach us.

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::io::Read;

/// Largest decompressed release we accept. Real releases are kilobytes to a
/// few megabytes; this only stops a corrupt or hostile payload.
const MAX_RELEASE_BYTES: u64 = 64 << 20;

const GZIP_MAGIC: [u8; 3] = [0x1f, 0x8b, 0x08];

/// The parts of Helm's release JSON that kubeloupe shows.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StoredRelease {
    pub name: String,
    pub namespace: String,
    pub version: u32,
    pub info: StoredInfo,
    pub chart: StoredChart,
    /// The values the user supplied (`--set`, `-f`), not the chart defaults.
    pub config: Option<Value>,
    pub manifest: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StoredInfo {
    pub first_deployed: String,
    pub last_deployed: String,
    pub description: String,
    pub status: String,
    pub notes: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StoredChart {
    pub metadata: StoredChartMeta,
    /// The chart's default values.
    pub values: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct StoredChartMeta {
    pub name: String,
    pub version: String,
    pub app_version: String,
}

/// Decode the `release` payload: base64, then gzip (Helm also reads
/// uncompressed JSON, from old releases), then JSON.
pub(crate) fn decode_release(payload: &[u8]) -> Result<StoredRelease> {
    let text: Vec<u8> = payload
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&text)
        .context("the release data is not valid base64")?;
    let json = if raw.starts_with(&GZIP_MAGIC) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .take(MAX_RELEASE_BYTES + 1)
            .read_to_end(&mut out)
            .context("the release data is not valid gzip")?;
        if out.len() as u64 > MAX_RELEASE_BYTES {
            bail!(
                "the release data is larger than {} MiB",
                MAX_RELEASE_BYTES >> 20
            );
        }
        out
    } else {
        raw
    };
    serde_json::from_slice(&json).context("the release data is not valid Helm release JSON")
}

/// A values object as YAML; empty when there are none.
pub(crate) fn values_yaml(values: Option<&Value>) -> Result<String> {
    match values {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::Object(m)) if m.is_empty() => Ok(String::new()),
        Some(v) => Ok(serde_yaml::to_string(v)?),
    }
}

/// The values a release was rendered with: the chart defaults with the
/// user's values on top, like `helm get values --all`. Maps merge key by
/// key; any other user value replaces the default, and a user `null`
/// removes the key.
pub(crate) fn computed_values(defaults: Option<&Value>, user: Option<&Value>) -> Value {
    let mut out = defaults.cloned().unwrap_or(Value::Object(Map::new()));
    if let Some(user) = user {
        merge(&mut out, user);
    }
    out
}

fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                if v.is_null() {
                    b.remove(k);
                } else if let Some(existing) = b.get_mut(k) {
                    merge(existing, v);
                } else {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    /// A real Helm 3.16 release (`demo-chart` revision 2, `--set
    /// greeting=bye`), as the Secret's `data.release` after the API's
    /// base64.
    pub(crate) const FIXTURE: &str = include_str!("testdata/release-fx-v2.b64");

    /// Encode a release JSON the way Helm stores it.
    pub(crate) fn encode(json: &Value) -> String {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(json.to_string().as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(gz.finish().unwrap())
    }

    #[test]
    fn decodes_a_real_release() {
        let r = decode_release(FIXTURE.as_bytes()).unwrap();
        assert_eq!((r.name.as_str(), r.version), ("fx", 2));
        assert_eq!(r.namespace, "kl-helm-probe");
        assert_eq!(r.info.status, "deployed");
        assert_eq!(r.info.description, "Upgrade complete");
        assert_eq!(r.info.notes, "fx says bye.\n");
        assert_eq!(r.chart.metadata.name, "demo-chart");
        assert_eq!(r.chart.metadata.version, "0.1.0");
        assert_eq!(r.chart.metadata.app_version, "1.36");
        assert_eq!(r.config, Some(json!({"greeting": "bye"})));
        assert!(r.manifest.contains("kind: Deployment"));
        assert_eq!(values_yaml(r.config.as_ref()).unwrap(), "greeting: bye\n");
    }

    #[test]
    fn accepts_whitespace_and_uncompressed_json() {
        let mut wrapped = String::new();
        for chunk in FIXTURE.trim().as_bytes().chunks(76) {
            wrapped.push_str(std::str::from_utf8(chunk).unwrap());
            wrapped.push('\n');
        }
        assert_eq!(decode_release(wrapped.as_bytes()).unwrap().version, 2);
        let plain = base64::engine::general_purpose::STANDARD
            .encode(json!({"name": "a", "version": 3}).to_string());
        let r = decode_release(plain.as_bytes()).unwrap();
        assert_eq!((r.name.as_str(), r.version), ("a", 3));
        assert_eq!(r.info.status, "");
    }

    #[test]
    fn rejects_garbage_with_a_reason() {
        let e = decode_release(b"not base64!").unwrap_err();
        assert!(e.to_string().contains("base64"), "{e}");
        let bad_gzip = base64::engine::general_purpose::STANDARD.encode([0x1f, 0x8b, 0x08, 0, 1]);
        let e = decode_release(bad_gzip.as_bytes()).unwrap_err();
        assert!(e.to_string().contains("gzip"), "{e}");
        let e = decode_release(encode(&json!([1, 2])).as_bytes()).unwrap_err();
        assert!(e.to_string().contains("JSON"), "{e}");
    }

    #[test]
    fn empty_values_are_empty_text() {
        assert_eq!(values_yaml(None).unwrap(), "");
        assert_eq!(values_yaml(Some(&json!({}))).unwrap(), "");
        assert_eq!(values_yaml(Some(&Value::Null)).unwrap(), "");
    }

    #[test]
    fn computed_values_merge_like_helm() {
        let defaults = json!({"a": 1, "nested": {"x": 1, "y": 2}, "list": [1, 2], "gone": true});
        let user = json!({"a": 5, "nested": {"y": 3, "z": 4}, "list": [9], "gone": null});
        assert_eq!(
            computed_values(Some(&defaults), Some(&user)),
            json!({"a": 5, "nested": {"x": 1, "y": 3, "z": 4}, "list": [9]})
        );
        assert_eq!(computed_values(None, None), json!({}));
        assert_eq!(
            computed_values(None, Some(&json!({"a": 1}))),
            json!({"a": 1})
        );
    }
}
