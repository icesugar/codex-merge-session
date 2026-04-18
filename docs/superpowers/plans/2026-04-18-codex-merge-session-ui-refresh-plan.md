# codex-merge-session UI Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `codex-merge-session` 主界面重做为更清晰的专业工具风界面，同时保持现有合并与修复逻辑不变。

**Architecture:** 继续以 `src/app.rs` 作为主界面入口，把视觉样式、卡片布局、状态块和按钮呈现抽成更清晰的 UI 辅助函数。必要时在 `src/main.rs` 设置全局 `egui` 视觉参数，避免页面继续依赖默认外观。

**Tech Stack:** Rust, eframe/egui, cargo test, cargo build --release

---

### Task 1: 锁定新的视觉常量与布局约束

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: 写失败测试，锁定主按钮、次按钮和右对齐按钮列的新样式**

新增或修改以下断言：

```rust
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
```

- [ ] **Step 2: 运行测试，确认先失败**

Run: `cargo test merge_button_uses_professional_primary_style --lib`

Expected: FAIL，因为现有主按钮仍是旧颜色和旧尺寸。

- [ ] **Step 3: 实现新的按钮样式与按钮列布局辅助函数**

```rust
fn control_action_column_layout() -> egui::Layout {
    egui::Layout::top_down(egui::Align::Max)
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
```

- [ ] **Step 4: 运行测试，确认通过**

Run: `cargo test merge_button_uses_professional_primary_style --lib`

Expected: PASS

### Task 2: 重做主页面卡片布局

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: 写失败测试，锁定新的页面用语和结果区风格入口**

新增或修改以下测试：

```rust
#[test]
fn merge_button_label_is_shortened_for_toolbar_layout() {
    let button = merge_button(true);
    let debug_text = format!("{button:?}");
    assert!(debug_text.contains("开始合并"));
}
```

- [ ] **Step 2: 运行测试，确认先失败**

Run: `cargo test merge_button_label_is_shortened_for_toolbar_layout --lib`

Expected: FAIL，因为当前文案还是“开始合并会话”。

- [ ] **Step 3: 重做 `update()`，将页面改为摘要栏 + 双卡片 + 结果卡**

要点：

```rust
egui::TopBottomPanel::top("summary")
    .frame(egui::Frame::default().fill(page_header_fill()))
    .show(ctx, |ui| {
        // 标题与弱化说明
    });

egui::CentralPanel::default()
    .frame(egui::Frame::default().fill(page_background_fill()))
    .show(ctx, |ui| {
        // 左右卡片
        // 底部结果状态卡
    });
```

页面结构包括：

- `Provider 概览` 卡片
- `会话操作` 卡片
- `执行结果与错误` 卡片

- [ ] **Step 4: 运行局部测试，确认新文案和布局辅助函数通过**

Run: `cargo test --lib`

Expected: PASS

### Task 3: 重做 provider 列表、预览信息和状态卡

**Files:**
- Modify: `src/app.rs`

- [ ] **Step 1: 写失败测试，锁定状态卡颜色和提示卡颜色**

新增测试：

```rust
#[test]
fn success_status_tone_uses_soft_teal_surface() {
    let tone = success_status_tone();
    assert_eq!(tone.fill, egui::Color32::from_rgb(231, 244, 241));
    assert_eq!(tone.text, egui::Color32::from_rgb(34, 82, 73));
}
```

- [ ] **Step 2: 运行测试，确认先失败**

Run: `cargo test success_status_tone_uses_soft_teal_surface --lib`

Expected: FAIL，因为当前还没有状态色辅助函数。

- [ ] **Step 3: 实现卡片 UI 辅助函数和状态块**

包括：

- provider 摘要行
- 行高更高的 provider 列表
- 当前目标 provider 高亮
- 预览信息块
- 无可合并会话提示条
- 成功 / 提示 / 错误三类状态卡

- [ ] **Step 4: 运行完整测试**

Run: `cargo test`

Expected: PASS，现有存储逻辑测试不受影响。

### Task 4: 收尾和构建验证

**Files:**
- Modify: `src/app.rs`
- Modify: `src/main.rs`（仅当需要全局视觉初始化时）

- [ ] **Step 1: 调整确认合并弹窗，使主次按钮样式与主界面一致**

```rust
egui::Window::new("确认合并")
    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
    .resizable(false)
    .show(ctx, |ui| {
        // 统一标题、正文、按钮样式
    });
```

- [ ] **Step 2: 运行完整测试**

Run: `cargo test`

Expected: PASS

- [ ] **Step 3: 运行 release 构建**

Run: `cargo build --release`

Expected: PASS；若默认 release 因 exe 占用失败，则改跑：

Run: `CARGO_TARGET_DIR=target-verify cargo build --release`

Expected: PASS
