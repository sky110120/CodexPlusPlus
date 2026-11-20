//! 原生输入预测（实验）的运行时投影；只输出布尔值，不把供应商地址或凭据送入补丁。

use crate::settings::BackendSettings;
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeSettings {
    pub enhancements_enabled: bool,
    pub codex_app_native_predictions: bool,
    /// 当前生效配置是否为本机自定义 API；与原生 Fast 共用同一份判定。
    pub native_predictions_local_api_configured: bool,
}

pub(crate) fn runtime_settings(settings: &BackendSettings) -> RuntimeSettings {
    runtime_settings_with_local_api(
        settings,
        crate::native_fast::local_api_configured_from_home(
            &crate::codex_home::default_codex_home_dir(),
        ),
    )
}

pub(crate) fn runtime_settings_with_local_api(
    settings: &BackendSettings,
    local_api_configured: bool,
) -> RuntimeSettings {
    RuntimeSettings {
        enhancements_enabled: settings.enhancements_enabled,
        codex_app_native_predictions: settings.codex_app_native_predictions,
        native_predictions_local_api_configured: local_api_configured,
    }
}

/// Fast 和输入预测共享模块通道，分别按各自的开关和适用条件启用。
pub(crate) fn native_modules_requested(settings: &BackendSettings) -> bool {
    native_modules_requested_in_home(settings, &crate::codex_home::default_codex_home_dir())
}

pub(crate) fn native_modules_requested_in_home(settings: &BackendSettings, home: &Path) -> bool {
    (settings.enhancements_enabled && settings.codex_app_native_predictions)
        || crate::native_fast::native_module_requested_in_home(settings, home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projection_contains_only_feature_flags() {
        let mut settings = BackendSettings::default();
        assert!(!settings.codex_app_native_predictions);
        assert!(!native_modules_requested(&settings));
        settings.codex_app_native_predictions = true;
        assert!(native_modules_requested(&settings));
        settings.enhancements_enabled = false;
        assert!(!native_modules_requested(&settings));
        settings.enhancements_enabled = true;
        assert_eq!(
            serde_json::to_value(runtime_settings_with_local_api(&settings, true)).unwrap(),
            json!({
                "enhancementsEnabled": true,
                "codexAppNativePredictions": true,
                "nativePredictionsLocalApiConfigured": true,
            })
        );
    }
}
