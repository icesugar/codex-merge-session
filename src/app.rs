use std::path::PathBuf;

use eframe::egui;

use crate::backup_registry::BackupEntry;
use crate::codex_store::{CodexStore, MergePreview, MergeReport, RepairReport, ScanResult};
use crate::provider_scan::ProviderOption;

pub struct CodexMergeApp {
    store: CodexStore,
    scan_result: Option<ScanResult>,
    backup_entries: Vec<BackupEntry>,
    selected_target_provider: String,
    manual_provider_input: String,
    preview: Option<MergePreview>,
    show_confirm_dialog: bool,
    show_restore_backup_dialog: Option<PathBuf>,
    show_delete_backup_dialog: Option<PathBuf>,
    repair_blocking_codex_pids: Vec<u32>,
    last_error: Option<String>,
    last_report: Option<RunReport>,
}

#[derive(Debug, Clone)]
enum RunReport {
    Merge(MergeReport),
    Repair(RepairReport),
    Info(String),
}

impl CodexMergeApp {
    pub fn new(store: CodexStore) -> Self {
        let mut app = Self {
            store,
            scan_result: None,
            backup_entries: Vec::new(),
            selected_target_provider: String::new(),
            manual_provider_input: String::new(),
            preview: None,
            show_confirm_dialog: false,
            show_restore_backup_dialog: None,
            show_delete_backup_dialog: None,
            repair_blocking_codex_pids: Vec::new(),
            last_error: None,
            last_report: None,
        };
        app.refresh_scan();
        app
    }

    fn refresh_scan(&mut self) {
        match self.store.scan() {
            Ok(scan) => {
                let provider_names = selectable_provider_names(&scan);
                if self.selected_target_provider.is_empty()
                    || !provider_names.contains(&self.selected_target_provider)
                {
                    self.selected_target_provider = scan.current_provider.clone();
                }
                self.scan_result = Some(scan);
                match self.store.list_backups() {
                    Ok(backups) => {
                        self.backup_entries = backups;
                        self.last_error = None;
                        self.refresh_preview();
                    }
                    Err(error) => {
                        self.backup_entries.clear();
                        self.last_error = Some(format!("{error:#}"));
                    }
                }
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
                self.repair_blocking_codex_pids.clear();
                self.refresh_scan();
                self.last_error = if report.errors.is_empty() {
                    None
                } else {
                    Some(report.errors.join("\n"))
                };
                self.last_report = Some(RunReport::Repair(report));
            }
            Err(error) => {
                let error_text = format!("{error:#}");
                self.repair_blocking_codex_pids.clear();
                if error_text.contains("检测到 codex.exe 正在运行") {
                    if let Ok(mut pids) = self.store.running_codex_pids() {
                        pids.sort_unstable();
                        pids.dedup();
                        self.repair_blocking_codex_pids = pids;
                    }
                }
                self.last_error = Some(error_text);
            }
        }
    }

    fn stop_codex_and_retry_repair(&mut self) {
        let target_pids = self.repair_blocking_codex_pids.clone();
        match self.store.terminate_codex_pids(&target_pids) {
            Ok(remaining_pids) if remaining_pids.is_empty() => {
                self.repair_blocking_codex_pids.clear();
                self.execute_repair();
            }
            Ok(mut remaining_pids) => {
                remaining_pids.sort_unstable();
                remaining_pids.dedup();
                let pid_text = remaining_pids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                self.repair_blocking_codex_pids = remaining_pids;
                self.last_error = Some(format!(
                    "结束 codex.exe 失败，请手动关闭后再试。PID: {pid_text}"
                ));
            }
            Err(error) => {
                self.last_error = Some(format!("{error:#}"));
            }
        }
    }

    fn add_manual_provider(&mut self) {
        let provider = self.manual_provider_input.trim().to_string();
        if provider.is_empty() {
            return;
        }

        match self.store.add_manual_provider(&provider) {
            Ok(()) => {
                self.manual_provider_input.clear();
                self.refresh_scan();
                self.last_report = Some(RunReport::Info(format!(
                    "已添加手动 provider：{provider}"
                )));
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }

    fn remove_manual_provider(&mut self, provider: &str) {
        match self.store.remove_manual_provider(provider) {
            Ok(()) => {
                self.refresh_scan();
                self.last_report = Some(RunReport::Info(format!(
                    "已移除手动 provider：{provider}"
                )));
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }

    fn restore_backup(&mut self, backup_dir: &PathBuf) {
        match self.store.restore_backup(backup_dir) {
            Ok(()) => {
                self.show_restore_backup_dialog = None;
                self.refresh_scan();
                self.last_report = Some(RunReport::Info(format!(
                    "已恢复备份：{}",
                    backup_dir.display()
                )));
            }
            Err(error) => self.last_error = Some(format!("{error:#}")),
        }
    }

    fn delete_backup(&mut self, backup_dir: &PathBuf) {
        match self.store.delete_backup(backup_dir) {
            Ok(()) => {
                self.show_delete_backup_dialog = None;
                self.refresh_scan();
                self.last_report = Some(RunReport::Info(format!(
                    "已删除备份：{}",
                    backup_dir.display()
                )));
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
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Codex 根目录:").color(text_secondary_color()),
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
                let mut pending_backup_restore = None;
                let mut pending_backup_delete = None;
                ui.columns(2, |columns| {
                    columns[0].set_min_height(left_column_min_height());
                    columns[1].set_min_height(right_column_min_height());
                    columns[0].add_space(4.0);
                    columns[0].group(|ui| {
                        ui.set_min_height(left_column_min_height());
                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.heading(egui::RichText::new("Provider 概览").size(16.0).strong());
                            ui.add_space(8.0);

                            if let Some(scan) = &self.scan_result {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "已发现 {} 个 provider",
                                        scan.provider_options.len()
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
                                            egui::RichText::new("来源")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.label(
                                            egui::RichText::new("rollout")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.label(
                                            egui::RichText::new("SQLite")
                                                .color(text_secondary_color())
                                                .size(13.0),
                                        );
                                        ui.end_row();

                                        for option in &scan.provider_options {
                                            let is_selected =
                                                option.id == self.selected_target_provider;
                                            let row_frame = if is_selected {
                                                egui::Frame::default().fill(selected_row_fill())
                                            } else {
                                                egui::Frame::default()
                                            };

                                            row_frame.show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(if option.is_current {
                                                        format!("{} (当前)", option.id)
                                                    } else {
                                                        option.id.clone()
                                                    })
                                                    .color(text_primary_color())
                                                    .size(14.0),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format_provider_sources(option))
                                                        .color(text_secondary_color())
                                                        .size(13.0),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}/{}",
                                                        option.rollout_active_count,
                                                        option.rollout_archived_count
                                                    ))
                                                    .color(text_primary_color())
                                                    .size(14.0),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{}/{}",
                                                        option.sqlite_active_count,
                                                        option.sqlite_archived_count
                                                    ))
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

                        if backup_list_column_index() == 0 {
                            ui.add_space(12.0);
                            render_backup_list_card(
                                ui,
                                &self.backup_entries,
                                &mut pending_backup_restore,
                                &mut pending_backup_delete,
                            );
                        }
                    });

                    columns[1].add_space(4.0);
                    columns[1].group(|ui| {
                        ui.set_min_height(right_column_min_height());
                        let provider_names = self
                            .scan_result
                            .as_ref()
                            .map(selectable_provider_names)
                            .unwrap_or_default();
                        let manual_providers = self
                            .scan_result
                            .as_ref()
                            .map(manual_provider_names)
                            .unwrap_or_default();
                        let can_merge = can_merge_preview(self.preview.as_ref());
                        let mut pending_manual_remove = None;

                        card_frame().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(right_column_min_height() - 32.0);
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    ui.heading(egui::RichText::new("会话操作").size(16.0).strong());
                                });
                                let remaining_width = ui.available_width();
                                ui.allocate_ui_with_layout(
                                    egui::vec2(remaining_width, 0.0),
                                    control_action_column_layout(),
                                    |ui| {
                                        if ui
                                            .add_enabled_ui(can_merge, |ui| {
                                                ui.add_sized(action_button_size(), merge_button(can_merge))
                                            })
                                            .inner
                                            .clicked()
                                        {
                                            self.show_confirm_dialog = true;
                                        }
                                        ui.add_space(8.0);
                                        if ui
                                            .add_sized(action_button_size(), repair_button())
                                            .clicked()
                                        {
                                            self.execute_repair();
                                        }
                                        if !self.repair_blocking_codex_pids.is_empty() {
                                            ui.add_space(8.0);
                                            if ui
                                                .add_sized(action_button_size(), end_codex_button())
                                                .clicked()
                                            {
                                                self.stop_codex_and_retry_repair();
                                            }
                                        }
                                    },
                                );
                            });

                            if !self.repair_blocking_codex_pids.is_empty() {
                                ui.add_space(12.0);
                                error_tone().show(ui, |ui| {
                                    let pid_text = self
                                        .repair_blocking_codex_pids
                                        .iter()
                                        .map(u32::to_string)
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "检测到 codex.exe 正在运行。点击“结束Codex”后会自动再次尝试修复。PID: {pid_text}"
                                        ))
                                        .color(error_tone().text),
                                    );
                                });
                            }

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

                            ui.label(
                                egui::RichText::new("手动 provider")
                                    .color(text_primary_color())
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.manual_provider_input)
                                        .hint_text("输入 provider id")
                                        .desired_width(ui.available_width() - 120.0),
                                );
                                if ui.add(primary_button("添加")).clicked() {
                                    self.add_manual_provider();
                                }
                            });
                            ui.add_space(8.0);
                            if manual_providers.is_empty() {
                                info_tone().show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("暂未添加手动 provider。")
                                            .color(info_tone().text),
                                    );
                                });
                            } else {
                                ui.horizontal_wrapped(|ui| {
                                    for provider in &manual_providers {
                                        ui.group(|ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(provider)
                                                        .color(text_primary_color())
                                                        .size(13.0),
                                                );
                                                if ui.add(secondary_button("移除")).clicked() {
                                                    pending_manual_remove = Some(provider.clone());
                                                }
                                            });
                                        });
                                    }
                                });
                            }

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
                                        ui.label(egui::RichText::new("将更新 config:").color(text_secondary_color()));
                                        ui.label(
                                            egui::RichText::new(if preview.will_update_config { "是" } else { "否" })
                                                .color(text_primary_color())
                                                .strong(),
                                        );
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
                                    ui.label(egui::RichText::new("当前没有可合并的会话、文件，也没有待更新的 config。").color(info_tone().text));
                                });
                            }
                        });

                        if let Some(provider) = pending_manual_remove {
                            self.remove_manual_provider(&provider);
                        }
                    });
                });

                if let Some(path) = pending_backup_restore {
                    self.show_restore_backup_dialog = Some(path);
                }
                if let Some(path) = pending_backup_delete {
                    self.show_delete_backup_dialog = Some(path);
                }

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
                        ui.label(
                            egui::RichText::new(format!(
                                "目标 provider: {}",
                                preview.target_provider
                            ))
                            .color(text_primary_color()),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "将同步 {} 条线程、{} 个会话文件，涉及 {} 个源 provider。",
                                preview.affected_thread_ids.len(),
                                preview.affected_rollout_paths.len(),
                                preview.source_providers.len()
                            ))
                            .color(text_primary_color()),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "将更新 config.toml: {}",
                                if preview.will_update_config { "是" } else { "否" }
                            ))
                            .color(text_primary_color()),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "备份将写入: {}",
                                preview.backup_dir.display()
                            ))
                            .color(text_secondary_color())
                            .size(13.0),
                        );
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

        if let Some(path) = self.show_restore_backup_dialog.clone() {
            egui::Window::new("确认恢复备份")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .default_width(420.0)
                .movable(false)
                .resizable(false)
                .frame(card_frame())
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(format!("将恢复备份: {}", path.display()))
                            .color(text_primary_color()),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("恢复会覆盖当前同名文件，请确认 Codex 已关闭。")
                            .color(text_secondary_color()),
                    );
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(primary_button("确认恢复")).clicked() {
                            self.restore_backup(&path);
                        }
                        ui.add_space(8.0);
                        if ui.add(secondary_button("取消")).clicked() {
                            self.show_restore_backup_dialog = None;
                        }
                    });
                });
        }

        if let Some(path) = self.show_delete_backup_dialog.clone() {
            egui::Window::new("确认删除备份")
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .fixed_size(delete_backup_dialog_size())
                .movable(false)
                .resizable(false)
                .frame(card_frame())
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("即将删除以下备份目录")
                                .color(text_secondary_color())
                                .size(13.0),
                        );
                        ui.add_space(8.0);
                        info_tone().show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(path.display().to_string())
                                        .monospace()
                                        .color(text_primary_color()),
                                )
                                .wrap(),
                            );
                        });
                        ui.add_space(12.0);
                        error_tone().show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "删除后无法通过本工具直接恢复，建议确认不再需要该备份后再继续。",
                                )
                                .color(error_tone().text),
                            );
                        });
                    });
                    ui.add_space(16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(primary_button("确认删除")).clicked() {
                            self.delete_backup(&path);
                        }
                        ui.add_space(8.0);
                        if ui.add(secondary_button("取消")).clicked() {
                            self.show_delete_backup_dialog = None;
                        }
                    });
                });
        }
    }
}

pub fn window_title() -> &'static str {
    "Codex Merge Session"
}

pub fn default_viewport_size() -> [f32; 2] {
    [1120.0, 860.0]
}

fn merge_button(can_merge: bool) -> egui::Button<'static> {
    let style = merge_button_style(can_merge);

    egui::Button::new(
        egui::RichText::new(merge_button_label())
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
        egui::RichText::new(repair_button_label())
            .size(style.text_size)
            .strong()
            .color(style.text_color),
    )
    .fill(style.fill)
    .stroke(egui::Stroke::new(1.0, style.stroke_color))
    .corner_radius(8.0)
    .min_size(egui::vec2(style.min_width, style.min_height))
}

fn end_codex_button() -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(end_codex_button_label())
            .size(14.0)
            .strong()
            .color(egui::Color32::WHITE),
    )
    .fill(egui::Color32::from_rgb(180, 83, 9))
    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(146, 64, 14)))
    .corner_radius(8.0)
    .min_size(action_button_size())
}

fn primary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(15.0)
            .strong()
            .color(egui::Color32::WHITE),
    )
    .fill(egui::Color32::from_rgb(31, 87, 122))
    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(22, 63, 88)))
    .corner_radius(8.0)
    .min_size(egui::vec2(120.0, 36.0))
}

fn secondary_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .size(14.0)
            .color(egui::Color32::from_rgb(61, 79, 97)),
    )
    .fill(egui::Color32::WHITE)
    .stroke(egui::Stroke::new(
        1.0,
        egui::Color32::from_rgb(137, 156, 177),
    ))
    .corner_radius(8.0)
    .min_size(egui::vec2(100.0, 32.0))
}

fn control_action_column_layout() -> egui::Layout {
    egui::Layout::top_down(egui::Align::Max)
}

fn merge_button_label() -> &'static str {
    "开始合并"
}

fn repair_button_label() -> &'static str {
    "尝试修复"
}

fn end_codex_button_label() -> &'static str {
    "结束Codex"
}

fn backup_list_column_index() -> usize {
    0
}

fn delete_backup_dialog_size() -> egui::Vec2 {
    egui::vec2(560.0, 240.0)
}

fn action_button_size() -> egui::Vec2 {
    egui::vec2(160.0, 36.0)
}

fn left_column_min_height() -> f32 {
    640.0
}

fn right_column_min_height() -> f32 {
    640.0
}

fn selectable_provider_names(scan: &ScanResult) -> Vec<String> {
    scan.provider_options
        .iter()
        .map(|option| option.id.clone())
        .collect()
}

fn manual_provider_names(scan: &ScanResult) -> Vec<String> {
    scan.provider_options
        .iter()
        .filter(|option| option.from_manual)
        .map(|option| option.id.clone())
        .collect()
}

fn can_merge_preview(preview: Option<&MergePreview>) -> bool {
    preview.is_some_and(|preview| {
        preview.will_update_config
            || !preview.affected_thread_ids.is_empty()
            || !preview.affected_rollout_paths.is_empty()
    })
}

fn format_provider_sources(option: &ProviderOption) -> String {
    let mut sources = Vec::new();
    if option.from_config {
        sources.push("config");
    }
    if option.from_rollout {
        sources.push("rollout");
    }
    if option.from_sqlite {
        sources.push("SQLite");
    }
    if option.from_manual {
        sources.push("manual");
    }
    if sources.is_empty() {
        return "-".to_string();
    }
    sources.join(" / ")
}

fn render_backup_list_card(
    ui: &mut egui::Ui,
    backup_entries: &[BackupEntry],
    pending_backup_restore: &mut Option<PathBuf>,
    pending_backup_delete: &mut Option<PathBuf>,
) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.heading(egui::RichText::new("备份列表").size(16.0).strong());
        ui.add_space(10.0);

        if backup_entries.is_empty() {
            info_tone().show(ui, |ui| {
                ui.label(
                    egui::RichText::new("当前没有可用备份。")
                        .color(info_tone().text),
                );
            });
            return;
        }

        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for entry in backup_entries {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}{}",
                                if entry.kind == "merge" { "合并备份" } else { "修复备份" },
                                if entry.is_legacy_location {
                                    "（旧目录）"
                                } else {
                                    ""
                                }
                            ))
                            .color(text_primary_color())
                            .strong(),
                        );
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("路径: {}", entry.path.display()))
                                    .color(text_secondary_color())
                                    .size(13.0),
                            )
                            .wrap(),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.add(secondary_button("恢复")).clicked() {
                                *pending_backup_restore = Some(entry.path.clone());
                            }
                            if ui.add(secondary_button("删除")).clicked() {
                                *pending_backup_delete = Some(entry.path.clone());
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
            });
    });
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
            min_width: 160.0,
            min_height: 36.0,
        };
    }

    MergeButtonStyle {
        fill: egui::Color32::from_rgb(188, 198, 209),
        stroke_color: egui::Color32::from_rgb(164, 175, 188),
        text_color: egui::Color32::from_rgb(246, 248, 250),
        min_width: 160.0,
        min_height: 36.0,
    }
}

fn repair_button_style() -> RepairButtonStyle {
    RepairButtonStyle {
        fill: egui::Color32::from_rgb(255, 255, 255),
        stroke_color: egui::Color32::from_rgb(137, 156, 177),
        text_color: egui::Color32::from_rgb(61, 79, 97),
        min_width: 160.0,
        min_height: 36.0,
        text_size: 15.0,
    }
}

fn page_background_fill() -> egui::Color32 {
    egui::Color32::from_rgb(248, 249, 250)
}

// 页面头部背景色
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
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(226, 232, 240),
        ))
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
        RunReport::Info(message) => message.clone(),
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
        && report.recovered_workspace_root_count == 0
        && report.updated_thread_workspace_hint_count == 0
        && !report.rebuilt_session_index
    {
        return "最近一次修复检查完成：没有发现需要修复的数据。".to_string();
    }

    let mut parts = Vec::new();
    if report.normalized_cwd_count > 0 {
        parts.push(format!(
            "规范化了 {} 条线程路径",
            report.normalized_cwd_count
        ));
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
    if report.recovered_workspace_root_count > 0 {
        parts.push(format!(
            "补回了 {} 个项目",
            report.recovered_workspace_root_count
        ));
    }
    if report.updated_thread_workspace_hint_count > 0 {
        parts.push(format!(
            "补回了 {} 条线程项目映射",
            report.updated_thread_workspace_hint_count
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
    use crate::codex_store::ProviderSummary;
    use crate::provider_scan::ProviderOption;
    use std::path::PathBuf;

    #[test]
    fn merge_button_uses_professional_primary_style() {
        let style = merge_button_style(true);

        assert_eq!(style.fill, egui::Color32::from_rgb(31, 87, 122));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(22, 63, 88));
        assert_eq!(style.text_color, egui::Color32::WHITE);
        assert_eq!(style.min_width, 160.0);
        assert_eq!(style.min_height, 36.0);
    }

    #[test]
    fn repair_button_uses_subtle_secondary_style() {
        let style = repair_button_style();

        assert_eq!(style.fill, egui::Color32::from_rgb(255, 255, 255));
        assert_eq!(style.stroke_color, egui::Color32::from_rgb(137, 156, 177));
        assert_eq!(style.text_color, egui::Color32::from_rgb(61, 79, 97));
        assert_eq!(style.min_width, 160.0);
        assert_eq!(style.min_height, 36.0);
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
            recovered_workspace_root_count: 3,
            updated_thread_workspace_hint_count: 9,
            rebuilt_session_index: true,
            backup_dir: PathBuf::from(
                r"C:\Users\home127\.codex-merge-session\backups\1776494198-cwd-normalize",
            ),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_repair_report(&report),
            "最近一次修复已完成：规范化了 147 条线程路径，并清理了 12 个会话文件里的路径残留，并同步修复了 8 处项目状态路径，并补回了 3 个项目，并补回了 9 条线程项目映射，并重建了会话索引。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776494198-cwd-normalize"
        );
    }

    #[test]
    fn repair_report_message_is_human_friendly_for_noop() {
        let report = RepairReport {
            normalized_cwd_count: 0,
            normalized_rollout_file_count: 0,
            normalized_workspace_path_count: 0,
            recovered_workspace_root_count: 0,
            updated_thread_workspace_hint_count: 0,
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
            recovered_workspace_root_count: 0,
            updated_thread_workspace_hint_count: 0,
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
    fn repair_report_message_is_human_friendly_for_workspace_recovery_only() {
        let report = RepairReport {
            normalized_cwd_count: 0,
            normalized_rollout_file_count: 0,
            normalized_workspace_path_count: 0,
            recovered_workspace_root_count: 2,
            updated_thread_workspace_hint_count: 4,
            rebuilt_session_index: false,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1776494198"),
            rolled_back: false,
            errors: Vec::new(),
        };

        assert_eq!(
            format_repair_report(&report),
            "最近一次修复已完成：补回了 2 个项目，并补回了 4 条线程项目映射。备份已保存到 C:\\Users\\home127\\.codex-merge-session\\backups\\1776494198"
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

    #[test]
    fn window_title_uses_provider_sync_branding() {
        assert_eq!(window_title(), "Codex Merge Session");
    }

    #[test]
    fn end_codex_button_label_uses_compact_text() {
        assert_eq!(end_codex_button_label(), "结束Codex");
    }

    #[test]
    fn selectable_provider_names_use_provider_options() {
        let scan = ScanResult {
            current_provider: "openai".to_string(),
            provider_summaries: vec![ProviderSummary {
                name: "summary-only".to_string(),
                active_count: 1,
                archived_count: 0,
                latest_updated_at: Some(10),
            }],
            provider_options: vec![
                ProviderOption {
                    id: "manual-only".to_string(),
                    from_config: false,
                    from_rollout: false,
                    from_sqlite: false,
                    from_manual: true,
                    is_current: false,
                    rollout_active_count: 0,
                    rollout_archived_count: 0,
                    sqlite_active_count: 0,
                    sqlite_archived_count: 0,
                },
                ProviderOption {
                    id: "openai".to_string(),
                    from_config: true,
                    from_rollout: true,
                    from_sqlite: true,
                    from_manual: false,
                    is_current: true,
                    rollout_active_count: 1,
                    rollout_archived_count: 0,
                    sqlite_active_count: 1,
                    sqlite_archived_count: 0,
                },
            ],
        };

        assert_eq!(
            selectable_provider_names(&scan),
            vec!["manual-only".to_string(), "openai".to_string()]
        );
    }

    #[test]
    fn can_merge_preview_is_true_for_config_only_changes() {
        let preview = MergePreview {
            target_provider: "custom".to_string(),
            source_providers: Vec::new(),
            affected_thread_ids: Vec::new(),
            affected_rollout_paths: Vec::new(),
            will_update_config: true,
            backup_dir: PathBuf::from(r"C:\Users\home127\.codex-merge-session\backups\1"),
        };

        assert!(can_merge_preview(Some(&preview)));
    }

    #[test]
    fn backup_list_prefers_left_column() {
        assert_eq!(backup_list_column_index(), 0);
    }

    #[test]
    fn delete_backup_dialog_uses_compact_fixed_size() {
        assert_eq!(delete_backup_dialog_size(), egui::vec2(560.0, 240.0));
    }

    #[test]
    fn action_buttons_use_consistent_fixed_size() {
        assert_eq!(action_button_size(), egui::vec2(160.0, 36.0));
    }

    #[test]
    fn default_viewport_height_is_tall_enough_to_expose_result_panel() {
        assert_eq!(default_viewport_size(), [1120.0, 860.0]);
    }

    #[test]
    fn main_columns_share_same_target_height() {
        assert_eq!(left_column_min_height(), right_column_min_height());
    }
}
