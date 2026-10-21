use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use blazar_core_types::transfer::{
    ImportAction, ImportItem, ImportReport, MAX_IMPORT_BYTES, section_label,
};

use crate::hub::{enc, fail_of, send};

pub fn default_name() -> PathBuf {
    PathBuf::from(format!(
        "blazar-export-{}.json",
        chrono::Local::now().format("%Y%m%d")
    ))
}

pub async fn export(hub: &str, out: Option<&Path>, force: bool) -> Result<i32> {
    let path = out.map_or_else(default_name, Path::to_path_buf);
    if path.exists() && !force {
        anyhow::bail!(
            "{} 已经存在；换个文件名，或者加 --force 覆盖",
            path.display()
        );
    }
    let r = send(hub, "GET", "/api/export", None).await?;
    if !(200..300).contains(&r.status) {
        return Err(fail_of(&r));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("写不了 {}", path.display()))?;
    file.write_all(r.text.as_bytes())
        .with_context(|| format!("写不了 {}", path.display()))?;
    println!("已导出到 {}", path.display());
    Ok(0)
}

fn action_label(item: &ImportItem) -> &'static str {
    match (item.action, item.imported) {
        (ImportAction::Create, _) => "新建",
        (ImportAction::Skip, _) => "跳过",
        (ImportAction::Attention, true) => "需处理（已导入）",
        (ImportAction::Attention, false) => "需处理（未导入）",
    }
}

pub fn render(report: &ImportReport) -> Vec<String> {
    let mut lines: Vec<String> = report
        .items
        .iter()
        .map(|i| {
            let mut line = format!(
                "{}\t{}\t{}",
                action_label(i),
                section_label(&i.section),
                i.key
            );
            if !i.detail.is_empty() {
                line.push_str(&format!("\t{}", i.detail));
            }
            line
        })
        .collect();
    lines.push(format!(
        "新建 {} · 跳过 {} · 需要处理 {}{}",
        report.counts.create,
        report.counts.skip,
        report.counts.attention,
        if report.applied {
            "，已导入"
        } else {
            "（试运行，没有写入）"
        }
    ));
    lines.extend(report.notes.iter().map(|n| format!("提示：{n}")));
    lines
}

pub async fn import(
    hub: &str,
    file: &Path,
    dry_run: bool,
    local_node: Option<&str>,
    self_node: Option<&str>,
) -> Result<i32> {
    let size = std::fs::metadata(file)
        .with_context(|| format!("读不了 {}", file.display()))?
        .len();
    if size > MAX_IMPORT_BYTES as u64 {
        anyhow::bail!(
            "{} 太大了（超过 {} MB）",
            file.display(),
            MAX_IMPORT_BYTES / 1024 / 1024
        );
    }
    let text =
        std::fs::read_to_string(file).with_context(|| format!("读不了 {}", file.display()))?;
    let mut path = format!("/api/import?dry_run={dry_run}");
    if let Some(node) = local_node {
        path.push_str(&format!("&local_node={}", enc(node)));
    }
    if let Some(node) = self_node {
        path.push_str(&format!("&self_node={}", enc(node)));
    }
    let r = send(hub, "POST", &path, Some(text)).await?;
    if !(200..300).contains(&r.status) {
        return Err(fail_of(&r));
    }
    let report: ImportReport =
        serde_json::from_value(r.body).context("hub 返回的导入报告读不懂")?;
    for line in render(&report) {
        println!("{line}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_lines_show_action_section_key_and_detail() {
        let mut report = ImportReport::default();
        report.push(ImportItem {
            section: "agents".into(),
            key: "reviewer".into(),
            action: ImportAction::Attention,
            imported: true,
            detail: "重新填写环境变量的值：KEY".into(),
        });
        report.push(ImportItem {
            section: "nodes".into(),
            key: "gpu-1".into(),
            action: ImportAction::Skip,
            imported: false,
            detail: String::new(),
        });
        report.notes.push("先停掉".into());
        assert_eq!(
            render(&report),
            vec![
                "需处理（已导入）\t智能体档案\treviewer\t重新填写环境变量的值：KEY",
                "跳过\t节点\tgpu-1",
                "新建 0 · 跳过 1 · 需要处理 1（试运行，没有写入）",
                "提示：先停掉",
            ]
        );
    }

    #[test]
    fn default_name_carries_the_date() {
        let name = default_name().display().to_string();
        assert!(name.starts_with("blazar-export-") && name.ends_with(".json"));
        assert_eq!(name.len(), "blazar-export-20261020.json".len());
    }
}
