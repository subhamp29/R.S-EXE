use crate::commands::CommandResult;
use tauri::Window;

#[tauri::command]
pub fn window_minimize(window: Window) -> CommandResult<bool> {
    match window.minimize() {
        Ok(_) => CommandResult::success(true),
        Err(e) => CommandResult::fail(format!("Failed to minimize window: {}", e)),
    }
}

#[tauri::command]
pub fn window_maximize(window: Window) -> CommandResult<bool> {
    match window.is_maximized() {
        Ok(is_max) => {
            if is_max {
                match window.unmaximize() {
                    Ok(_) => CommandResult::success(false),
                    Err(e) => CommandResult::fail(format!("Failed to restore window: {}", e)),
                }
            } else {
                match window.maximize() {
                    Ok(_) => CommandResult::success(true),
                    Err(e) => CommandResult::fail(format!("Failed to maximize window: {}", e)),
                }
            }
        }
        Err(e) => CommandResult::fail(format!("Failed to check window state: {}", e)),
    }
}

#[tauri::command]
pub fn window_close(window: Window) -> CommandResult<bool> {
    match window.close() {
        Ok(_) => CommandResult::success(true),
        Err(e) => CommandResult::fail(format!("Failed to close window: {}", e)),
    }
}
