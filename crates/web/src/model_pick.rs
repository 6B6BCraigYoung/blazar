/// One layer of a model choice: `(model, effort)`. `None` leaves a field to the layers below;
/// an empty string is an explicit pick of the runtime's own default.
pub type Layer = (Option<String>, Option<String>);

/// The choice a conversation runs with: each field from the first layer that sets it.
pub fn layered(layers: &[Option<&Layer>]) -> Layer {
    let mut model = None;
    let mut effort = None;
    for (m, e) in layers.iter().flatten() {
        if model.is_none() {
            model.clone_from(m);
        }
        if effort.is_none() {
            effort.clone_from(e);
        }
    }
    (model, effort)
}

pub fn plain_desc(desc: &str) -> String {
    desc.split(" · ")
        .filter(|part| !is_price(part))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn is_price(part: &str) -> bool {
    let p = part.trim();
    p.starts_with('$') && p.to_ascii_lowercase().ends_with(" per mtok")
}

pub fn short_name(label: &str, desc: &str) -> String {
    let label = label
        .replace(" (recommended)", "")
        .replace(" (Recommended)", "");
    let first = desc.split(" · ").next().unwrap_or_default().trim();
    let versioned = first
        .strip_prefix(label.as_str())
        .and_then(|rest| rest.strip_prefix(' '))
        .is_some_and(|rest| {
            let number = rest.split(' ').next().unwrap_or_default();
            let tail = rest[number.len()..].trim();
            number.starts_with(|c: char| c.is_ascii_digit())
                && number.chars().all(|c| c.is_ascii_digit() || c == '.')
                && (tail.is_empty() || (tail.starts_with('[') && tail.ends_with(']')))
        });
    if versioned { first.to_owned() } else { label }
}

pub fn custom_model_name(id: &str) -> Option<String> {
    let rest = id.strip_prefix("custom:")?;
    if let Some((provider, model)) = rest.split_once('/') {
        let provider = if provider == "chatgpt" {
            "ChatGPT"
        } else {
            provider
        };
        return Some(format!("{model} · {provider}"));
    }
    let name = match rest.rsplit_once('-') {
        Some((head, tail)) if !head.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => rest,
    };
    Some(name.replace('-', " "))
}

pub fn split_models(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in s.split([',', '，', '、', ' ', '\n', '\t']).map(str::trim) {
        if !m.is_empty() && !out.iter().any(|x| x == m) {
            out.push(m.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn droid_custom_models_read_as_names() {
        assert_eq!(
            custom_model_name("custom:DeepSeek-Flash-0").as_deref(),
            Some("DeepSeek Flash")
        );
        assert_eq!(
            custom_model_name("custom:chatgpt/gpt-6-astra").as_deref(),
            Some("gpt-6-astra · ChatGPT")
        );
        assert_eq!(
            custom_model_name("custom:my-model").as_deref(),
            Some("my model")
        );
        assert_eq!(custom_model_name("gpt-6.1-sol"), None);
    }

    #[test]
    fn model_lists_split_on_any_separator() {
        assert_eq!(
            split_models("deepseek-v4-pro, deepseek-flash，deepseek-flash、 glm-5\n"),
            vec!["deepseek-v4-pro", "deepseek-flash", "glm-5"]
        );
        assert!(split_models(" , ").is_empty());
    }

    #[test]
    fn subscription_prices_are_dropped_and_the_rest_is_kept_verbatim() {
        assert_eq!(
            plain_desc(
                "Fable 5.2 · Most capable for your hardest and longest-running tasks · $8/$40 per Mtok"
            ),
            "Fable 5.2 · Most capable for your hardest and longest-running tasks"
        );
        assert_eq!(
            plain_desc("Use the default model (currently Opus 5.5) · $4/$20 per Mtok"),
            "Use the default model (currently Opus 5.5)"
        );
        assert_eq!(
            plain_desc("Opus 5 · Best for everyday, complex tasks"),
            "Opus 5 · Best for everyday, complex tasks"
        );
        assert_eq!(
            plain_desc("Costs $5 per day · fast"),
            "Costs $5 per day · fast"
        );
        assert_eq!(plain_desc(""), "");
    }

    #[test]
    fn the_short_name_carries_the_version_the_cli_names() {
        assert_eq!(
            short_name("Fable", "Fable 5.2 · Most capable · $8/$40 per Mtok"),
            "Fable 5.2"
        );
        assert_eq!(
            short_name("Opus", "Opus 5.5 · For complex work"),
            "Opus 5.5"
        );
        assert_eq!(
            short_name("Fable", "Fable 5.1 [1m] · Long context"),
            "Fable 5.1 [1m]"
        );
        assert_eq!(
            short_name(
                "Default (recommended)",
                "Use the default model (currently Opus 5.5)"
            ),
            "Default"
        );
        assert_eq!(short_name("Fable 5.1", "Most capable"), "Fable 5.1");
        assert_eq!(
            short_name("Haiku", "Haiku is fastest for quick answers"),
            "Haiku",
            "描述只是以名字开头的一句话时不拿来当名字"
        );
        assert_eq!(short_name("Sonnet", ""), "Sonnet");
        assert_eq!(
            short_name("Fable", "Fable 5.2 (preview) · Most capable"),
            "Fable",
            "版本号后面跟着别的字，就不拿来当名字"
        );
        assert_eq!(
            short_name(
                "Fable",
                "Fable 5.2 · Last run on this machine · Most capable"
            ),
            "Fable 5.2"
        );
    }

    fn l(m: Option<&str>, e: Option<&str>) -> Layer {
        (m.map(str::to_owned), e.map(str::to_owned))
    }

    #[test]
    fn each_field_comes_from_the_first_layer_that_sets_it() {
        let thread = l(Some("haiku"), None);
        let ws = l(Some("opus"), Some("high"));
        let global = l(Some("sonnet"), Some("low"));
        assert_eq!(
            layered(&[Some(&thread), Some(&ws), Some(&global)]),
            l(Some("haiku"), Some("high"))
        );
        assert_eq!(layered(&[None, Some(&ws), Some(&global)]), ws);
        assert_eq!(layered(&[None, None, Some(&global)]), global);
        assert_eq!(layered(&[None, None, None]), l(None, None));
    }

    #[test]
    fn an_explicit_default_is_not_filled_from_below() {
        let thread = l(Some(""), Some(""));
        let ws = l(Some("opus"), Some("high"));
        assert_eq!(layered(&[Some(&thread), Some(&ws)]), l(Some(""), Some("")));
    }
}
