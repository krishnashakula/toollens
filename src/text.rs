//! Tokenizer shared by keyword retrieval: splits snake/camel case, drops stopwords, light stemming.

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "for", "from", "has", "how", "i",
    "if", "in", "into", "is", "it", "its", "me", "my", "of", "on", "or", "our", "so", "that",
    "the", "their", "then", "this", "to", "us", "was", "we", "what", "when", "which", "who", "why",
    "will", "with", "you", "your",
];

pub fn tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut word = String::new();
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            // camelCase boundary: "listDeployments" -> "list", "deployments"
            if c.is_uppercase() && prev_lower && !word.is_empty() {
                push(&mut out, &word);
                word.clear();
            }
            word.extend(c.to_lowercase());
            prev_lower = c.is_lowercase() || c.is_numeric();
        } else {
            push(&mut out, &word);
            word.clear();
            prev_lower = false;
        }
    }
    push(&mut out, &word);
    out
}

fn push(out: &mut Vec<String>, word: &str) {
    if word.is_empty() || STOPWORDS.contains(&word) {
        return;
    }
    out.push(stem(word));
}

/// Suffix stripping tuned to keep inflections of one word together
/// ("deployments"/"deployed"/"deploying" -> "deploy"). Not linguistics; good enough for tool text.
pub fn stem(w: &str) -> String {
    let n = w.chars().count();
    if !w.is_ascii() || n <= 3 {
        return w.to_string();
    }
    let mut s = w.to_string();
    if s.ends_with("ies") && n > 4 {
        s.truncate(s.len() - 3);
        s.push('y');
    } else if s.ends_with("ing") && n > 5 {
        s.truncate(s.len() - 3);
        undouble(&mut s);
    } else if s.ends_with("ed") && n > 4 {
        s.truncate(s.len() - 2);
        undouble(&mut s);
    } else if s.ends_with('s') && !s.ends_with("ss") && !s.ends_with("us") {
        s.pop();
    }
    for suffix in ["ment", "ure"] {
        if s.ends_with(suffix) && s.len() > suffix.len() + 3 {
            s.truncate(s.len() - suffix.len());
        }
    }
    if s.ends_with('e') && s.len() > 4 {
        s.pop();
    }
    s
}

fn undouble(s: &mut String) {
    let b = s.as_bytes();
    if b.len() >= 2 && b[b.len() - 1] == b[b.len() - 2] && !b"aeiouls".contains(&b[b.len() - 1]) {
        s.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cases_and_drops_stopwords() {
        assert_eq!(
            tokens("listDeployments for the checkout-api"),
            ["list", "deploy", "checkout", "api"]
        );
        assert_eq!(tokens("get_pod_logs"), ["get", "pod", "log"]);
        assert!(tokens("").is_empty());
        assert_eq!(tokens("日本 service"), ["日本", "servic"]);
    }

    #[test]
    fn stems_group_inflections() {
        for group in [
            &[
                "deploy",
                "deploys",
                "deployed",
                "deploying",
                "deployment",
                "deployments",
            ][..],
            &["fail", "failed", "failing", "failure"],
            &["log", "logs", "logging", "logged"],
            &["change", "changes", "changed", "changing"],
            &["commit", "commits", "committed"],
            &["service", "services"],
            &["dependency", "dependencies"],
        ] {
            let stems: Vec<String> = group.iter().map(|w| stem(w)).collect();
            assert!(
                stems.iter().all(|s| s == &stems[0]),
                "{group:?} -> {stems:?}"
            );
        }
        assert_eq!(stem("status"), "status");
        assert_eq!(stem("access"), "access");
    }
}
