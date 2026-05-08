use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BootstrapFrontMatter {
    title: String,
    priority: Option<String>,
    state: Option<String>,
    acceptance_criteria: Option<Vec<String>>,
    validation_items: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    review_required: Option<bool>,
}

pub fn parse_bootstrap_task_file(queue_key: &str, file: &Path) -> Result<tasker_db::CreateTask> {
    let text =
        fs::read_to_string(file).with_context(|| format!("failed to read {}", file.display()))?;
    parse_bootstrap_task(queue_key, &file.display().to_string(), &text)
}

pub fn parse_bootstrap_task(
    queue_key: &str,
    source_name: &str,
    text: &str,
) -> Result<tasker_db::CreateTask> {
    let (front_matter, brief) = split_front_matter(text)?;
    let front_matter: BootstrapFrontMatter = serde_yaml::from_str(front_matter)
        .with_context(|| format!("failed to parse YAML front matter in {source_name}"))?;

    Ok(tasker_db::CreateTask {
        queue_key: queue_key.to_string(),
        title: front_matter.title,
        brief: brief.trim().to_string(),
        priority: normalize_label(front_matter.priority.as_deref().unwrap_or("normal")),
        state: normalize_label(front_matter.state.as_deref().unwrap_or("ready")),
        review_required: front_matter.review_required.unwrap_or(false),
        acceptance_criteria: front_matter.acceptance_criteria.unwrap_or_default(),
        validation_items: front_matter.validation_items.unwrap_or_default(),
        tags: front_matter.tags.unwrap_or_default(),
    })
}

fn split_front_matter(text: &str) -> Result<(&str, &str)> {
    let Some(after_start) = text.strip_prefix("---\n") else {
        anyhow::bail!("bootstrap task file must start with YAML front matter delimited by ---");
    };
    let Some((front_matter, body)) = after_start.split_once("\n---\n") else {
        anyhow::bail!("bootstrap task file must close YAML front matter with ---");
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
    fn parser_requires_front_matter() {
        let error = parse_bootstrap_task("TASK", "inline", "title: Missing delimiters")
            .expect_err("missing front matter fails");

        assert!(error.to_string().contains("must start"));
    }
}
