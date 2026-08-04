use crate::models::{IdentityMembership, Project};
use colored::Colorize;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

/// How results are rendered.
///
/// `inf` is an agent-facing tool: its caller is normally a script or an LLM
/// agent piping stdout into `jq`, not a person reading a terminal. So JSON is
/// the default and the only thing ever written to stdout in that mode — no
/// tables, no status lines, no colour. `--human` is the explicit opt-in for
/// interactive use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Json,
    Human,
}

impl Format {
    pub fn new(human: bool) -> Self {
        if human {
            Format::Human
        } else {
            Format::Json
        }
    }

    pub fn is_human(self) -> bool {
        self == Format::Human
    }
}

/// Write the result value to stdout as JSON. Pretty-printed: `jq` is
/// indifferent and a human debugging a pipeline is not.
pub fn json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        // stdout must stay parseable, so a serialisation failure is reported on
        // stderr in the same shape as every other error.
        Err(e) => eprintln!(
            "{}",
            serde_json::json!({
                "error": "serialization_failed",
                "message": e.to_string(),
            })
        ),
    }
}

/// Confirmation of a mutation, in whichever shape the caller asked for. The
/// JSON branch is what makes `--json`-less commands like `delete` scriptable.
pub fn emit<T: Serialize>(format: Format, value: &T, human: impl FnOnce()) {
    if format.is_human() {
        human();
    } else {
        json(value);
    }
}

/// Render an aligned table. Columns are sized to their widest cell so output
/// stays readable when names vary wildly in length, and the trailing column is
/// never padded so copy-paste does not pick up stray whitespace.
///
/// Widths are *display* widths. Byte length inflates a column for every
/// non-ASCII cell, and character count still breaks on double-width CJK and
/// emoji — both of which appear in real project names.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("{}", "(none)".dimmed());
        return;
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| h.width()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.width());
            }
        }
    }

    println!("{}", render_row(headers.iter().copied(), &widths).bold());
    for row in rows {
        println!("{}", render_row(row.iter().map(String::as_str), &widths));
    }
}

/// Cells beyond the header count have no column to size against, so they are
/// emitted unpadded rather than panicking on `widths[i]`.
fn render_row<'a>(cells: impl Iterator<Item = &'a str>, widths: &[usize]) -> String {
    let last = widths.len().saturating_sub(1);
    cells
        .enumerate()
        .map(|(i, cell)| match widths.get(i) {
            Some(width) if i != last => pad(cell, *width),
            _ => cell.to_string(),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn pad(value: &str, width: usize) -> String {
    let mut padded = String::from(value);
    padded.extend(std::iter::repeat_n(
        ' ',
        width.saturating_sub(value.width()),
    ));
    padded
}

pub fn projects_table(projects: &[Project]) {
    let rows: Vec<Vec<String>> = projects
        .iter()
        .map(|p| {
            vec![
                p.name.clone(),
                p.slug.clone(),
                p.project_type.clone().unwrap_or_else(|| "-".into()),
                envs_summary(p),
                p.id.clone(),
            ]
        })
        .collect();
    table(&["NAME", "SLUG", "TYPE", "ENVS", "ID"], &rows);
}

fn envs_summary(project: &Project) -> String {
    if project.environments.is_empty() {
        "-".to_string()
    } else {
        project
            .environments
            .iter()
            .map(|e| e.slug.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Organization listings populate `authMethods`, so show it.
pub fn identities_table(identities: &[IdentityMembership]) {
    let rows: Vec<Vec<String>> = identities
        .iter()
        .map(|i| {
            vec![
                i.name().to_string(),
                i.role(),
                i.auth_methods(),
                i.id().to_string(),
            ]
        })
        .collect();
    table(&["NAME", "ROLE", "AUTH", "ID"], &rows);
}

/// Project membership listings return `authMethods: []` even for identities
/// that have one attached, so the column is omitted rather than printed as a
/// confident "none" the API never actually asserted. `inf identity get` reports
/// the real auth configuration.
pub fn project_identities_table(identities: &[IdentityMembership]) {
    let rows: Vec<Vec<String>> = identities
        .iter()
        .map(|i| vec![i.name().to_string(), i.role(), i.id().to_string()])
        .collect();
    table(&["IDENTITY", "ROLE", "ID"], &rows);
}

pub fn project_detail(project: &Project) {
    println!("{} {}", "Name:".bold(), project.name);
    println!("{} {}", "Slug:".bold(), project.slug);
    println!("{} {}", "ID:".bold(), project.id);
    if let Some(project_type) = &project.project_type {
        println!("{} {}", "Type:".bold(), project_type);
    }
    if let Some(description) = &project.description {
        if !description.is_empty() {
            println!("{} {}", "Description:".bold(), description);
        }
    }
    // Delete protection changes what `inf project delete` can do, so it is
    // worth stating rather than leaving the user to discover it on failure.
    if project.has_delete_protection {
        println!("{} {}", "Delete protection:".bold(), "enabled".yellow());
    }
    if let Some(created) = &project.created_at {
        println!("{} {}", "Created:".bold(), created);
    }

    println!("\n{}", "Environments".bold());
    let rows: Vec<Vec<String>> = project
        .environments
        .iter()
        .map(|e| vec![e.name.clone(), e.slug.clone(), e.id.clone()])
        .collect();
    table(&["NAME", "SLUG", "ID"], &rows);
}

/// Human-mode only. In JSON mode the result object on stdout already says what
/// happened, and a decorative line would be noise the caller has to strip.
pub fn success(format: Format, message: &str) {
    if format.is_human() {
        println!("{} {}", "✓".green().bold(), message);
    }
}

pub fn warn(format: Format, message: &str) {
    if format.is_human() {
        eprintln!("{} {}", "!".yellow().bold(), message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_fixture(envs: serde_json::Value) -> Project {
        serde_json::from_value(serde_json::json!({
            "id": "p1", "name": "billing", "slug": "billing", "environments": envs
        }))
        .unwrap()
    }

    #[test]
    fn envs_summary_joins_slugs() {
        let project = project_fixture(serde_json::json!([
            {"id": "e1", "name": "Dev", "slug": "dev"},
            {"id": "e2", "name": "Prod", "slug": "prod"}
        ]));
        assert_eq!(envs_summary(&project), "dev,prod");
    }

    #[test]
    fn envs_summary_handles_no_environments() {
        let project = project_fixture(serde_json::json!([]));
        assert_eq!(envs_summary(&project), "-");
    }

    #[test]
    fn pad_fills_to_display_width() {
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("ab", 2), "ab");
        // Never truncates, and never underflows on an over-wide cell.
        assert_eq!(pad("abcdef", 3), "abcdef");
    }

    #[test]
    fn pad_counts_display_width_not_bytes() {
        // "Café" is 5 bytes but 4 columns; padding by bytes left the column one
        // space short of the others.
        assert_eq!(pad("Café", 6).width(), 6);
        // CJK is two columns per character.
        assert_eq!(pad("生産", 6).width(), 6);
        assert_eq!(pad("生産", 6), "生産  ");
    }

    #[test]
    fn rows_align_across_mixed_scripts() {
        let widths = vec![12, 4];
        let ascii = render_row(["alpha", "a"].into_iter(), &widths);
        let accented = render_row(["Café Billing", "b"].into_iter(), &widths);
        let cjk = render_row(["生産システム", "c"].into_iter(), &widths);
        // Every first column occupies the same number of terminal columns, so
        // the second column starts at the same offset on every row.
        for line in [&ascii, &accented, &cjk] {
            let first = line.rsplit_once("  ").unwrap().0;
            assert_eq!(first.width(), 12, "misaligned: {line:?}");
        }
    }

    #[test]
    fn last_column_is_never_padded() {
        // Trailing whitespace would be picked up by copy-paste and by `cut`.
        let row = render_row(["ab", "cd"].into_iter(), &[5, 5]);
        assert_eq!(row, "ab     cd");
        assert!(!row.ends_with(' '));
    }

    #[test]
    fn extra_cells_do_not_panic() {
        // Sizing skipped cells beyond the header count while rendering indexed
        // them unguarded, so a mismatched row panicked instead of degrading.
        let row = render_row(["a", "b", "surplus"].into_iter(), &[3, 3]);
        assert!(row.contains("surplus"));
    }

    #[test]
    fn short_rows_pad_against_the_header_columns() {
        // Padding decided by `row.len()` suppressed it on the wrong column when
        // a row was shorter than the header.
        let row = render_row(["a"].into_iter(), &[4, 4]);
        assert_eq!(row, "a   ");
    }

    #[test]
    fn json_is_the_default_format() {
        assert!(!Format::new(false).is_human());
        assert!(Format::new(true).is_human());
    }
}
