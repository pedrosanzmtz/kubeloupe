//! `kubectl get`-style table rendering for the CLI.

use super::*;
use crate::age_of_secs;

/// Render rows like `kubectl get`: aligned columns, age computed here, and
/// without the columns that only the desktop UI can fill (live metrics).
pub fn render_table(kind: &ResourceKind, rows: &[ResourceRow]) -> String {
    let mut sorted: Vec<&ResourceRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    let mut table = vec![table_header(kind)];
    table.extend(sorted.into_iter().map(|row| table_cells(kind, row)));
    align_columns(&table)
}

/// Indexes of the columns the CLI shows: all but live metrics.
fn shown_columns(kind: &ResourceKind) -> impl Iterator<Item = usize> + '_ {
    kind.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            !matches!(
                c.kind,
                ColumnKind::PodCpu
                    | ColumnKind::PodMemory
                    | ColumnKind::NodeCpu
                    | ColumnKind::NodeMemory
            )
        })
        .map(|(i, _)| i)
}

/// The upper-case header of [`render_table`].
pub fn table_header(kind: &ResourceKind) -> Vec<String> {
    shown_columns(kind)
        .map(|i| kind.columns[i].label.to_uppercase())
        .collect()
}

/// One row's cells as [`render_table`] prints them (age computed now).
pub fn table_cells(kind: &ResourceKind, row: &ResourceRow) -> Vec<String> {
    shown_columns(kind)
        .map(|i| match kind.columns[i].kind {
            ColumnKind::Age => age_of_secs(row.created),
            _ => row.cells.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}

/// Left-align every column of `table` (first row is the header), two spaces
/// apart, without trailing whitespace.
pub fn align_columns(table: &[Vec<String>]) -> String {
    let cols = table.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            table
                .iter()
                .filter_map(|r| r.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for r in table {
        let line: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    out
}
#[cfg(test)]
mod tests {
    use super::super::columns::columns_for;
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn render_table_aligns_and_skips_metrics_columns() {
        let mut k = kind("", "Pod", "pods", &[]);
        k.columns = columns_for(&k.resource, &[]);
        let row = |name: &str, status: &str| ResourceRow {
            key: format!("ns/{name}"),
            name: name.into(),
            namespace: "ns".into(),
            created: 0,
            cells: vec![
                name.into(),
                "ns".into(),
                "1/1".into(),
                status.into(),
                "0".into(),
                "node".into(),
                "".into(),
                "".into(),
                "".into(),
            ],
        };
        let out = render_table(&k, &[row("b", "Running"), row("a", "Pending")]);
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(
            lines[0],
            "NAME  NAMESPACE  READY  STATUS   RESTARTS  NODE  AGE"
        );
        assert!(lines[1].starts_with("a     ns"), "{out}");
        assert!(lines[2].starts_with("b     ns"), "{out}");
        assert!(!out.contains("CPU"));
    }
}
