//! 用量托盘 + 预算阈值系统通知（P3-2）。
//!
//! 托盘常驻显示本月花费/预算占比；每次真实 TokenUsage 落库后刷新，
//! 跨过 70% / 90% / 100% 阈值时发一次系统通知（同一自然月同一档只提醒一次）。

use crate::sessions::{Budget, SessionHistoryStore};
use serde::{Deserialize, Serialize};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_notification::NotificationExt;

/// setting 表里记录「本月已提醒到哪一档」的 key
const BUDGET_NOTIFY_STATE_KEY: &str = "budget_notify_state";

pub struct TrayState {
    // TrayIcon 被 drop 时图标会从系统托盘消失，必须持有
    _tray: TrayIcon<Wry>,
    usage_item: MenuItem<Wry>,
}

/// 已提醒状态：month_start 变了（跨月）自动归零
#[derive(Debug, Default, Serialize, Deserialize)]
struct BudgetNotifyState {
    month_start: i64,
    level: u8,
}

pub fn setup(app: &tauri::App) -> Result<(), String> {
    let usage_item = MenuItem::with_id(app, "helm-usage", "本月用量读取中…", false, None::<&str>)
        .map_err(|e| format!("创建托盘菜单项失败：{e}"))?;
    let show_item = MenuItem::with_id(app, "helm-show", "显示主窗口", true, None::<&str>)
        .map_err(|e| format!("创建托盘菜单项失败：{e}"))?;
    let usage_page_item =
        MenuItem::with_id(app, "helm-usage-page", "查看用量页", true, None::<&str>)
            .map_err(|e| format!("创建托盘菜单项失败：{e}"))?;
    let quit_item = MenuItem::with_id(app, "helm-quit", "退出 Helm", true, None::<&str>)
        .map_err(|e| format!("创建托盘菜单项失败：{e}"))?;
    let separator =
        PredefinedMenuItem::separator(app).map_err(|e| format!("创建托盘分隔线失败：{e}"))?;
    let menu = Menu::with_items(
        app,
        &[
            &usage_item,
            &separator,
            &show_item,
            &usage_page_item,
            &quit_item,
        ],
    )
    .map_err(|e| format!("创建托盘菜单失败：{e}"))?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "缺少应用图标".to_string())?;

    let tray = TrayIconBuilder::with_id("helm-tray")
        .icon(icon)
        .tooltip("Helm")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "helm-show" => show_main_window(app),
            "helm-usage-page" => {
                show_main_window(app);
                // 复用前端已有的跨页跳转事件通道
                let _ = tauri::Emitter::emit(app, "helm-navigate", "usage");
            }
            "helm-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("创建系统托盘失败：{e}"))?;

    app.manage(TrayState {
        _tray: tray,
        usage_item,
    });
    refresh_usage(app.handle());
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 托盘菜单文案：真实预算数据 → 一句话
pub fn usage_menu_text(budget: &Budget) -> String {
    if budget.monthly_limit > 0.0 {
        format!(
            "本月 ${:.2} / ${:.2}（{:.0}%）",
            budget.current_month_cost, budget.monthly_limit, budget.percentage
        )
    } else {
        format!("本月 ${:.2} · 未设预算", budget.current_month_cost)
    }
}

/// 计算本次花费跨过的最高阈值档（0 表示未过任何档）
pub fn threshold_level(current_cost: f64, monthly_limit: f64) -> u8 {
    if monthly_limit <= 0.0 {
        return 0;
    }
    let pct = current_cost / monthly_limit * 100.0;
    if pct >= 100.0 {
        100
    } else if pct >= 90.0 {
        90
    } else if pct >= 70.0 {
        70
    } else {
        0
    }
}

/// 同一自然月同一档只提醒一次；跨月或更高档才再次提醒
pub fn should_notify(stored_month: i64, stored_level: u8, month_start: i64, level: u8) -> bool {
    if level == 0 {
        return false;
    }
    if stored_month != month_start {
        return true;
    }
    level > stored_level
}

/// TokenUsage 落库 / 预算修改后调用：刷新托盘文案 + 检查阈值通知
pub fn refresh_usage(app: &AppHandle) {
    let Some(store) = app.try_state::<SessionHistoryStore>() else {
        return;
    };
    let Ok(budget) = store.get_budget() else {
        return;
    };
    if let Some(state) = app.try_state::<TrayState>() {
        let _ = state.usage_item.set_text(usage_menu_text(&budget));
    }
    maybe_notify_threshold(app, &store, &budget);
}

fn maybe_notify_threshold(app: &AppHandle, store: &SessionHistoryStore, budget: &Budget) {
    // 复用预算卡的提醒开关：关掉提醒 = 不发系统通知
    if !budget.alert_at_80 {
        return;
    }
    let level = threshold_level(budget.current_month_cost, budget.monthly_limit);
    if level == 0 {
        return;
    }
    let Ok(month_start) = store.current_month_start() else {
        return;
    };
    let stored = store
        .get_json_setting::<BudgetNotifyState>(BUDGET_NOTIFY_STATE_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    if !should_notify(stored.month_start, stored.level, month_start, level) {
        return;
    }

    let body = if level >= 100 {
        format!(
            "本月花费 ${:.2} 已达预算 ${:.2}，新任务已被阻止。",
            budget.current_month_cost, budget.monthly_limit
        )
    } else {
        format!(
            "本月花费 ${:.2}，已达预算 ${:.2} 的 {level}%。",
            budget.current_month_cost, budget.monthly_limit
        )
    };
    let _ = app
        .notification()
        .builder()
        .title("Helm 用量提醒")
        .body(body)
        .show();
    let _ = store.set_json_setting(
        BUDGET_NOTIFY_STATE_KEY,
        &BudgetNotifyState { month_start, level },
    );
}

#[cfg(test)]
mod tests {
    use super::{should_notify, threshold_level, usage_menu_text};
    use crate::sessions::Budget;

    #[test]
    fn threshold_level_matches_p3_2_tiers() {
        assert_eq!(threshold_level(0.0, 100.0), 0);
        assert_eq!(threshold_level(69.9, 100.0), 0);
        assert_eq!(threshold_level(70.0, 100.0), 70);
        assert_eq!(threshold_level(89.9, 100.0), 70);
        assert_eq!(threshold_level(90.0, 100.0), 90);
        assert_eq!(threshold_level(100.0, 100.0), 100);
        assert_eq!(threshold_level(250.0, 100.0), 100);
        // 未设预算永远不触发
        assert_eq!(threshold_level(999.0, 0.0), 0);
    }

    #[test]
    fn should_notify_once_per_tier_per_month() {
        let month = 1_750_000_000;
        // 首次跨档提醒
        assert!(should_notify(0, 0, month, 70));
        // 同月同档不重复
        assert!(!should_notify(month, 70, month, 70));
        // 同月升档要提醒
        assert!(should_notify(month, 70, month, 90));
        // 跨月归零重新提醒
        assert!(should_notify(month, 100, month + 2_678_400, 70));
        // 未过档不提醒
        assert!(!should_notify(month, 0, month, 0));
    }

    #[test]
    fn usage_menu_text_shows_budget_share_or_absence() {
        let with_budget = Budget {
            monthly_limit: 50.0,
            alert_at_80: true,
            stop_at_100: false,
            current_month_cost: 12.5,
            percentage: 25.0,
        };
        assert_eq!(usage_menu_text(&with_budget), "本月 $12.50 / $50.00（25%）");

        let without_budget = Budget {
            monthly_limit: 0.0,
            alert_at_80: true,
            stop_at_100: false,
            current_month_cost: 3.2,
            percentage: 0.0,
        };
        assert_eq!(usage_menu_text(&without_budget), "本月 $3.20 · 未设预算");
    }
}
