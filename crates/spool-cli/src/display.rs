pub fn truncate_title(title: &str, max_chars: usize) -> String {
    let mut chars = title.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        if max_chars <= 1 {
            "…".to_string()
        } else {
            let keep = max_chars.saturating_sub(1);
            format!("{}…", prefix.chars().take(keep).collect::<String>())
        }
    } else {
        prefix
    }
}

pub fn task_label(identifier: &str, title: &str, max_title_chars: usize) -> String {
    format!("{} {}", identifier, truncate_title(title, max_title_chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_titles_with_ellipsis() {
        assert_eq!(truncate_title("Short", 20), "Short");
        assert_eq!(truncate_title("Long title", 5), "Long…");
        assert_eq!(truncate_title("éclair", 4), "écl…");
    }
}
