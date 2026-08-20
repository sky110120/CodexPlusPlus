use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::settings::{BackendSettings, DreamSkinThemeConfig};

const THEMES_DIR: &str = "dream-skin/themes";
const THEME_CONFIG_FILE: &str = "theme.json";
const THEME_CONFIG_LIMIT: u64 = 256 * 1024;
const THEME_AUXILIARY_LIMIT: u64 = 1024 * 1024;
const ACTIVE_THEME_DIR: &str = "dream-skin/theme";
const ACTIVE_STAGING_PREFIX: &str = ".stage-current-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DreamSkinThemeKind {
    Builtin,
    Stored,
    ActiveUnsaved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamSkinThemeSummary {
    pub key: String,
    pub id: String,
    pub name: String,
    pub preview_path: String,
    pub kind: DreamSkinThemeKind,
    pub builtin: bool,
    pub active: bool,
    pub modified: bool,
    #[serde(default)]
    pub damaged: bool,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamSkinThemeDraft {
    pub config: DreamSkinThemeConfig,
    pub image_path: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamSkinThemeLibrary {
    pub themes: Vec<DreamSkinThemeSummary>,
    pub active_draft: DreamSkinThemeDraft,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DreamSkinActivation {
    pub config: DreamSkinThemeConfig,
    pub active_image_path: String,
    pub staging_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DreamSkinRestoreAssessment {
    pub requires_decision: bool,
    pub can_save_active: bool,
    pub active_draft: Option<DreamSkinThemeDraft>,
    pub stable_theme_key: String,
}

pub fn list_dream_skin_themes(
    state_dir: &Path,
    settings: &BackendSettings,
) -> anyhow::Result<DreamSkinThemeLibrary> {
    let default_theme = DreamSkinThemeConfig::default();
    let active_config = settings.codex_app_dream_skin_theme_config.clone();
    let active_image_path = settings.codex_app_dream_skin_image_path.trim().to_string();
    let enabled = settings.codex_app_dream_skin_enabled;
    let mut warnings = Vec::new();
    let active_css_path = state_dir.join(ACTIVE_THEME_DIR).join("current.css");
    let builtin_active = enabled && active_matches_builtin(state_dir, settings);
    let mut themes = vec![DreamSkinThemeSummary {
        key: "builtin".to_string(),
        id: default_theme.id.clone(),
        name: default_theme.name.clone(),
        preview_path: String::new(),
        kind: DreamSkinThemeKind::Builtin,
        builtin: true,
        active: builtin_active,
        modified: false,
        damaged: false,
        error: String::new(),
    }];

    let themes_dir = state_dir.join(THEMES_DIR);
    if themes_dir.is_dir() {
        for entry in std::fs::read_dir(&themes_dir)
            .with_context(|| format!("failed to read {}", themes_dir.display()))?
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(format!("读取 Dream Skin 主题目录项失败：{error}"));
                    continue;
                }
            };
            match read_theme_summary(
                &entry.path(),
                enabled,
                &active_config,
                &active_image_path,
                &active_css_path,
            ) {
                Ok(Some(summary)) => themes.push(summary),
                Ok(None) => {}
                Err(error) => warnings.push(error.to_string()),
            }
        }
    }

    if enabled && !themes.iter().any(|theme| theme.active) {
        themes.push(DreamSkinThemeSummary {
            key: "active-unsaved".to_string(),
            id: active_config.id.clone(),
            name: active_config.name.clone(),
            preview_path: active_image_path.clone(),
            kind: DreamSkinThemeKind::ActiveUnsaved,
            builtin: false,
            active: true,
            modified: true,
            damaged: false,
            error: String::new(),
        });
    }

    themes[1..].sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(DreamSkinThemeLibrary {
        themes,
        active_draft: if builtin_active {
            DreamSkinThemeDraft {
                config: default_theme,
                image_path: String::new(),
                builtin: true,
            }
        } else {
            DreamSkinThemeDraft {
                config: active_config,
                image_path: active_image_path,
                builtin: false,
            }
        },
        warnings,
    })
}

fn read_theme_summary(
    directory: &Path,
    enabled: bool,
    active_config: &DreamSkinThemeConfig,
    active_image_path: &str,
    active_css_path: &Path,
) -> anyhow::Result<Option<DreamSkinThemeSummary>> {
    let metadata = std::fs::symlink_metadata(directory)
        .with_context(|| format!("failed to inspect {}", directory.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "已跳过不安全的 Dream Skin 主题路径：{}",
            directory.display()
        );
    }
    let Some(id) = directory.file_name().and_then(|value| value.to_str()) else {
        bail!(
            "已跳过名称无法识别的 Dream Skin 主题目录：{}",
            directory.display()
        );
    };
    if !valid_theme_id(id) {
        bail!("已跳过 ID 不安全的 Dream Skin 主题目录：{id}");
    }
    ensure_known_theme_directory(directory)
        .with_context(|| format!("已跳过包含未知或不安全内容的 Dream Skin 主题目录：{id}"))?;
    let active_id_matches = enabled && id == active_config.id;
    let config_path = directory.join(THEME_CONFIG_FILE);
    let config_metadata = match std::fs::symlink_metadata(&config_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(Some(damaged_theme_summary(
                id,
                id,
                format!("主题配置缺失：{error}"),
                active_id_matches,
            )));
        }
    };
    if !config_metadata.file_type().is_file()
        || config_metadata.file_type().is_symlink()
        || config_metadata.len() > THEME_CONFIG_LIMIT
    {
        return Ok(Some(damaged_theme_summary(
            id,
            id,
            "主题配置不是安全普通文件或超过 256 KiB".to_string(),
            active_id_matches,
        )));
    }
    let config: DreamSkinThemeConfig = match std::fs::read(&config_path)
        .context("读取主题配置失败")
        .and_then(|bytes| serde_json::from_slice(&bytes).context("主题配置 JSON 无效"))
    {
        Ok(config) => config,
        Err(error) => {
            return Ok(Some(damaged_theme_summary(
                id,
                id,
                error.to_string(),
                active_id_matches,
            )));
        }
    };
    if config.id != id || config.name.trim().is_empty() {
        return Ok(Some(damaged_theme_summary(
            id,
            id,
            "主题 ID 与目录不一致或名称为空".to_string(),
            active_id_matches,
        )));
    }
    if let Err(error) = validate_theme_auxiliary_files(directory) {
        return Ok(Some(damaged_theme_summary(
            id,
            &config.name,
            error.to_string(),
            active_id_matches,
        )));
    }
    let Some(image_path) = find_theme_image(directory) else {
        return Ok(Some(DreamSkinThemeSummary {
            key: format!("stored:{id}"),
            id: config.id,
            name: config.name,
            preview_path: String::new(),
            kind: DreamSkinThemeKind::Stored,
            builtin: false,
            active: false,
            modified: active_id_matches,
            damaged: true,
            error: "主题必须且只能包含一张受支持的普通图片".to_string(),
        }));
    };
    if let Err(error) = validate_stored_image(&image_path) {
        return Ok(Some(DreamSkinThemeSummary {
            key: format!("stored:{id}"),
            id: config.id,
            name: config.name,
            preview_path: String::new(),
            kind: DreamSkinThemeKind::Stored,
            builtin: false,
            active: false,
            modified: active_id_matches,
            damaged: true,
            error: error.to_string(),
        }));
    }
    let complete_match = active_id_matches
        && config == *active_config
        && image_matches_active(&image_path, active_image_path)
        && stored_css_matches_active_for_directory(directory, active_css_path);
    Ok(Some(DreamSkinThemeSummary {
        key: format!("stored:{id}"),
        id: config.id,
        name: config.name,
        preview_path: image_path.to_string_lossy().into_owned(),
        kind: DreamSkinThemeKind::Stored,
        builtin: false,
        active: complete_match,
        modified: active_id_matches && !complete_match,
        damaged: false,
        error: String::new(),
    }))
}

fn damaged_theme_summary(
    id: &str,
    name: &str,
    error: String,
    modified: bool,
) -> DreamSkinThemeSummary {
    DreamSkinThemeSummary {
        key: format!("stored:{id}"),
        id: id.to_string(),
        name: name.to_string(),
        preview_path: String::new(),
        kind: DreamSkinThemeKind::Stored,
        builtin: false,
        active: false,
        modified,
        damaged: true,
        error,
    }
}

fn find_theme_image(directory: &Path) -> Option<PathBuf> {
    let mut images = std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let Ok(metadata) = std::fs::symlink_metadata(path) else {
                return false;
            };
            metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && supported_image_extension(path)
        });
    let image = images.next()?;
    if images.next().is_some() {
        return None;
    }
    Some(image)
}

fn supported_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| {
            matches!(
                extension.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
}

fn stored_theme_image_extension(state_dir: &Path, source: &Path) -> Option<String> {
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            matches!(
                extension.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })?;
    let source = std::fs::canonicalize(source).ok()?;
    let themes_dir = std::fs::canonicalize(state_dir.join(THEMES_DIR)).ok()?;
    let theme_dir = source.parent()?;
    if theme_dir.parent()? != themes_dir {
        return None;
    }
    let theme_id = theme_dir.file_name()?.to_str()?;
    valid_theme_id(theme_id).then_some(extension)
}

fn valid_theme_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

pub fn create_dream_skin_theme_from_image(
    source: &Path,
    state_dir: &Path,
) -> anyhow::Result<DreamSkinThemeDraft> {
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Dream Skin");
    let base_id = slugify_theme_id(stem);
    let themes_dir = state_dir.join(THEMES_DIR);
    let mut id = base_id.clone();
    let mut suffix = 2;
    while themes_dir.join(&id).exists() {
        id = format!("{base_id}-{suffix}");
        suffix += 1;
    }
    let mut config = DreamSkinThemeConfig::default();
    config.id = id;
    config.name = stem.to_string();
    let draft = DreamSkinThemeDraft {
        config,
        image_path: source.to_string_lossy().into_owned(),
        builtin: false,
    };
    save_dream_skin_theme(state_dir, &draft)?;
    load_stored_dream_skin_theme(state_dir, &draft.config.id)
}

pub fn save_dream_skin_theme(
    state_dir: &Path,
    draft: &DreamSkinThemeDraft,
) -> anyhow::Result<DreamSkinThemeSummary> {
    validate_theme_draft(draft)?;
    let themes_dir = state_dir.join(THEMES_DIR);
    std::fs::create_dir_all(&themes_dir)
        .with_context(|| format!("failed to create {}", themes_dir.display()))?;
    reject_symlink(&themes_dir)?;

    let suffix = unique_suffix();
    let staging = themes_dir.join(format!(".stage-{}-{suffix}", draft.config.id));
    std::fs::create_dir(&staging).with_context(|| {
        format!(
            "failed to create theme staging directory {}",
            staging.display()
        )
    })?;

    let target = themes_dir.join(&draft.config.id);
    let staged = (|| -> anyhow::Result<()> {
        if target.exists() {
            copy_known_theme_auxiliary_files(&target, &staging)?;
        }
        if draft.image_path.trim().is_empty() {
            let (_, bytes) = crate::assets::dream_skin_default_image();
            crate::settings::atomic_write(&staging.join("image.png"), bytes)?;
        } else {
            crate::dream_skin::prepare_dream_skin_image_for_directory(
                Path::new(draft.image_path.trim()),
                &staging,
                "image",
            )?;
            copy_draft_safe_css(Path::new(draft.image_path.trim()), &staging)?;
        }
        let config = serde_json::to_vec_pretty(&draft.config)?;
        if config.len() as u64 > THEME_CONFIG_LIMIT {
            bail!("Dream Skin theme config exceeds 256 KiB");
        }
        crate::settings::atomic_write(&staging.join(THEME_CONFIG_FILE), &config)?;
        let image = find_theme_image(&staging)
            .context("Dream Skin theme staging must contain exactly one image")?;
        validate_stored_image(&image)?;
        validate_theme_auxiliary_files(&staging)?;
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = remove_known_theme_directory(&staging);
        return Err(error);
    }

    replace_theme_directory(&staging, &target, &suffix)?;
    let stored = load_stored_dream_skin_theme(state_dir, &draft.config.id)?;
    Ok(summary_from_draft(&stored, false, false))
}

pub fn save_dream_skin_theme_selection(
    state_dir: &Path,
    draft: &DreamSkinThemeDraft,
    source_key: &str,
    explicit_save: bool,
) -> anyhow::Result<DreamSkinThemeDraft> {
    let source_key = source_key.trim();
    if source_key == "builtin" {
        if !draft.builtin {
            bail!("built-in Dream Skin selection does not match draft");
        }
        if !explicit_save
            && draft.config == DreamSkinThemeConfig::default()
            && draft.image_path.trim().is_empty()
        {
            return Ok(draft.clone());
        }
        let unique = unique_stored_theme_draft(state_dir, draft);
        save_dream_skin_theme(state_dir, &unique)?;
        return load_stored_dream_skin_theme(state_dir, &unique.config.id);
    }
    if source_key == "active-unsaved" {
        if draft.builtin {
            bail!("active-unsaved Dream Skin draft cannot be built-in");
        }
        let unique = unique_stored_theme_draft(state_dir, draft);
        save_dream_skin_theme(state_dir, &unique)?;
        return load_stored_dream_skin_theme(state_dir, &unique.config.id);
    }
    if let Some(id) = source_key.strip_prefix("stored:") {
        if draft.builtin || id != draft.config.id || !valid_theme_id(id) {
            bail!("stored Dream Skin selection does not match draft");
        }
        load_stored_dream_skin_theme(state_dir, id)
            .context("selected Dream Skin theme is missing or damaged")?;
        save_dream_skin_theme(state_dir, draft)?;
        return load_stored_dream_skin_theme(state_dir, id);
    }
    bail!("Dream Skin source selection is missing or invalid")
}

pub fn save_validated_dream_skin_package(
    state_dir: &Path,
    package: &crate::dream_skin_package::ValidatedDreamSkinPackage,
) -> anyhow::Result<DreamSkinThemeSummary> {
    let config: DreamSkinThemeConfig = serde_json::from_value(package.theme.clone())
        .context("主题包 theme.json 与 Codex++ 主题配置不兼容")?;
    let draft = DreamSkinThemeDraft {
        config: config.clone(),
        image_path: String::new(),
        builtin: false,
    };
    validate_theme_draft(&draft)?;
    let config_bytes = serde_json::to_vec_pretty(&config)?;
    if config_bytes.len() as u64 > THEME_CONFIG_LIMIT {
        bail!("Dream Skin theme config exceeds 256 KiB");
    }

    let themes_dir = state_dir.join(THEMES_DIR);
    std::fs::create_dir_all(&themes_dir)
        .with_context(|| format!("failed to create {}", themes_dir.display()))?;
    reject_symlink(&themes_dir)?;
    let target = themes_dir.join(&config.id);
    if target.exists() {
        let metadata = std::fs::symlink_metadata(&target).context("无法检查同 ID 本地主题目录")?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            bail!("同 ID 本地主题目录不安全，已拒绝覆盖");
        }
        ensure_known_theme_directory(&target).context("同 ID 本地主题包含未知内容，已拒绝覆盖")?;
        if let Ok(existing) = load_stored_dream_skin_theme(state_dir, &config.id)
            && existing.config.id != config.id
        {
            bail!("同 ID 本地主题身份不一致，已拒绝覆盖");
        }
    }
    let suffix = unique_suffix();
    let staging = themes_dir.join(format!(".stage-{}-{suffix}", config.id));
    std::fs::create_dir(&staging).with_context(|| {
        format!(
            "failed to create Dream Skin package staging directory {}",
            staging.display()
        )
    })?;
    let staged = (|| -> anyhow::Result<()> {
        let image_extension = match package.image_name.as_str() {
            "background.webp" => "webp",
            "background.jpg" => "jpg",
            "background.png" => "png",
            other => bail!("unsupported Dream Skin package image: {other}"),
        };
        crate::settings::atomic_write(
            &staging.join(format!("image.{image_extension}")),
            &package.image_bytes,
        )?;
        crate::settings::atomic_write(&staging.join(THEME_CONFIG_FILE), &config_bytes)?;
        crate::settings::atomic_write(&staging.join("theme.css"), package.css.as_bytes())?;
        crate::settings::atomic_write(&staging.join("manifest.json"), &package.manifest_bytes)?;
        if let Some(license) = &package.license_bytes {
            crate::settings::atomic_write(&staging.join("LICENSE.txt"), license)?;
        }
        let image = find_theme_image(&staging)
            .context("Dream Skin package staging must contain exactly one image")?;
        validate_stored_image(&image)?;
        validate_theme_auxiliary_files(&staging)?;
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = remove_known_theme_directory(&staging);
        return Err(error);
    }
    if let Err(error) = replace_theme_directory(&staging, &target, &suffix) {
        let _ = remove_known_theme_directory(&staging);
        return Err(error);
    }
    let stored = load_stored_dream_skin_theme(state_dir, &config.id)?;
    Ok(summary_from_draft(&stored, false, false))
}

pub fn load_stored_dream_skin_theme(
    state_dir: &Path,
    id: &str,
) -> anyhow::Result<DreamSkinThemeDraft> {
    if !valid_theme_id(id) {
        bail!("invalid Dream Skin theme id");
    }
    let directory = state_dir.join(THEMES_DIR).join(id);
    let metadata = std::fs::symlink_metadata(&directory)
        .with_context(|| format!("Dream Skin theme not found: {id}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("Dream Skin theme path is not a safe directory");
    }
    ensure_known_theme_directory(&directory)?;
    let config_path = directory.join(THEME_CONFIG_FILE);
    let metadata = std::fs::symlink_metadata(&config_path)
        .with_context(|| format!("Dream Skin theme config not found: {id}"))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > THEME_CONFIG_LIMIT
    {
        bail!("invalid Dream Skin theme config");
    }
    let config: DreamSkinThemeConfig = serde_json::from_slice(&std::fs::read(&config_path)?)?;
    if config.id != id {
        bail!("Dream Skin theme id does not match directory");
    }
    validate_theme_auxiliary_files(&directory)?;
    let image = find_theme_image(&directory)
        .ok_or_else(|| anyhow::anyhow!("Dream Skin theme must contain exactly one image"))?;
    validate_stored_image(&image)?;
    let draft = DreamSkinThemeDraft {
        config,
        image_path: image.to_string_lossy().into_owned(),
        builtin: false,
    };
    validate_theme_draft(&draft)?;
    Ok(draft)
}

pub fn rename_dream_skin_theme(
    state_dir: &Path,
    id: &str,
    name: &str,
) -> anyhow::Result<DreamSkinThemeSummary> {
    let mut draft = load_stored_dream_skin_theme(state_dir, id)?;
    draft.config.name = name.trim().to_string();
    save_dream_skin_theme(state_dir, &draft)
}

pub fn prepare_dream_skin_activation(
    state_dir: &Path,
    draft: &DreamSkinThemeDraft,
) -> anyhow::Result<DreamSkinActivation> {
    let managed_dir = state_dir.join(ACTIVE_THEME_DIR);
    std::fs::create_dir_all(&managed_dir)
        .with_context(|| format!("failed to create {}", managed_dir.display()))?;
    reject_symlink(&managed_dir)?;
    let staging = managed_dir.join(format!("{ACTIVE_STAGING_PREFIX}{}", unique_suffix()));
    std::fs::create_dir(&staging)
        .with_context(|| format!("failed to create {}", staging.display()))?;

    let prepared = (|| -> anyhow::Result<DreamSkinActivation> {
        if draft.builtin {
            if draft.config != DreamSkinThemeConfig::default()
                || !draft.image_path.trim().is_empty()
            {
                bail!("invalid built-in Dream Skin theme draft");
            }
            return Ok(DreamSkinActivation {
                config: draft.config.clone(),
                active_image_path: String::new(),
                staging_dir: staging.clone(),
            });
        }
        validate_theme_draft(draft)?;
        let source = Path::new(draft.image_path.trim());
        let active_image = if let Some(extension) = stored_theme_image_extension(state_dir, source)
        {
            validate_stored_image(source)?;
            let destination = staging.join(format!("current.{extension}"));
            let bytes = std::fs::read(source)
                .with_context(|| format!("failed to read Dream Skin image {}", source.display()))?;
            crate::settings::atomic_write(&destination, &bytes).with_context(|| {
                format!("failed to store Dream Skin image {}", destination.display())
            })?;
            destination
        } else {
            crate::dream_skin::prepare_dream_skin_image_for_directory(source, &staging, "current")?
        };
        copy_activation_safe_css(source, &staging)?;
        let active_file_name = active_image
            .file_name()
            .context("prepared Dream Skin image has no file name")?;
        Ok(DreamSkinActivation {
            config: draft.config.clone(),
            active_image_path: managed_dir
                .join(active_file_name)
                .to_string_lossy()
                .into_owned(),
            staging_dir: staging.clone(),
        })
    })();
    if prepared.is_err() {
        let _ = remove_active_staging_directory(&staging);
    }
    prepared
}

pub fn commit_dream_skin_activation(
    state_dir: &Path,
    activation: &DreamSkinActivation,
) -> anyhow::Result<()> {
    let managed_dir = state_dir.join(ACTIVE_THEME_DIR);
    let expected_parent = activation
        .staging_dir
        .parent()
        .context("Dream Skin activation staging has no parent")?;
    let staging_name = activation
        .staging_dir
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if expected_parent != managed_dir || !staging_name.starts_with(ACTIVE_STAGING_PREFIX) {
        bail!("invalid Dream Skin activation staging directory");
    }
    ensure_known_active_directory(&activation.staging_dir)?;
    for path in managed_dream_skin_files(state_dir)? {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    for entry in std::fs::read_dir(&activation.staging_dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .context("Dream Skin activation entry has no file name")?;
        std::fs::rename(&path, managed_dir.join(name))
            .with_context(|| format!("failed to commit Dream Skin resource {}", path.display()))?;
    }
    std::fs::remove_dir(&activation.staging_dir)
        .with_context(|| format!("failed to remove {}", activation.staging_dir.display()))?;
    Ok(())
}

pub fn discard_dream_skin_activation(activation: &DreamSkinActivation) -> anyhow::Result<()> {
    remove_active_staging_directory(&activation.staging_dir)
}

pub fn managed_dream_skin_files(state_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let managed_dir = state_dir.join(ACTIVE_THEME_DIR);
    if !managed_dir.exists() {
        return Ok(Vec::new());
    }
    reject_symlink(&managed_dir)?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&managed_dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !is_known_active_file(&path) {
            if name.starts_with("current.") {
                bail!("Dream Skin active directory contains unsafe resource: {name}");
            }
            continue;
        }
        files.push(path);
    }
    files.sort();
    Ok(files)
}

pub fn delete_dream_skin_theme(
    state_dir: &Path,
    id: &str,
    active_id: Option<&str>,
    allow_damaged: bool,
) -> anyhow::Result<()> {
    if !valid_theme_id(id) {
        bail!("invalid Dream Skin theme id");
    }
    if active_id.is_some_and(|active| active == id) {
        bail!("current Dream Skin theme cannot be deleted");
    }
    let directory = state_dir.join(THEMES_DIR).join(id);
    let metadata = std::fs::symlink_metadata(&directory)
        .with_context(|| format!("Dream Skin theme not found: {id}"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("Dream Skin theme path is not a safe directory");
    }
    if id == DreamSkinThemeConfig::default().id {
        bail!("built-in Dream Skin theme cannot be deleted");
    }
    if !allow_damaged {
        load_stored_dream_skin_theme(state_dir, id)?;
    }
    remove_known_theme_directory(&directory)
}

pub fn assess_dream_skin_restore(
    state_dir: &Path,
    settings: &BackendSettings,
) -> anyhow::Result<DreamSkinRestoreAssessment> {
    let managed_files = managed_dream_skin_files(state_dir);
    let active_path = settings.codex_app_dream_skin_image_path.trim();
    let has_active_artifacts = !active_path.is_empty()
        || managed_files
            .as_ref()
            .map_or(true, |files| !files.is_empty());
    let stable_theme_key = if active_matches_builtin(state_dir, settings) {
        "builtin".to_string()
    } else if let Some(id) = matching_stored_theme_id(state_dir, settings) {
        format!("stored:{id}")
    } else {
        String::new()
    };
    let recoverable = !has_active_artifacts || !stable_theme_key.is_empty();
    let active_draft = if !active_path.is_empty() {
        let draft = DreamSkinThemeDraft {
            config: settings.codex_app_dream_skin_theme_config.clone(),
            image_path: active_path.to_string(),
            builtin: false,
        };
        if crate::dream_skin::is_managed_dream_skin_image(Path::new(active_path), state_dir)
            && validate_stored_image(Path::new(active_path)).is_ok()
            && validate_theme_draft(&draft).is_ok()
        {
            Some(draft)
        } else {
            None
        }
    } else {
        None
    };
    Ok(DreamSkinRestoreAssessment {
        requires_decision: has_active_artifacts && !recoverable,
        can_save_active: active_draft.is_some(),
        active_draft,
        stable_theme_key,
    })
}

fn copy_known_theme_auxiliary_files(source: &Path, destination: &Path) -> anyhow::Result<()> {
    ensure_known_theme_directory(source)?;
    for name in ["theme.css", "manifest.json", "LICENSE.txt"] {
        let path = source.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > THEME_AUXILIARY_LIMIT
        {
            bail!("Dream Skin auxiliary file is unsafe: {name}");
        }
        let bytes = std::fs::read(&path)?;
        if name == "theme.css" {
            validate_safe_css_bytes(&bytes)?;
        }
        crate::settings::atomic_write(&destination.join(name), &bytes)?;
    }
    Ok(())
}

fn validate_theme_auxiliary_files(directory: &Path) -> anyhow::Result<()> {
    for name in ["theme.css", "manifest.json", "LICENSE.txt"] {
        let path = directory.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("无法检查主题辅助文件：{name}"));
            }
        };
        let limit = if name == "theme.css" {
            THEME_CONFIG_LIMIT
        } else {
            THEME_AUXILIARY_LIMIT
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > limit
        {
            bail!("主题辅助文件不安全或超过大小限制：{name}");
        }
        if name == "theme.css" {
            validate_safe_css_bytes(&std::fs::read(&path)?)?;
        }
    }
    Ok(())
}

fn copy_draft_safe_css(image: &Path, destination: &Path) -> anyhow::Result<()> {
    let Some(parent) = image.parent() else {
        return Ok(());
    };
    let source = [parent.join("theme.css"), parent.join("current.css")]
        .into_iter()
        .find(|path| path.exists());
    if let Some(source) = source {
        copy_safe_css(&source, &destination.join("theme.css"))?;
    }
    Ok(())
}

fn copy_activation_safe_css(image: &Path, destination: &Path) -> anyhow::Result<()> {
    let Some(parent) = image.parent() else {
        return Ok(());
    };
    let source = [parent.join("theme.css"), parent.join("current.css")]
        .into_iter()
        .find(|path| path.exists());
    if let Some(source) = source {
        copy_safe_css(&source, &destination.join("current.css"))?;
    }
    Ok(())
}

fn copy_safe_css(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect Dream Skin Safe CSS {}", source.display()))?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > THEME_CONFIG_LIMIT
    {
        bail!("Dream Skin Safe CSS is not a safe ordinary file");
    }
    let bytes = std::fs::read(source)
        .with_context(|| format!("failed to read Dream Skin Safe CSS {}", source.display()))?;
    validate_safe_css_bytes(&bytes)?;
    crate::settings::atomic_write(destination, &bytes)
}

fn validate_safe_css_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    crate::dream_skin_package::validate_safe_css(
        std::str::from_utf8(bytes).context("Dream Skin Safe CSS is not valid UTF-8")?,
    )
}

fn unique_stored_theme_draft(state_dir: &Path, draft: &DreamSkinThemeDraft) -> DreamSkinThemeDraft {
    let themes_dir = state_dir.join(THEMES_DIR);
    let source = if valid_theme_id(&draft.config.id) {
        draft.config.id.clone()
    } else {
        slugify_theme_id(&draft.config.name)
    };
    let mut id = source.clone();
    let mut suffix = 2usize;
    while themes_dir.join(&id).exists() || id == DreamSkinThemeConfig::default().id {
        id = format!("{}-{suffix}", source.chars().take(56).collect::<String>());
        suffix += 1;
    }
    let mut unique = draft.clone();
    unique.builtin = false;
    unique.config.id = id;
    if unique.config.name == DreamSkinThemeConfig::default().name {
        unique.config.name = format!("{} 副本", unique.config.name);
    }
    unique
}

fn ensure_known_active_directory(directory: &Path) -> anyhow::Result<()> {
    reject_symlink(directory)?;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if !is_known_active_file(&path) {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            bail!("Dream Skin activation staging contains unknown entry: {name}");
        }
        if path.file_name().and_then(|value| value.to_str()) == Some("current.css") {
            let bytes = std::fs::read(&path)?;
            validate_safe_css_bytes(&bytes)?;
        }
    }
    Ok(())
}

fn remove_active_staging_directory(directory: &Path) -> anyhow::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    ensure_known_active_directory(directory)?;
    for entry in std::fs::read_dir(directory)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(directory)?;
    Ok(())
}

fn is_known_active_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    name == "current.css" || (name.starts_with("current.") && supported_image_extension(path))
}

fn active_matches_builtin(state_dir: &Path, settings: &BackendSettings) -> bool {
    if settings.codex_app_dream_skin_theme_config != DreamSkinThemeConfig::default() {
        return false;
    }
    let active_path = settings.codex_app_dream_skin_image_path.trim();
    let image_matches = active_path.is_empty()
        || read_safe_active_image(Path::new(active_path))
            .is_some_and(|bytes| bytes == crate::assets::dream_skin_default_image().1);
    image_matches
        && !state_dir
            .join(ACTIVE_THEME_DIR)
            .join("current.css")
            .exists()
}

fn matching_stored_theme_id(state_dir: &Path, settings: &BackendSettings) -> Option<String> {
    let themes_dir = state_dir.join(THEMES_DIR);
    let entries = std::fs::read_dir(&themes_dir).ok()?;
    let mut ids = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().map(ToString::to_string))
        .collect::<Vec<_>>();
    ids.sort_by(|left, right| {
        (left != &settings.codex_app_dream_skin_theme_config.id)
            .cmp(&(right != &settings.codex_app_dream_skin_theme_config.id))
            .then_with(|| left.cmp(right))
    });
    ids.into_iter().find(|id| {
        let Ok(stored) = load_stored_dream_skin_theme(state_dir, &id) else {
            return false;
        };
        stored.config == settings.codex_app_dream_skin_theme_config
            && image_matches_active(
                Path::new(&stored.image_path),
                settings.codex_app_dream_skin_image_path.trim(),
            )
            && stored_css_matches_active(state_dir, &id)
    })
}

fn stored_css_matches_active(state_dir: &Path, id: &str) -> bool {
    stored_css_matches_active_for_directory(
        &state_dir.join(THEMES_DIR).join(id),
        &state_dir.join(ACTIVE_THEME_DIR).join("current.css"),
    )
}

fn stored_css_matches_active_for_directory(directory: &Path, active: &Path) -> bool {
    let stored = directory.join("theme.css");
    match (stored.exists(), active.exists()) {
        (false, false) => true,
        (true, true) => read_safe_css(&stored)
            .zip(read_safe_css(active))
            .is_some_and(|(left, right)| left == right),
        _ => false,
    }
}

fn read_safe_css(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > THEME_CONFIG_LIMIT
    {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    validate_safe_css_bytes(&bytes).ok()?;
    Some(bytes)
}

fn validate_stored_image(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("无法检查主题图片：{}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("主题图片不是安全普通文件");
    }
    if metadata.len() == 0 {
        bail!("主题图片为空");
    }
    if metadata.len() > crate::dream_skin::DREAM_SKIN_PREPARED_LIMIT {
        bail!("主题图片超过 16 MiB");
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("无法读取主题图片：{}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let valid = match extension.as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "webp" => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "bmp" => bytes.starts_with(b"BM"),
        _ => false,
    };
    if !valid {
        bail!("主题图片内容与扩展名不匹配或已损坏");
    }
    Ok(())
}

fn read_safe_active_image(path: &Path) -> Option<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > crate::dream_skin::DREAM_SKIN_PREPARED_LIMIT
    {
        return None;
    }
    std::fs::read(path).ok()
}

fn validate_theme_draft(draft: &DreamSkinThemeDraft) -> anyhow::Result<()> {
    if draft.builtin {
        bail!("built-in Dream Skin theme is read-only");
    }
    if !valid_theme_id(&draft.config.id) {
        bail!("invalid Dream Skin theme id");
    }
    if draft.config.schema_version != 1 {
        bail!("unsupported Dream Skin theme schema");
    }
    if draft.config.name.trim().is_empty() {
        bail!("Dream Skin theme name is empty");
    }
    if !draft.config.style_preset.is_empty() && !valid_theme_id(&draft.config.style_preset) {
        bail!("invalid Dream Skin style preset");
    }
    if let Some(colors) = &draft.config.colors {
        for color in [
            &colors.background,
            &colors.panel,
            &colors.panel_alt,
            &colors.accent,
            &colors.accent_alt,
            &colors.secondary,
            &colors.highlight,
            &colors.text,
            &colors.muted,
            &colors.line,
        ] {
            if !valid_css_color(color) {
                bail!("invalid Dream Skin theme color: {color}");
            }
        }
    }
    Ok(())
}

fn valid_css_color(value: &str) -> bool {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    let body = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgba("))
        .and_then(|value| value.strip_suffix(')'));
    body.is_some_and(|body| {
        !body.is_empty()
            && body.bytes().all(|byte| {
                byte.is_ascii_digit()
                    || byte.is_ascii_whitespace()
                    || matches!(byte, b',' | b'.' | b'%')
            })
    })
}

fn slugify_theme_id(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("theme-{}", unique_suffix())
    } else {
        slug.chars().take(56).collect()
    }
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

fn replace_theme_directory(staging: &Path, target: &Path, suffix: &str) -> anyhow::Result<()> {
    if !target.exists() {
        std::fs::rename(staging, target)
            .with_context(|| format!("failed to install Dream Skin theme {}", target.display()))?;
        return Ok(());
    }
    reject_symlink(target)?;
    ensure_known_theme_directory(target)?;
    let parent = target
        .parent()
        .context("Dream Skin theme target has no parent")?;
    let id = target
        .file_name()
        .and_then(|value| value.to_str())
        .context("Dream Skin theme target has invalid id")?;
    let backup = parent.join(format!(".backup-{id}-{suffix}"));
    std::fs::rename(target, &backup)
        .with_context(|| format!("failed to back up Dream Skin theme {id}"))?;
    if let Err(error) = std::fs::rename(staging, target) {
        let _ = std::fs::rename(&backup, target);
        return Err(error).context("failed to replace Dream Skin theme directory");
    }
    remove_known_theme_directory(&backup)?;
    Ok(())
}

fn ensure_known_theme_directory(directory: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let metadata = std::fs::symlink_metadata(&path)?;
        let known = metadata.file_type().is_file()
            && !metadata.file_type().is_symlink()
            && (name == THEME_CONFIG_FILE
                || name == ".dream-skin-import.jpg"
                || (name.starts_with("image.") && supported_image_extension(&path))
                || matches!(name, "theme.css" | "manifest.json" | "LICENSE.txt"));
        if !known {
            bail!("Dream Skin theme directory contains unknown entry: {name}");
        }
    }
    Ok(())
}

fn remove_known_theme_directory(directory: &Path) -> anyhow::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    ensure_known_theme_directory(directory)?;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    }
    std::fs::remove_dir(directory)
        .with_context(|| format!("failed to remove {}", directory.display()))?;
    Ok(())
}

fn reject_symlink(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        bail!("Dream Skin path must not be a symbolic link");
    }
    Ok(())
}

fn summary_from_draft(
    draft: &DreamSkinThemeDraft,
    active: bool,
    modified: bool,
) -> DreamSkinThemeSummary {
    DreamSkinThemeSummary {
        key: format!("stored:{}", draft.config.id),
        id: draft.config.id.clone(),
        name: draft.config.name.clone(),
        preview_path: draft.image_path.clone(),
        kind: DreamSkinThemeKind::Stored,
        builtin: false,
        active,
        modified,
        damaged: false,
        error: String::new(),
    }
}

fn image_matches_active(stored: &Path, active_path: &str) -> bool {
    let Ok(stored_bytes) = std::fs::read(stored) else {
        return false;
    };
    if active_path.trim().is_empty() {
        return stored_bytes == crate::assets::dream_skin_default_image().1;
    }
    read_safe_active_image(Path::new(active_path))
        .is_some_and(|active_bytes| active_bytes == stored_bytes)
}
