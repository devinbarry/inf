use crate::models::{IdentityMembership, Project};
use colored::Colorize;
use serde::Serialize;

/// Render an aligned table. Columns are sized to their widest cell so output
/// stays readable when names vary wildly in length, and the trailing column is
/// never padded so copy-paste does not pick up stray whitespace.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("{}", "(none)".dimmed());
        return;
    }

    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| pad(h, widths[i], i == headers.len() - 1))
        .collect();
    println!("{}", header_line.join("  ").bold());

    for row in rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i], i == row.len() - 1))
            .collect();
        println!("{}", line.join("  "));
    }
}

fn pad(value: &str, width: usize, is_last: bool) -> String {
    if is_last {
        value.to_string()
    } else {
        format!("{value:<width$}")
    }
}

pub fn json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("{} {e}", "error serializing output:".red()),
    }
}

pub fn json_raw(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
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

pub fn success(message: &str) {
    println!("{} {}", "✓".green().bold(), message);
}

pub fn warn(message: &str) {
    eprintln!("{} {}", "!".yellow().bold(), message);
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
    fn pad_fills_interior_columns_only() {
        assert_eq!(pad("ab", 5, false), "ab   ");
        // The final column is left unpadded so copied output has no trailing
        // spaces.
        assert_eq!(pad("ab", 5, true), "ab");
    }
}
