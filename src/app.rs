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
        ctx.style_mut(|style| {
            style.visuals.window_fill = page_background_fill();
            style.visuals.panel_fill = page_background_fill();
        });

        egui::TopBottomPanel::top("summary")
            .frame(
                egui::Frame::default()
                    .fill(page_header_fill())
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered_justified(|ui| {
                    ui.heading(egui::RichText::new("codex-merge-session").size(20.0).strong());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Codex 根目录:")
                                .color(text_secondary_color()),
                        );
                        ui.label(
                            egui::RichText::new(format!("{}", self.store.codex_root().display()))
                                .color(text_primary_color()),
                        );
                    });
                    if let Some(scan) = &self.scan_result {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("当前配置 provider:")
                                    .color(text_secondary_color()),
                            );
                            ui.label(
                                egui::RichText::new(&scan.current_provider)
                                    .color(text_primary_color()),
                            );
                        });
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(page_background_fill()).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.columns(2, |columns| {
                    columns[0].add_space(4.0);
                    columns[0].group(|ui| {
                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.heading(egui::RichText::new("Provider 概览").size(16.0).strong());
                            ui.add_space(8.0);

                            if let Some(scan) = &self.scan_result {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "已发现 {} 个 provider",
                                        scan.provider_summaries.len()
                                    ))
                                    .color(text_secondary_color())
                                    .size(13.0),
                                );
                                ui.add_space(12.0);

                                egui::Grid::new("providers-grid")
                                    .striped(true)
                                    .min_row_height(28.0)
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("Provider")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.label(
                                            egui::RichText::new("活跃")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.label(
                                            egui::RichText::new("归档")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.label(
                                            egui::RichText::new("最近更新时间")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.end_row();

                                        for summary in &scan.provider_summaries {
                                            let is_selected =
                                                summary.name == self.selected_target_provider;
                                            let row_frame = if is_selected {
                                                egui::Frame::default().fill(selected_row_fill())
                                            } else {
                                                egui::Frame::default()
                                            };

                                            row_frame.show(ui, |ui| {
                                                ui.label(egui::RichText::new(&summary.name).color(text_primary_color()).size(14.0));
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(egui::RichText::new(summary.active_count.to_string()).color(text_primary_color()).size(14.0));
                                                });
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(egui::RichText::new(summary.archived_count.to_string()).color(text_primary_color()).size(14.0));
                                                });
                                                ui.label(
                                                    egui::RichText::new(
                                                        summary
                                                            .latest_updated_at
                                                            .map(|value| value.to_string())
                                                            .unwrap_or_else(|| "-".to_string()),
                                                    )
                                                    .color(text_primary_color())
                                                    .size(14.0),
                                                );
                                                ui.end_row();
                                            });
                                        }
                                    });
                            } else {
                                ui.label(
                                    egui::RichText::new("尚未加载到 provider 数据。")
                                        .color(text_secondary_color()),
                                );
                            }
                        });
                    });

                    columns[1].add_space(4.0);
                    columns[1].group(|ui| {
                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
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
                                ui.vertical(|ui| {
                                    ui.heading(egui::RichText::new("会话操作").size(16.0).strong());
                                });
                                let remaining_width = ui.available_width();
                                ui.allocate_ui_with_layout(
                                    egui::vec2(remaining_width, 0.0),
                                    control_action_column_layout(),
                                    |ui| {
                                        if ui.add_enabled(can_merge, merge_button(can_merge)).clicked() {
                                            self.show_confirm_dialog = true;
                                        }
                                        ui.add_space(8.0);
                                        if ui.add(repair_button()).clicked() {
                                            self.execute_repair();
                                        }
                                    },
                                );
                            });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(12.0);

                            ui.label(egui::RichText::new("目标 provider").color(text_primary_color()).size(14.0));
                            ui.add_space(6.0);
                            egui::ComboBox::new("target_provider_combo", "")
                                .selected_text(if self.selected_target_provider.is_empty() {
                                    "请选择".to_string()
                                } else {
                                    self.selected_target_provider.clone()
                                })
                                .width(ui.available_width())
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

                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.add(secondary_button("重新扫描")).clicked() {
                                    self.refresh_scan();
                                }
                                if ui.add(secondary_button("刷新预览")).clicked() {
                                    self.refresh_preview();
                                }
                            });

                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(12.0);

                            if let Some(preview) = &self.preview {
                                ui.label(egui::RichText::new("预览信息").color(text_primary_color()).size(14.0).strong());
                                ui.add_space(10.0);
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("待迁移线程数:").color(text_secondary_color()));
                                        ui.label(egui::RichText::new(format!("{}", preview.affected_thread_ids.len())).color(text_primary_color()).strong());
                                    });
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("待迁移文件数:").color(text_secondary_color()));
                                        ui.label(egui::RichText::new(format!("{}", preview.affected_rollout_paths.len())).color(text_primary_color()).strong());
                                    });
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("来源 provider:").color(text_secondary_color()));
                                        ui.label(
                                            egui::RichText::new(
                                                if preview.source_providers.is_empty() {
                                                    "-".to_string()
                                                } else {
                                                    preview.source_providers.join(", ")
                                                },
                                            )
                                            .color(text_primary_color()),
                                        );
                                    });
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new(format!("备份目录: {}", preview.backup_dir.display())).color(text_secondary_color()).size(13.0));
                                });
                            } else {
                                info_tone().show(ui, |ui| {
                                    ui.label(egui::RichText::new("暂时无法生成预览，请先处理上方错误信息。").color(info_tone().text));
                                });
                            }

                            if !can_merge {
                                ui.add_space(8.0);
                                info_tone().show(ui, |ui| {
                                    ui.label(egui::RichText::new("当前没有可合并的会话，或预览尚未就绪。").color(info_tone().text));
                                });
                            }
                        });
                    });
                });

                ui.add_space(12.0);

                card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.heading(egui::RichText::new("执行结果与错误").size(16.0).strong());
                    ui.add_space(10.0);

                    if let Some(error) = &self.last_error {
                        error_tone().show(ui, |ui| {
                            ui.label(egui::RichText::new(error).color(error_tone().text));
                        });
                    } else if let Some(report) = &self.last_report {
                        success_tone().show(ui, |ui| {
                            ui.label(egui::RichText::new(format_run_report(report)).color(success_tone().text));
                        });
                    } else {
                        info_tone().show(ui, |ui| {
                            ui.label(egui::RichText::new("暂无执行结果。").color(info_tone().text));
                        });
                    }
                });
            });

        if self.show_confirm_dialog {
            let preview = self.preview.clone();
            egui::Window::new("确认合并")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .default_width(420.0)
                .movable(false)
                .resizable(false)
                .frame(card_frame())
                .show(ctx, |ui| {
                    if let Some(preview) = preview {
                        ui.label(egui::RichText::new(format!("目标 provider: {}", preview.target_provider)).color(text_primary_color()));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(format!(
                            "将迁移 {} 条线程，涉及 {} 个源 provider。",
                            preview.affected_thread_ids.len(),
                            preview.source_providers.len()
                        )).color(text_primary_color()));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(format!("备份将写入: {}", preview.backup_dir.display())).color(text_secondary_color()).size(13.0));
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(12.0);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(primary_button("确认执行")).clicked() {
                            self.execute_merge();
                        }
                        ui.add_space(8.0);
                        if ui.add(secondary_button("取消")).clicked() {
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
        egui::RichText::new("开始合并")
            .size(15.0)
            .strong()
            .color(style.text_color),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke_color))
    .corner_radius(8.0)
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
    .corner_radius(8.0)
    .min_size(egui::vec2(style.min_width, style.min_height))
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(15.0).strong().color(egui::Color32::WHITE))
        .fill(egui::Color32::from_rgb(31, 87, 122))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(22, 63, 88)))
        .corner_radius(8.0)
        .min_size(egui::vec2(120.0, 36.0))
}

fn secondary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).size(14.0).color(egui::Color32::from_rgb(61, 79, 97)))
        .fill(egui::Color32::WHITE)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(137, 156, 177)))
        .corner_radius(8.0)
        .min_size(egui::vec2(100.0, 32.0))
}

fn control_action_column_layout() -> egui::Layout {
    egui::Layout::top_down(egui::Align::Max)
}

#[derive(Debug)]
struct MergeButtonStyle {
    fill: egui::Color32,
    stroke_color: egui::Color32,
    text_color: egui::Color32,
    min_width: f32,
    min_height: f32,
}

#[derive(Debug)]
struct RepairButtonStyle {
    fill: egui::Color32,
    stroke_color: egui::Color32,
    text_color: egui::Color32,
    min_width: f32,
    min_height: f32,
    text_size: f32,
}

#[derive(Debug)]
struct StatusTone {
    fill: egui::Color32,
    text: egui::Color32,
    stroke: egui::Color32,
}

impl StatusTone {
    fn show<R>(&self, ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
        let frame = egui::Frame::default()
            .fill(self.fill)
            .stroke(egui::Stroke::new(1.0, self.stroke))
            .inner_margin(egui::Margin::same(10))
            .corner_radius(8.0);
        frame.show(ui, add_contents).inner
    }
}

fn merge_button_style(can_merge: bool) -> MergeButtonStyle {
    if can_merge {
        return MergeButtonStyle {
            fill: egui::Color32::from_rgb(31, 87, 122),
            stroke_color: egui::Color32::from_rgb(22, 63, 88),
            text_color: egui::Color32::WHITE,
            min_width: 148.0,
            min_height: 38.0,
        };
    }

    MergeButtonStyle {
        fill: egui::Color32::from_rgb(188, 198, 209),
        stroke_color: egui::Color32::from_rgb(164, 175, 188),
        text_color: egui::Color32::from_rgb(246, 248, 250),
        min_width: 148.0,
        min_height: 38.0,
    }
}

fn repair_button_style() -> RepairButtonStyle {
    RepairButtonStyle {
        fill: egui::Color32::from_rgb(255, 255, 255),
        stroke_color: egui::Color32::from_rgb(137, 156, 177),
        text_color: egui::Color32::from_rgb(61, 79, 97),
        min_width: 148.0,
        min_height: 34.0,
        text_size: 15.0,
    }
}

fn page_background_fill() -> egui::Color32 {
    egui::Color32::from_rgb(248, 249, 250)
}

fn page_header_fill() -> egui::Color32 {
    egui::Color32::from_rgb(240, 243, 246)
}

fn text_primary_color() -> egui::Color32 {
    egui::Color32::from_rgb(45, 55, 72)
}

fn text_secondary_color() -> egui::Color32 {
    egui::Color32::from_rgb(107, 114, 128)
}

fn selected_row_fill() -> egui::Color32 {
    egui::Color32::from_rgb(238, 242, 247)
}

fn card_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(egui::Color32::WHITE)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(226, 232, 240)))
        .inner_margin(egui::Margin::same(16))
        .corner_radius(12.0)
}

fn success_tone() -> StatusTone {
    StatusTone {
        fill: egui::Color32::from_rgb(231, 244, 241),
        text: egui::Color32::from_rgb(34, 82, 73),
        stroke: egui::Color32::from_rgb(201, 227, 219),
    }
}

fn info_tone() -> StatusTone {
    StatusTone {
        fill: egui::Color32::from_rgb(241, 245, 249),
        text: egui::Color32::from_rgb(51, 65, 85),
        stroke: egui::Color32::from_rgb(226, 232, 240),
    }
}

fn error_tone() -> StatusTone {
    StatusTone {
        fill: egui::Color32::from_rgb(254, 242, 242),
        text: egui::Color32::from_rgb(153, 27, 27),
        stroke: egui::Color32::from_rgb(252, 210, 210),
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
        "已完成本次合并，成功处理 {} 条会话，更新了 {} 个会话文件。备份已保存到 {}",
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
    fn merge_button_uses_professional_primary_style() {
        let style = merge_button_style(true);

        assert_eq!(style.fill, egui::Color32::from_rgb(31, 87, 122));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(22, 63, 88));
        assert_eq!(style.text_color, egui::Color32::WHITE);
        assert_eq!(style.min_width, 148.0);
        assert_eq!(style.min_height, 38.0);
    }

    #[test]
    fn repair_button_uses_subtle_secondary_style() {
        let style = repair_button_style();

        assert_eq!(style.fill, egui::Color32::from_rgb(255, 255, 255));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(137, 156, 177));
        assert_eq!(style.text_color, egui::Color32::from_rgb(61, 79, 97));
        assert_eq!(style.min_width, 148.0);
        assert_eq!(style.min_height, 34.0);
        assert_eq!(style.text_size, 15.0);
    }

    #[test]
    fn success_status_tone_uses_soft_teal_surface() {
        let tone = success_tone();
        assert_eq!(tone.fill, egui::Color32::from_rgb(231, 244, 241));
        assert_eq!(tone.text, egui::Color32::from_rgb(34, 82, 73));
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
            "已完成本次合并，成功处理 90 条会话，更新了 90 个会话文件。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776492506"
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
    fn control_action_layout_keeps_buttons_right_aligned() {
        let layout = control_action_column_layout();

        assert_eq!(layout.main_dir, egui::Direction::TopDown);
        assert_eq!(layout.cross_align, egui::Align::Max);
        assert!(!layout.main_wrap);
    }
}
