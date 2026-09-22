use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    ClaudeStreamJson,

    CodexJsonl,

    PlainLines,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeStyle {
    FlagSpace(&'static str),

    FlagEquals(&'static str),

    Subcommand(&'static str),

    Unsupported,
}

impl ResumeStyle {
    #[must_use]
    pub fn args(&self, id: &str) -> Vec<String> {
        match self {
            Self::FlagSpace(f) => vec![(*f).to_owned(), id.to_owned()],
            Self::FlagEquals(f) => vec![format!("{f}={id}")],
            Self::Subcommand(c) => vec![(*c).to_owned(), id.to_owned()],
            Self::Unsupported => Vec::new(),
        }
    }

    #[must_use]
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub id: &'static str,

    pub label: &'static str,

    pub program: &'static str,

    pub headless_args: &'static [&'static str],

    pub prompt_positional: bool,

    pub prompt_flag: Option<&'static str>,
    pub resume: ResumeStyle,
    pub output: OutputFormat,

    pub model_flag: Option<&'static str>,

    pub cwd_flag: Option<&'static str>,

    pub end_of_options: bool,

    pub interactive: bool,
}

impl AgentSpec {
    #[must_use]
    pub fn build_args(
        &self,
        prompt: &str,
        resume_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        extra: &[String],
    ) -> Vec<String> {
        let mut args = self.base_args(resume_id, model, cwd, extra);

        if self.prompt_positional {
            if self.end_of_options {
                args.push("--".to_owned());
            }
            args.push(prompt.to_owned());
        } else if let Some(flag) = self.prompt_flag {
            args.push(format!("{flag}={prompt}"));
        }
        args
    }

    #[must_use]
    pub fn base_args(
        &self,
        resume_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        extra: &[String],
    ) -> Vec<String> {
        let mut args: Vec<String> = self.headless_args.iter().map(|s| (*s).to_owned()).collect();

        if let (Some(flag), Some(dir)) = (self.cwd_flag, cwd) {
            args.push(flag.to_owned());
            args.push(dir.to_owned());
        }
        if let (Some(flag), Some(m)) = (self.model_flag, model) {
            args.push(flag.to_owned());
            args.push(m.to_owned());
        }

        args.extend(extra.iter().cloned());
        if let Some(id) = resume_id {
            args.extend(self.resume.args(id));
        }
        args
    }
}

pub const BUILTIN: &[AgentSpec] = &[
    AgentSpec {
        id: "claude",
        label: "Claude Code",
        program: "claude",

        headless_args: &["-p", "--output-format", "stream-json", "--verbose"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::FlagSpace("--resume"),
        output: OutputFormat::ClaudeStreamJson,
        model_flag: Some("--model"),
        cwd_flag: None,

        interactive: true,
        end_of_options: true,
    },
    AgentSpec {
        id: "codex",
        label: "Codex",
        program: "codex",
        headless_args: &["exec", "--json", "--skip-git-repo-check"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::Subcommand("resume"),
        output: OutputFormat::CodexJsonl,
        model_flag: Some("--model"),
        cwd_flag: Some("-C"),
        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "copilot",
        label: "GitHub Copilot CLI",
        program: "copilot",
        headless_args: &["-p"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::FlagEquals("--resume"),
        output: OutputFormat::PlainLines,
        model_flag: None,
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "opencode",
        label: "OpenCode",
        program: "opencode",
        headless_args: &["run"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::FlagSpace("--session"),
        output: OutputFormat::PlainLines,
        model_flag: Some("--model"),
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "qwen",
        label: "Qwen Code",
        program: "qwen",
        headless_args: &["-p"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::Unsupported,
        output: OutputFormat::PlainLines,
        model_flag: Some("-m"),
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "gemini",
        label: "Gemini CLI",
        program: "gemini",
        headless_args: &["-p"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::Unsupported,
        output: OutputFormat::PlainLines,
        model_flag: Some("-m"),
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "grok",
        label: "Grok Build",
        program: "grok",

        headless_args: &["--output-format", "plain"],
        prompt_positional: false,
        prompt_flag: Some("-p"),
        resume: ResumeStyle::FlagSpace("--resume"),
        output: OutputFormat::PlainLines,
        model_flag: Some("-m"),
        cwd_flag: Some("--cwd"),

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "dsh",
        label: "DeepSeek dsh",
        program: "dsh",

        headless_args: &["--profile", "headless"],
        prompt_positional: true,
        prompt_flag: None,
        resume: ResumeStyle::Unsupported,
        output: OutputFormat::PlainLines,
        model_flag: None,
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
    AgentSpec {
        id: "deepcode",
        label: "Deep Code",
        program: "deepcode",
        headless_args: &["-x"],
        prompt_positional: false,
        prompt_flag: Some("-p"),
        resume: ResumeStyle::FlagSpace("-r"),
        output: OutputFormat::PlainLines,
        model_flag: None,
        cwd_flag: None,

        interactive: false,
        end_of_options: false,
    },
];

#[must_use]
pub fn find(id: &str) -> Option<&'static AgentSpec> {
    BUILTIN.iter().find(|s| s.id == id)
}

#[must_use]
pub fn ids() -> Vec<&'static str> {
    BUILTIN.iter().map(|s| s.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_args_match_verified_invocation() {
        let s = find("claude").unwrap();
        let a = s.build_args("你好", None, None, Some("/tmp"), &[]);

        assert!(!a.contains(&"/tmp".to_owned()));
        assert_eq!(a[0], "-p");
        assert!(
            a.windows(2)
                .any(|w| w == ["--output-format", "stream-json"])
        );
        assert!(
            a.contains(&"--verbose".to_owned()),
            "stream-json 必须配 --verbose"
        );
        assert_eq!(a.last().unwrap(), "你好", "提示词必须在最后");
    }

    #[test]
    fn extra_args_go_before_resume_subcommand() {
        let s = find("codex").unwrap();
        let a = s.build_args(
            "hi",
            Some("t1"),
            None,
            Some("/w"),
            &["-s".into(), "workspace-write".into()],
        );
        let at = |x: &str| a.iter().position(|v| v == x).unwrap();
        assert!(at("-s") < at("resume"), "{a:?}");
        assert_eq!(a.last().unwrap(), "hi");
    }

    #[test]
    fn flag_prompt_is_attached() {
        let s = find("grok").unwrap();
        let a = s.build_args("- 改 A\n- 改 B", None, None, Some("/w"), &[]);
        assert_eq!(a.last().unwrap(), "-p=- 改 A\n- 改 B");
    }

    #[test]
    fn codex_passes_cwd_by_flag() {
        let s = find("codex").unwrap();
        let a = s.build_args("hi", None, None, Some("/w"), &[]);
        assert!(
            a.windows(2).any(|w| w == ["-C", "/w"]),
            "codex 用 -C 指定目录"
        );
    }

    #[test]
    fn resume_styles_render_differently() {
        assert_eq!(
            ResumeStyle::FlagSpace("--resume").args("x"),
            vec!["--resume", "x"]
        );
        assert_eq!(
            ResumeStyle::FlagEquals("--resume").args("x"),
            vec!["--resume=x"]
        );
        assert_eq!(
            ResumeStyle::Subcommand("resume").args("x"),
            vec!["resume", "x"]
        );
        assert!(ResumeStyle::Unsupported.args("x").is_empty());
    }

    #[test]
    fn unsupported_resume_is_visible() {
        assert!(!find("qwen").unwrap().resume.is_supported());
        assert!(find("claude").unwrap().resume.is_supported());
    }

    #[test]
    fn prompt_stays_last_even_with_resume_and_model() {
        let s = find("claude").unwrap();
        let a = s.build_args(
            "最后一句",
            Some("sess-1"),
            Some("opus"),
            None,
            &["--x".into()],
        );
        assert_eq!(a.last().unwrap(), "最后一句");
        assert!(a.windows(2).any(|w| w == ["--resume", "sess-1"]));
        assert!(a.windows(2).any(|w| w == ["--model", "opus"]));
    }

    #[test]
    fn every_builtin_has_unique_id_and_a_way_to_pass_prompt() {
        let mut seen = std::collections::HashSet::new();
        for s in BUILTIN {
            assert!(seen.insert(s.id), "id 重复: {}", s.id);
            assert!(
                s.prompt_positional || s.prompt_flag.is_some(),
                "{} 没有任何传提示词的方式",
                s.id
            );
        }
    }
}
