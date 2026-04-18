use eframe::egui;

use crate::codex_store::{CodexStore, MergePreview, MergeReport, RepairReport, ScanResult};

pub struct CodexMergeApp {
    store: CodexStore,
    scan_result: Option<ScanResult>,
    selected_target_provider: String,
    preview: Option<MergePreview>,
    show_confirm_dialog: bool,
    last_error: Option<String>,
    last_report: Option<RunReport>,
}

#[derive(Debug, Clone)]
enum RunReport {
    Merge(MergeReport),
    Repair(RepairReport),
}

impl CodexMergeApp {
    pub fn new(store: CodexStore) -> Self {
        let mut app = Self {
            store,
            scan_result: None,
            selected_target_provider: String::new(),
            preview: None,
            show_confirm_dialog: false,
            last_error: None,
            last_report: None,
        };
        app.refresh_scan();
        app
    }

    fn refresh_scan(&mut self) {
        match self.store.scan() {
            Ok(scan) => {
                let provider_names: Vec<String> = scan
                    .provider_summaries
                    .iter()
                    .map(|summary| summary.name.clone())
                    .collect();
                if self.selected_target_provider.is_empty()
                    || !provider_names.contains(&self.selected_target_provider)
                {
                    self.selected_target_provider = scan.current_provider.clone();
                }
                self.scan_result = Some(scan);
                self.last_error = None;
                self.refresh_preview();
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }

    fn refresh_preview(&mut self) {
        if self.selected_target_provider.is_empty() {
            self.preview = None;
            return;
        }

        match self.store.build_preview(&self.selected_target_provider) {
            Ok(preview) => {
                self.preview = Some(preview);
                self.last_error = None;
            }
            Err(error) => {
                self.preview = None;
                self.last_error = Some(format!("{error:#}"));
            }
        }
    }

    fn execute_merge(&mut self) {
        match self.store.execute_merge(&self.selected_target_provider) {
            Ok(report) => {
                self.show_confirm_dialog = false;
                self.refresh_scan();
                self.last_error = if report.errors.is_empty() {
                    None
                } else {
                    Some(report.errors.join("\n"))
                };
                self.last_report = Some(RunReport::Merge(report));
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }

    fn execute_repair(&mut self) {
        match self.store.execute_repair() {
            Ok(report) => {
                self.refresh_scan();
                self.last_error = if report.errors.is_empty() {
                    None
                } else {
                    Some(report.errors.join("\n"))
                };
                self.last_report = Some(RunReport::Repair(report));
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }
}

impl eframe::App for CodexMergeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("summary").show(ctx, |ui| {
            ui.heading("codex-merge-session");
            ui.label(format!(
                "Codex 根目录: {}",
                self.store.codex_root().display()
            ));
            if let Some(scan) = &self.scan_result {
                ui.label(format!("当前配置 provider: {}", scan.current_provider));
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.heading("Provider 扫描结果");
                    if let Some(scan) = &self.scan_result {
                        egui::Grid::new("providers-grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label("Provider");
                                ui.label("活跃");
                                ui.label("归档");
                                ui.label("最近更新时间");
                                ui.end_row();

                                for summary in &scan.provider_summaries {
                                    ui.label(&summary.name);
                                    ui.label(summary.active_count.to_string());
                                    ui.label(summary.archived_count.to_string());
                                    ui.label(
                                        summary
                                            .latest_updated_at
                                            .map(|value| value.to_string())
                                            .unwrap_or_else(|| "-".to_string()),
                                    );
                                    ui.end_row();
                                }
                            });
                    } else {
                        ui.label("尚未加载到 provider 数据。");
                    }
                });

                columns[1].group(|ui| {
                    let provider_names: Vec<String> = self
                        .scan_result
                        .as_ref()
                        .map(|scan| {
                            scan.provider_summaries
                                .iter()
                                .map(|summary| summary.name.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    let can_merge = self
                        .preview
                        .as_ref()
                        .map(|preview| !preview.affected_thread_ids.is_empty())
                        .unwrap_or(false);

                    ui.horizontal_top(|ui| {
                        ui.heading("合并控制台");
                        let remaining_width = ui.available_width();
                        ui.allocate_ui_with_layout(
                            egui::vec2(remaining_width, 0.0),
                            control_action_column_layout(),
                            |ui| {
                                if ui.add_enabled(can_merge, merge_button(can_merge)).clicked() {
                                    self.show_confirm_dialog = true;
                                }
                                if ui.add(repair_button()).clicked() {
                                    self.execute_repair();
                                }
                            },
                        );
                    });

                    egui::ComboBox::from_label("目标 provider")
                        .selected_text(if self.selected_target_provider.is_empty() {
                            "请选择".to_string()
                        } else {
                            self.selected_target_provider.clone()
                        })
                        .show_ui(ui, |ui| {
                            for name in provider_names {
                                if ui
                                    .selectable_value(
                                        &mut self.selected_target_provider,
                                        name.clone(),
                                        name,
                                    )
                                    .changed()
                                {
                                    self.refresh_preview();
                                }
                            }
                        });

                    ui.horizontal(|ui| {
                        if ui.button("重新扫描").clicked() {
                            self.refresh_scan();
                        }
                        if ui.button("刷新预览").clicked() {
                            self.refresh_preview();
                        }
                    });

                    if let Some(preview) = &self.preview {
                        ui.separator();
                        ui.label(format!(
                            "待迁移线程数: {}",
                            preview.affected_thread_ids.len()
                        ));
                        ui.label(format!(
                            "待迁移文件数: {}",
                            preview.affected_rollout_paths.len()
                        ));
                        ui.label(format!(
                            "来源 provider: {}",
                            if preview.source_providers.is_empty() {
                                "-".to_string()
                            } else {
                                preview.source_providers.join(", ")
                            }
                        ));
                        ui.label(format!("备份目录: {}", preview.backup_dir.display()));
                    } else {
                        ui.separator();
                        ui.label("暂时无法生成预览，请先处理上方错误信息。");
                    }

                    if !can_merge {
                        ui.label("当前没有可合并的会话，或预览尚未就绪。");
                    }
                });
            });

            ui.separator();
            ui.heading("结果与错误");
            if let Some(error) = &self.last_error {
                ui.colored_label(egui::Color32::RED, error);
            } else if let Some(report) = &self.last_report {
                ui.label(format_run_report(report));
            } else {
                ui.label("暂无执行结果。");
            }
        });

        if self.show_confirm_dialog {
            let preview = self.preview.clone();
            egui::Window::new("确认合并")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .default_width(380.0)
                .movable(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if let Some(preview) = preview {
                        ui.label(format!("目标 provider: {}", preview.target_provider));
                        ui.label(format!(
                            "将迁移 {} 条线程，涉及 {} 个源 provider。",
                            preview.affected_thread_ids.len(),
                            preview.source_providers.len()
                        ));
                        ui.label(format!("备份将写入: {}", preview.backup_dir.display()));
                        ui.separator();
                    }

                    ui.horizontal(|ui| {
                        if ui.button("确认执行").clicked() {
                            self.execute_merge();
                        }
                        if ui.button("取消").clicked() {
                            self.show_confirm_dialog = false;
                        }
                    });
                });
        }
    }
}

fn merge_button(can_merge: bool) -> egui::Button<'static> {
    let style = merge_button_style(can_merge);

    egui::Button::new(
        egui::RichText::new("开始合并会话")
            .size(16.0)
            .strong()
            .color(style.text_color),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke_color))
    .corner_radius(9.0)
    .min_size(egui::vec2(style.min_width, style.min_height))
}

fn repair_button() -> egui::Button<'static> {
    let style = repair_button_style();

    egui::Button::new(
        egui::RichText::new("尝试修复")
            .size(style.text_size)
            .strong()
            .color(style.text_color),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke_color))
    .corner_radius(9.0)
    .min_size(egui::vec2(style.min_width, style.min_height))
}

fn control_action_column_layout() -> egui::Layout {
    egui::Layout::top_down(egui::Align::Max)
}

struct MergeButtonStyle {
    fill: egui::Color32,
    stroke_color: egui::Color32,
    text_color: egui::Color32,
    min_width: f32,
    min_height: f32,
}

struct RepairButtonStyle {
    fill: egui::Color32,
    stroke_color: egui::Color32,
    text_color: egui::Color32,
    min_width: f32,
    min_height: f32,
    text_size: f32,
}

fn merge_button_style(can_merge: bool) -> MergeButtonStyle {
    if can_merge {
        return MergeButtonStyle {
            fill: egui::Color32::from_rgb(28, 98, 94),
            stroke_color: egui::Color32::from_rgb(17, 66, 63),
            text_color: egui::Color32::WHITE,
            min_width: 176.0,
            min_height: 34.0,
        };
    }

    MergeButtonStyle {
        fill: egui::Color32::from_gray(130),
        stroke_color: egui::Color32::from_gray(95),
        text_color: egui::Color32::from_gray(235),
        min_width: 176.0,
        min_height: 34.0,
    }
}

fn repair_button_style() -> RepairButtonStyle {
    RepairButtonStyle {
        fill: egui::Color32::from_rgb(246, 230, 222),
        stroke_color: egui::Color32::from_rgb(201, 148, 128),
        text_color: egui::Color32::from_rgb(109, 42, 26),
        min_width: 176.0,
        min_height: 30.0,
        text_size: 15.0,
    }
}

fn format_run_report(report: &RunReport) -> String {
    match report {
        RunReport::Merge(report) => format_merge_report(report),
        RunReport::Repair(report) => format_repair_report(report),
    }
}

fn format_merge_report(report: &MergeReport) -> String {
    if report.rolled_back {
        return format!(
            "最近一次执行未完成：合并过程中出现问题，程序已经自动回滚。备份目录：{}",
            report.backup_dir.display()
        );
    }

    if report.migrated_thread_count == 0 && report.migrated_file_count == 0 {
        return "最近一次执行未发现需要合并的会话。".to_string();
    }

    format!(
        "最近一次执行已完成：成功合并了 {} 条会话，更新了 {} 个会话文件。备份已保存到 {}",
        report.migrated_thread_count,
        report.migrated_file_count,
        report.backup_dir.display()
    )
}

fn format_repair_report(report: &RepairReport) -> String {
    if report.rolled_back {
        return format!(
            "最近一次修复未完成：修复过程中出现问题，程序已经自动回滚。备份目录：{}",
            report.backup_dir.display()
        );
    }

    if report.normalized_cwd_count == 0
        && report.normalized_rollout_file_count == 0
        && report.normalized_workspace_path_count == 0
        && !report.rebuilt_session_index
    {
        return "最近一次修复检查完成：没有发现需要修复的数据。".to_string();
    }

    let mut parts = Vec::new();
    if report.normalized_cwd_count > 0 {
        parts.push(format!("规范化了 {} 条线程路径", report.normalized_cwd_count));
    }
    if report.normalized_rollout_file_count > 0 {
        parts.push(format!(
            "清理了 {} 个会话文件里的路径残留",
            report.normalized_rollout_file_count
        ));
    }
    if report.normalized_workspace_path_count > 0 {
        parts.push(format!(
            "同步修复了 {} 处项目状态路径",
            report.normalized_workspace_path_count
        ));
    }
    if report.rebuilt_session_index {
        parts.push("重建了会话索引".to_string());
    }

    if parts.is_empty() {
        return format!(
            "最近一次修复已完成：备份已保存到 {}",
            report.backup_dir.display()
        );
    }

    format!(
        "最近一次修复已完成：{}。备份已保存到 {}",
        parts.join("，并"),
        report.backup_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn merge_button_uses_compact_teal_primary_style() {
        let style = merge_button_style(true);

        assert_eq!(style.fill, egui::Color32::from_rgb(28, 98, 94));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(17, 66, 63));
        assert_eq!(style.text_color, egui::Color32::WHITE);
        assert_eq!(style.min_width, 176.0);
        assert_eq!(style.min_height, 34.0);
    }

    #[test]
    fn repair_report_message_is_human_friendly_for_success() {
        let report = RepairReport {
            normalized_cwd_count: 147,
            normalized_rollout_file_count: 12,
            normalized_workspace_path_count: 8,
            rebuilt_session_index: true,
            backup_dir: PathBuf::from(
                r"C:\Users\home127\.codex-merge-session\backups\1776494198-cwd-normalize",
            ),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_repair_report(&report),
            "最近一次修复已完成：规范化了 147 条线程路径，并清理了 12 个会话文件里的路径残留，并同步修复了 8 处项目状态路径，并重建了会话索引。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776494198-cwd-normalize"
        );
    }

    #[test]
    fn repair_report_message_is_human_friendly_for_noop() {
        let report = RepairReport {
            normalized_cwd_count: 0,
            normalized_rollout_file_count: 0,
            normalized_workspace_path_count: 0,
            rebuilt_session_index: false,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1776494198"),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_repair_report(&report),
            "最近一次修复检查完成：没有发现需要修复的数据。"
        );
    }

    #[test]
    fn repair_report_message_is_human_friendly_for_global_state_only() {
        let report = RepairReport {
            normalized_cwd_count: 0,
            normalized_rollout_file_count: 0,
            normalized_workspace_path_count: 8,
            rebuilt_session_index: false,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1776494198"),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_repair_report(&report),
            "最近一次修复已完成：同步修复了 8 处项目状态路径。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776494198"
        );
    }

    #[test]
    fn merge_report_message_is_human_friendly_for_success() {
        let report = MergeReport {
            migrated_thread_count: 90,
            migrated_file_count: 90,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1776492506"),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_merge_report(&report),
            "最近一次执行已完成：成功合并了 90 条会话，更新了 90 个会话文件。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776492506"
        );
    }

    #[test]
    fn merge_report_message_is_human_friendly_for_rollback() {
        let report = MergeReport {
            migrated_thread_count: 0,
            migrated_file_count: 0,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1776492506"),
            rolled_back: true,
            errors: vec!["写入失败".to_string()],
        };

        assert_eq!(
            format_merge_report(&report),
            "最近一次执行未完成：合并过程中出现问题，程序已经自动回滚。备份目录：C:\\Users\\home127\\.codex-merge-session\\backups\\1776492506"
        );
    }

    #[test]
    fn repair_button_uses_soft_warning_style() {
        let style = repair_button_style();

        assert_eq!(style.fill, egui::Color32::from_rgb(246, 230, 222));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(201, 148, 128));
        assert_eq!(style.text_color, egui::Color32::from_rgb(109, 42, 26));
        assert_eq!(style.min_width, 176.0);
        assert_eq!(style.min_height, 30.0);
        assert_eq!(style.text_size, 15.0);
    }

    #[test]
    fn control_action_layout_keeps_buttons_right_aligned() {
        let layout = control_action_column_layout();

        assert_eq!(layout.main_dir, egui::Direction::TopDown);
        assert_eq!(layout.cross_align, egui::Align::Max);
        assert!(!layout.main_wrap);
    }
}
