use crate::api::GitStatus;

pub(super) fn resolve_text(g: &GitStatus) -> String {
    let op = g.op.as_deref().unwrap_or("");
    let what = match op {
        "rebase" => format!("变基（rebase）到 {}", g.target),
        "merge" => "合并（merge）".to_owned(),
        o => o.to_owned(),
    };
    let cont = match op {
        "rebase" => "git rebase --continue",
        "merge" => "git commit --no-edit",
        _ => "git cherry-pick --continue",
    };
    let files: Vec<String> = g.conflicts.iter().map(|f| format!("- {f}")).collect();
    format!(
        "这个工作区正在{what}，进行到一半，下面这些文件有冲突：\n{}\n\n请逐个解决：先弄清两边各自想做什么，把两边的意图都保留下来，不要整段只选一边。解决完 git add，再执行 {cont}（设置 GIT_EDITOR=true 免得卡在编辑器上）；后面的提交又冲突就接着解决，直到整个过程结束。\n不要 abort，不要动和冲突无关的代码。完成后用几句话说明每处冲突是怎么取舍的。",
        files.join("\n")
    )
}

pub(super) fn op_text(op: &str) -> &str {
    match op {
        "rebase" => "变基",
        "merge" => "合并",
        other => other,
    }
}

pub(super) fn code_mark(code: &str) -> (&'static str, &'static str) {
    match code {
        "??" => ("U", "added"),
        "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => ("!", "deleted"),
        c if c.starts_with('A') => ("A", "added"),
        c if c.starts_with('D') => ("D", "deleted"),
        c if c.starts_with('R') => ("R", "renamed"),
        _ => ("M", "modified"),
    }
}
