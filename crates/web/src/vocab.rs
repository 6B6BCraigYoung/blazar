#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Run,
    Attention,
    Failed,
    Other,
}

impl Tone {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Attention => "attention",
            Self::Failed => "failed",
            Self::Other => "other",
        }
    }
}

#[must_use]
pub fn state(raw: &str) -> (&'static str, Tone) {
    match raw {
        "running" | "in_progress" | "busy" => ("运行中", Tone::Run),
        "starting" => ("启动中", Tone::Run),
        "awaiting_approval" | "awaiting" | "approval" => ("等你批准", Tone::Attention),
        "lost" => ("已失联", Tone::Attention),
        "in_review" => ("待审阅", Tone::Attention),
        "failed" | "errored" | "error" => ("失败", Tone::Failed),
        "paused" => ("已暂停", Tone::Other),
        "offline" => ("离线", Tone::Other),
        "online" => ("在线", Tone::Other),
        "completed" | "done" => ("已完成", Tone::Other),
        "interrupted" => ("已中断", Tone::Other),
        "killed" => ("已结束", Tone::Other),
        "cancelled" => ("已取消", Tone::Other),
        "skipped" => ("已跳过", Tone::Other),
        "pending" => ("等待中", Tone::Other),
        "backlog" => ("待规划", Tone::Other),
        "todo" => ("待办", Tone::Other),
        "active" => ("已启用", Tone::Other),
        "archived" => ("已归档", Tone::Other),
        _ => ("空闲", Tone::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_and_failed_never_share_a_tone() {
        for raw in ["running", "in_progress", "busy", "starting"] {
            assert_eq!(state(raw).1, Tone::Run);
        }
        for raw in ["failed", "errored"] {
            assert_eq!(state(raw), ("失败", Tone::Failed));
        }
    }

    #[test]
    fn each_state_has_one_name() {
        assert_eq!(state("running").0, state("in_progress").0);
        assert_eq!(state("awaiting").0, "等你批准");
        assert_eq!(state("awaiting_approval").0, "等你批准");
        assert_eq!(state("paused"), ("已暂停", Tone::Other));
        assert_eq!(state("done").0, state("completed").0);
    }
}
