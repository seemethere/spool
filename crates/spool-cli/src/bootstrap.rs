use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BootstrapFrontMatter {
    /// Optional same-batch reference key for dependency-aware multi-file creation.
    #[serde(default, alias = "local_id", alias = "batch_id")]
    batch_key: Option<String>,
    title: String,
    priority: Option<String>,
    state: Option<String>,
    acceptance_criteria: Option<Vec<String>>,
    validation_items: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    #[serde(default, alias = "anticipated_touched_files", alias = "touched_files")]
    conflict_hints: Option<Vec<String>>,
    #[serde(default, alias = "blocking_tasks", alias = "blockers")]
    blocking_task_identifiers: Option<Vec<String>>,
    #[serde(default, alias = "blocking_batch_keys", alias = "blocking_local_ids")]
    blocking_task_keys: Option<Vec<String>>,
    review_required: Option<bool>,
}

#[cfg(test)]
pub fn parse_bootstrap_task_file(queue_key: &str, file: &Path) -> Result<spool_db::CreateTask> {
    Ok(parse_bootstrap_task_file_with_warnings(queue_key, file)?.task)
}

pub fn lint_bootstrap_task_file(file: &Path) -> Result<spool_db::CreateTask> {
    let parsed = parse_bootstrap_task_file_with_warnings("__lint__", file)?;
    reject_batch_only_fields(&parsed, "task lint --file")?;
    let input = parsed.task;
    spool_db::validate_create_task(&input)?;
    Ok(input)
}

pub fn parse_bootstrap_task_file_with_warnings(
    queue_key: &str,
    file: &Path,
) -> Result<ParsedBootstrapTask> {
    let text =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    parse_bootstrap_task_with_warnings(queue_key, &file.display().to_string(), &text)
}

#[cfg(test)]
pub fn parse_bootstrap_task(
    queue_key: &str,
    source_name: &str,
    text: &str,
) -> Result<spool_db::CreateTask> {
    Ok(parse_bootstrap_task_with_warnings(queue_key, source_name, text)?.task)
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedBootstrapTask {
    pub task: spool_db::CreateTask,
    pub batch_key: Option<String>,
    pub blocking_task_keys: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn parse_bootstrap_task_with_warnings(
    queue_key: &str,
    source_name: &str,
    text: &str,
) -> Result<ParsedBootstrapTask> {
    let (front_matter, brief) = split_front_matter(text)?;
    let front_matter_text = front_matter;
    let front_matter: BootstrapFrontMatter =
        serde_yaml::from_str(front_matter_text).map_err(|error| {
            anyhow::anyhow!("failed to parse YAML front matter in {source_name}: {error}")
        })?;

    let priority = validate_enum_front_matter_field(
        source_name,
        front_matter_text,
        "priority",
        front_matter.priority.as_deref().unwrap_or("normal"),
        &["urgent", "high", "normal", "low"],
        &[("medium", "normal")],
    )?;
    let state = validate_enum_front_matter_field(
        source_name,
        front_matter_text,
        "state",
        front_matter.state.as_deref().unwrap_or("ready"),
        &["backlog", "ready"],
        &[],
    )?;

    let mut warnings = Vec::new();
    if let Some(alias) = &priority.alias {
        warnings.push(format!(
            "normalized priority alias \"{}\" to canonical \"{}\"",
            alias, priority.value
        ));
    }

    Ok(ParsedBootstrapTask {
        task: spool_db::CreateTask {
            queue_key: queue_key.to_string(),
            title: front_matter.title,
            brief: brief.trim().to_string(),
            priority: priority.value,
            state: state.value,
            review_required: front_matter.review_required.unwrap_or(false),
            acceptance_criteria: front_matter.acceptance_criteria.unwrap_or_default(),
            validation_items: front_matter.validation_items.unwrap_or_default(),
            tags: front_matter.tags.unwrap_or_default(),
            conflict_hints: front_matter.conflict_hints.unwrap_or_default(),
            blocking_task_identifiers: front_matter.blocking_task_identifiers.unwrap_or_default(),
        },
        batch_key: front_matter.batch_key,
        blocking_task_keys: front_matter.blocking_task_keys.unwrap_or_default(),
        warnings,
    })
}

pub fn reject_batch_only_fields(parsed: &ParsedBootstrapTask, command: &str) -> Result<()> {
    if parsed.batch_key.is_some() || !parsed.blocking_task_keys.is_empty() {
        anyhow::bail!(
            "{command} does not accept batch-only fields batch_key or blocking_task_keys; use task batch lint/create --from-file for dependency-aware multi-file Task Creation"
        );
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedEnumField {
    value: String,
    alias: Option<String>,
}

fn validate_enum_front_matter_field(
    source_name: &str,
    front_matter: &str,
    field: &str,
    raw_value: &str,
    allowed_values: &[&str],
    aliases: &[(&str, &str)],
) -> Result<NormalizedEnumField> {
    let normalized = normalize_label(raw_value);
    if allowed_values.contains(&normalized.as_str()) {
        return Ok(NormalizedEnumField {
            value: normalized,
            alias: None,
        });
    }

    for (alias, canonical) in aliases {
        if normalized == *alias {
            return Ok(NormalizedEnumField {
                value: (*canonical).to_string(),
                alias: Some(normalized),
            });
        }
    }

    let line = front_matter_field_line(front_matter, field)
        .map(|line| format!(":{line}"))
        .unwrap_or_default();
    let allowed = allowed_values.join(", ");
    let rejected = raw_value.trim();
    let mut message =
        format!("{source_name}{line}: invalid {field} \"{rejected}\"; expected one of: {allowed}");
    if !aliases.is_empty() {
        let alias_list = aliases
            .iter()
            .map(|(alias, canonical)| format!("{alias} -> {canonical}"))
            .collect::<Vec<_>>()
            .join(", ");
        message.push_str(&format!("; accepted aliases: {alias_list}"));
        for (alias, canonical) in aliases {
            if normalized == *alias {
                message.push_str(&format!(
                    "\nhint: use \"{canonical}\" instead of \"{alias}\""
                ));
            }
        }
    }
    anyhow::bail!(message)
}

fn front_matter_field_line(front_matter: &str, field: &str) -> Option<usize> {
    front_matter.lines().enumerate().find_map(|(index, line)| {
        line.split_once(':')
            .map(|(key, _)| key.trim())
            .filter(|key| *key == field)
            .map(|_| index + 2)
    })
}

fn split_front_matter(text: &str) -> Result<(&str, &str)> {
    let Some(after_start) = text.strip_prefix("---\n") else {
        anyhow::bail!(
            "file-backed task definition must start with YAML front matter delimited by ---"
        );
    };
    let Some((front_matter, body)) = after_start.split_once("\n---\n") else {
        anyhow::bail!("file-backed task definition must close YAML front matter with ---");
    };
    Ok((front_matter, body))
}

pub fn normalize_label(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_defaults_to_ready_normal() {
        let parsed = parse_bootstrap_task(
            "TASK",
            "inline",
            "---\ntitle: Test\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect("parse");

        assert_eq!(parsed.queue_key, "TASK");
        assert_eq!(parsed.priority, "normal");
        assert_eq!(parsed.state, "ready");
        assert_eq!(parsed.brief, "Brief");
    }

    #[test]
    fn parser_reads_blocking_task_identifier_aliases() {
        let parsed = parse_bootstrap_task(
            "TASK",
            "inline",
            "---\ntitle: Test\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\nblockers:\n  - TASK-1\n  - TASK-2\n---\nBrief\n",
        )
        .expect("parse");

        assert_eq!(
            parsed.blocking_task_identifiers,
            vec!["TASK-1".to_string(), "TASK-2".to_string()]
        );
    }

    #[test]
    fn parser_reads_conflict_hints_aliases() {
        let parsed = parse_bootstrap_task(
            "TASK",
            "inline",
            "---\ntitle: Test\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\nanticipated_touched_files:\n  - AGENTS.md\n  - docs/PRE_DOGFOODING_LOOP.md\n---\nBrief\n",
        )
        .expect("parse");

        assert_eq!(
            parsed.conflict_hints,
            vec![
                "AGENTS.md".to_string(),
                "docs/PRE_DOGFOODING_LOOP.md".to_string()
            ]
        );
    }

    #[test]
    fn parser_normalizes_priority_and_state_labels() {
        let parsed = parse_bootstrap_task(
            "TASK",
            "inline",
            "---\ntitle: Test\npriority: High\nstate: Backlog\n---\nBrief\n",
        )
        .expect("parse");

        assert_eq!(parsed.priority, "high");
        assert_eq!(parsed.state, "backlog");
    }

    #[test]
    fn canonical_bootstrap_template_parses() {
        let parsed = parse_bootstrap_task(
            "TASK",
            ".spool/bootstrap-tasks/TEMPLATE.md",
            include_str!("../../../.spool/bootstrap-tasks/TEMPLATE.md"),
        )
        .expect("canonical template parses");

        assert_eq!(parsed.queue_key, "TASK");
        assert_eq!(parsed.priority, "normal");
        assert_eq!(parsed.state, "ready");
        assert!(!parsed.acceptance_criteria.is_empty());
        assert!(!parsed.validation_items.is_empty());
        assert!(parsed.brief.contains("# Task Brief"));
    }

    #[test]
    fn parser_reports_invalid_priority_with_allowed_values_and_hint() {
        let error = parse_bootstrap_task(
            "TASK",
            ".spool/bootstrap-tasks/foo.md",
            "---\ntitle: Test\npriority: moderate\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect_err("invalid priority fails");
        let message = error.to_string();

        assert!(message.contains(".spool/bootstrap-tasks/foo.md:3"));
        assert!(message.contains("invalid priority \"moderate\""));
        assert!(message.contains("expected one of: urgent, high, normal, low"));
        assert!(message.contains("accepted aliases: medium -> normal"));
    }

    #[test]
    fn parser_normalizes_medium_priority_alias_to_canonical_normal_with_warning() {
        let parsed = parse_bootstrap_task_with_warnings(
            "TASK",
            "inline",
            "---\ntitle: Test\npriority: medium\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect("parse");

        assert_eq!(parsed.task.priority, "normal");
        assert_eq!(
            parsed.warnings,
            vec!["normalized priority alias \"medium\" to canonical \"normal\"".to_string()]
        );
    }

    #[test]
    fn parser_reports_invalid_state_with_allowed_values() {
        let error = parse_bootstrap_task(
            "TASK",
            "inline",
            "---\ntitle: Test\nstate: in_progress\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect_err("invalid state fails");
        let message = error.to_string();

        assert!(message.contains("inline:3"));
        assert!(message.contains("invalid state \"in_progress\""));
        assert!(message.contains("expected one of: backlog, ready"));
    }

    #[test]
    fn parser_requires_front_matter() {
        let error = parse_bootstrap_task("TASK", "inline", "title: Missing delimiters")
            .expect_err("missing front matter fails");

        assert!(error.to_string().contains("must start"));
    }

    #[test]
    fn lint_uses_create_task_validation_rules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let task_file = temp.path().join("task.md");
        fs::write(
            &task_file,
            "---\ntitle: Test\npriority: maybe\nacceptance_criteria:\n  - It works\nvalidation_items:\n  - Tests pass\n---\nBrief\n",
        )
        .expect("write task file");

        let error = lint_bootstrap_task_file(&task_file).expect_err("invalid priority fails");

        let message = error.to_string();
        assert!(message.contains("invalid priority \"maybe\""));
        assert!(message.contains("expected one of: urgent, high, normal, low"));
    }
}
