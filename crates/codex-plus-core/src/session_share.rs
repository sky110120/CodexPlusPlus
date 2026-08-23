use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use anyhow::{Context, bail};
use base64::Engine;
use fs2::FileExt;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_ROLLOUT_BYTES: usize = 16 * 1024 * 1024;
const SHARE_HOSTS: &[&str] = &["share.codexpp.cc", "codexpp-share.pages.dev"];

pub fn save_pending_session_share_from_protocol_url(url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(url).context("会话导入链接格式无效")?;
    if parsed.scheme() != "codexplusplus" || parsed.host_str() != Some("session") {
        bail!("不是 Codex++ 会话导入链接");
    }
    let share_url = parsed
        .query_pairs()
        .find(|(key, _)| key == "url")
        .map(|(_, value)| value.into_owned())
        .context("会话导入链接缺少分享 URL")?;
    validate_share_url(&share_url)?;
    let path = crate::paths::default_pending_session_share_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("{share_url}\n"))?;
    Ok(share_url)
}

pub fn load_pending_session_share() -> anyhow::Result<Option<String>> {
    let path = crate::paths::default_pending_session_share_path();
    if !path.exists() {
        return Ok(None);
    }
    let value = fs::read_to_string(&path)
        .with_context(|| format!("读取待导入会话链接失败：{}", path.display()))?;
    let value = value.trim().to_string();
    if value.is_empty() {
        return Ok(None);
    }
    validate_share_url(&value)?;
    Ok(Some(value))
}

pub fn clear_pending_session_share() -> anyhow::Result<()> {
    let path = crate::paths::default_pending_session_share_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("清理待导入会话链接失败：{}", path.display()))
        }
    }
}

pub async fn import_shared_session_url(home: &Path, url: &str) -> anyhow::Result<Value> {
    let parsed = validate_share_url(url)?;
    let share_id = parsed
        .query_pairs()
        .find(|(key, _)| key == "s")
        .map(|(_, value)| value.into_owned())
        .or_else(|| {
            parsed.path_segments().and_then(|mut segments| {
                match (segments.next(), segments.next()) {
                    (Some("s"), Some(id)) => Some(id.to_string()),
                    _ => None,
                }
            })
        })
        .filter(|id| (20..=32).contains(&id.len()))
        .context("分享链接缺少有效的会话 ID")?;

    let mut endpoint = parsed.clone();
    endpoint.set_path(&format!("/api/shares/{share_id}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("创建分享会话下载客户端失败")?;
    let record = client
        .get(endpoint)
        .send()
        .await
        .context("读取分享会话失败")?
        .error_for_status()
        .context("分享会话不存在、已撤销或已过期")?
        .json::<Value>()
        .await
        .context("分享会话响应格式无效")?;
    let encrypted = record.get("encrypted").context("分享会话缺少加密内容")?;
    let key_value = parsed
        .fragment()
        .and_then(|fragment| fragment.split('&').find_map(|pair| pair.strip_prefix("k=")))
        .context("分享链接缺少解密密钥")?;
    let plaintext = decrypt_shared_payload(encrypted, key_value)?;
    let payload: Value = serde_json::from_slice(&plaintext).context("分享会话内容格式无效")?;
    if payload.get("kind").and_then(Value::as_str) != Some("codex-rollout") {
        bail!("当前分享内容不是可导入的 Codex rollout 会话");
    }
    import_rollout(home, &payload)
}

fn validate_share_url(url: &str) -> anyhow::Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url.trim()).context("分享 URL 格式无效")?;
    if parsed.scheme() != "https"
        || !parsed
            .host_str()
            .is_some_and(|host| SHARE_HOSTS.contains(&host))
    {
        bail!("仅支持 Codex++ 分享站点链接");
    }
    if parsed.fragment().is_none() {
        bail!("分享链接缺少解密密钥");
    }
    Ok(parsed)
}

fn decrypt_shared_payload(encrypted: &Value, key_value: &str) -> anyhow::Result<Vec<u8>> {
    if encrypted.get("v").and_then(Value::as_u64) != Some(1) {
        bail!("不支持此分享的数据格式");
    }
    let decode = |field: &str| -> anyhow::Result<Vec<u8>> {
        let value = encrypted
            .get(field)
            .and_then(Value::as_str)
            .with_context(|| format!("分享数据缺少 {field}"))?;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .with_context(|| format!("分享数据 {field} 无效"))
    };
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key_value)
        .context("分享链接解密密钥无效")?;
    let iv = decode("iv")?;
    let ciphertext = decode("ciphertext")?;
    if key.len() != 32 || iv.len() != 12 || ciphertext.len() > MAX_ROLLOUT_BYTES + 16 {
        bail!("分享数据大小或格式无效");
    }
    let cipher = Aes256Gcm::new_from_slice(&key).context("创建会话解密器失败")?;
    cipher
        .decrypt(Nonce::from_slice(&iv), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("分享会话解密失败"))
}

pub fn import_rollout_file(home: &Path, source_path: &Path) -> anyhow::Result<Value> {
    let bytes = fs::read(source_path)
        .with_context(|| format!("读取会话文件失败：{}", source_path.display()))?;
    if bytes.len() > MAX_ROLLOUT_BYTES {
        bail!("会话文件超过导入大小限制");
    }
    let content = String::from_utf8(bytes).context("会话文件不是有效的 UTF-8")?;
    if let Ok(payload) = serde_json::from_str::<Value>(&content)
        && payload.get("kind").and_then(Value::as_str) == Some("codex-rollout")
    {
        return import_rollout(home, &payload);
    }
    let session_id = find_session_id(&content)
        .or_else(|| {
            source_path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(find_uuid)
        })
        .ok_or_else(|| anyhow::anyhow!("无法从会话文件识别会话 ID"))?;
    let title = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("导入的会话");
    import_rollout(
        home,
        &json!({
            "kind": "codex-rollout",
            "session_id": session_id,
            "content": content,
            "title": title,
        }),
    )
}

pub fn export_rollout(home: &Path, session_id: &str) -> anyhow::Result<Value> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        bail!("会话 ID 为空");
    }
    let path =
        find_rollout(home, session_id).ok_or_else(|| anyhow::anyhow!("找不到原生会话文件"))?;
    let bytes = fs::read(&path).with_context(|| format!("读取会话文件失败：{}", path.display()))?;
    if bytes.len() > MAX_ROLLOUT_BYTES {
        bail!("会话文件超过分享大小限制");
    }
    let content = String::from_utf8(bytes).context("会话文件不是有效的 UTF-8")?;
    Ok(json!({
        "status": "ok",
        "kind": "codex-rollout",
        "session_id": session_id,
        "content": content,
        "filename": path.file_name().and_then(|value| value.to_str()).unwrap_or("rollout.jsonl"),
    }))
}

pub fn import_rollout(home: &Path, payload: &Value) -> anyhow::Result<Value> {
    if payload.get("kind").and_then(Value::as_str) != Some("codex-rollout") {
        bail!("不支持的会话文件格式");
    }
    let source_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("导入的会话")
        .trim();
    if source_id.is_empty() || content.is_empty() {
        bail!("会话文件内容不完整");
    }
    if content.len() > MAX_ROLLOUT_BYTES {
        bail!("会话文件超过导入大小限制");
    }
    let new_id = Uuid::new_v4().to_string();
    let rewritten = rewrite_rollout(content, source_id, &new_id)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let directory = home.join("sessions").join("imported");
    fs::create_dir_all(&directory).context("创建会话目录失败")?;
    let path = directory.join(format!("rollout-{now}-{new_id}.jsonl"));
    let index_path = home.join("session_index.jsonl");
    crate::settings::atomic_write(&path, rewritten.as_bytes()).context("写入导入会话失败")?;

    let registered_db = match register_imported_thread(home, &new_id, &path, title, content, now) {
        Ok(db_path) => db_path,
        Err(error) => {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
    };

    if let Err(error) = append_session_index_entry(
        &index_path,
        &json!({
            "id": new_id,
            "thread_name": if title.is_empty() { "导入的会话" } else { title },
            "updated_at": now.to_string(),
        }),
    ) {
        let mut rollback_errors = Vec::new();
        if let Some(db_path) = registered_db.as_deref()
            && let Err(rollback_error) = rollback_imported_thread(db_path, &new_id)
        {
            rollback_errors.push(format!("回滚会话数据库失败：{rollback_error}"));
        }
        if let Err(rollback_error) = fs::remove_file(&path)
            && rollback_error.kind() != std::io::ErrorKind::NotFound
        {
            rollback_errors.push(format!("回滚会话文件失败：{rollback_error}"));
        }
        let mut message = format!("更新会话索引失败：{error}");
        if !rollback_errors.is_empty() {
            message.push_str(&format!("；{}", rollback_errors.join("；")));
        }
        bail!(message);
    }

    Ok(
        json!({ "status": "ok", "session_id": new_id, "title": if title.is_empty() { "导入的会话" } else { title } }),
    )
}

fn append_session_index_entry(index_path: &Path, entry: &Value) -> anyhow::Result<()> {
    if let Some(parent) = index_path.parent() {
        fs::create_dir_all(parent).context("创建会话索引目录失败")?;
    }
    let mut index = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(index_path)
        .with_context(|| format!("打开会话索引失败：{}", index_path.display()))?;
    index
        .lock_exclusive()
        .with_context(|| format!("锁定会话索引失败：{}", index_path.display()))?;

    let result = (|| -> anyhow::Result<()> {
        let original_len = index.metadata()?.len();
        let mut appended = Vec::new();
        if original_len > 0 {
            index.seek(SeekFrom::End(-1))?;
            let mut last = [0_u8; 1];
            index.read_exact(&mut last)?;
            if last[0] != b'\n' {
                appended.push(b'\n');
            }
        }
        appended.extend_from_slice(&serde_json::to_vec(entry)?);
        appended.push(b'\n');
        let written = index.write(&appended)?;
        if written != appended.len() {
            let rollback =
                rollback_session_index_append(&mut index, original_len, &appended[..written]);
            return Err(match rollback {
                Ok(()) => {
                    anyhow::anyhow!("会话索引追加不完整：写入 {written}/{} 字节", appended.len())
                }
                Err(rollback_error) => anyhow::anyhow!(
                    "会话索引追加不完整：写入 {written}/{} 字节；清理失败：{rollback_error}",
                    appended.len()
                ),
            });
        }
        if let Err(error) = index.sync_data() {
            let rollback = rollback_session_index_append(&mut index, original_len, &appended);
            return Err(match rollback {
                Ok(()) => anyhow::anyhow!("同步会话索引失败：{error}"),
                Err(rollback_error) => {
                    anyhow::anyhow!("同步会话索引失败：{error}；清理失败：{rollback_error}")
                }
            });
        }
        Ok(())
    })();
    let _ = FileExt::unlock(&index);
    result
}

fn rollback_session_index_append(
    index: &mut std::fs::File,
    original_len: u64,
    appended: &[u8],
) -> anyhow::Result<()> {
    if appended.is_empty() {
        return Ok(());
    }
    let segment_end = original_len
        .checked_add(appended.len() as u64)
        .context("会话索引回滚长度溢出")?;
    let current_len = index.metadata()?.len();
    if current_len < segment_end {
        bail!("会话索引长度不足，无法清理失败追加");
    }
    index.seek(SeekFrom::Start(original_len))?;
    let mut observed = vec![0_u8; appended.len()];
    index.read_exact(&mut observed)?;
    if observed != appended {
        bail!("会话索引已被并发修改，无法安全清理失败追加");
    }
    index.seek(SeekFrom::Start(segment_end))?;
    let mut trailing = Vec::new();
    index.read_to_end(&mut trailing)?;
    index.set_len(original_len)?;
    if !trailing.is_empty() {
        index.write_all(&trailing)?;
    }
    index.sync_data()?;
    Ok(())
}

fn register_imported_thread(
    home: &Path,
    session_id: &str,
    rollout_path: &Path,
    title: &str,
    content: &str,
    now: u64,
) -> anyhow::Result<Option<PathBuf>> {
    let db_path = crate::codex_sqlite::codex_session_db_path_from_home(home);
    if !db_path.exists() {
        return Ok(None);
    }
    let db = Connection::open(&db_path)
        .with_context(|| format!("打开 Codex 会话数据库失败：{}", db_path.display()))?;
    let has_threads: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'threads')",
        [],
        |row| row.get(0),
    )?;
    if !has_threads {
        return Ok(None);
    }
    let columns = db
        .prepare("PRAGMA table_info(threads)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    let metadata = session_metadata(content);
    let first_message = first_user_message(content).unwrap_or_else(|| title.to_string());
    let cwd = metadata
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| home.to_string_lossy().to_string());
    let model_provider = metadata
        .get("model_provider")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("custom");
    let source = metadata
        .get("source")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("cli");
    let thread_source = metadata
        .get("thread_source")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("user");
    let cli_version = metadata
        .get("cli_version")
        .and_then(Value::as_str)
        .unwrap_or("");
    let history_mode = metadata
        .get("history_mode")
        .and_then(Value::as_str)
        .unwrap_or("legacy");
    let timestamp = i64::try_from(now).unwrap_or(i64::MAX);
    let rollout_path = rollout_path.to_string_lossy().to_string();

    let mut values: Vec<(&str, Box<dyn rusqlite::ToSql>)> = Vec::new();
    macro_rules! add {
        ($name:literal, $value:expr) => {
            if columns.contains($name) {
                values.push(($name, Box::new($value)));
            }
        };
    }
    add!("id", session_id.to_string());
    add!("rollout_path", rollout_path);
    add!("created_at", timestamp);
    add!("updated_at", timestamp);
    add!("source", source.to_string());
    add!("model_provider", model_provider.to_string());
    add!("cwd", cwd);
    add!("title", title.to_string());
    add!("sandbox_policy", r#"{"type":"disabled"}"#.to_string());
    add!("approval_mode", "never".to_string());
    add!("tokens_used", 0_i64);
    add!("has_user_event", 1_i64);
    add!("archived", 0_i64);
    add!("cli_version", cli_version.to_string());
    add!("first_user_message", first_message.clone());
    add!("thread_source", thread_source.to_string());
    add!("preview", first_message);
    add!("history_mode", history_mode.to_string());
    add!("recency_at", timestamp);
    add!("recency_at_ms", timestamp.saturating_mul(1000));

    if !columns.contains("id") || !columns.contains("rollout_path") {
        bail!("Codex 会话数据库缺少必要字段");
    }
    let names = values.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    let placeholders = (1..=names.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>();
    let sql = format!(
        "INSERT OR REPLACE INTO threads ({}) VALUES ({})",
        names.join(", "),
        placeholders.join(", ")
    );
    let params = values
        .iter()
        .map(|(_, value)| value.as_ref())
        .collect::<Vec<_>>();
    db.execute(&sql, rusqlite::params_from_iter(params))?;
    Ok(Some(db_path))
}

fn rollback_imported_thread(db_path: &Path, session_id: &str) -> anyhow::Result<()> {
    let db = Connection::open(db_path)
        .with_context(|| format!("打开 Codex 会话数据库失败：{}", db_path.display()))?;
    db.execute("DELETE FROM threads WHERE id = ?1", [session_id])?;
    Ok(())
}

fn session_metadata(content: &str) -> serde_json::Map<String, Value> {
    content
        .lines()
        .find_map(|line| {
            let value = serde_json::from_str::<Value>(line).ok()?;
            if value.get("type").and_then(Value::as_str) != Some("session_meta") {
                return None;
            }
            value.get("payload")?.as_object().cloned()
        })
        .unwrap_or_default()
}

fn first_user_message(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let value = serde_json::from_str::<Value>(line).ok()?;
        let payload = value.get("payload")?;
        if value.get("type").and_then(Value::as_str) != Some("response_item")
            || payload.get("type").and_then(Value::as_str) != Some("message")
            || payload.get("role").and_then(Value::as_str) != Some("user")
        {
            return None;
        }
        let content = payload.get("content")?.as_array()?;
        let text = content
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        (!text.is_empty()).then_some(text)
    })
}

fn find_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    for root in [home.join("sessions"), home.join("archived_sessions")] {
        if let Some(path) = find_rollout_in(&root, session_id) {
            return Some(path);
        }
    }
    None
}

fn find_rollout_in(root: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_rollout_in(&path, session_id) {
                return Some(found);
            }
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.contains(session_id))
        {
            return Some(path);
        }
    }
    None
}

fn rewrite_rollout(content: &str, old_id: &str, new_id: &str) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    for line in content.lines() {
        let mut value: Value = serde_json::from_str(line).context("会话文件包含无效 JSON 行")?;
        replace_id(&mut value, old_id, new_id);
        lines.push(serde_json::to_string(&value)?);
    }
    if lines.is_empty() {
        bail!("会话文件没有内容");
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn replace_id(value: &mut Value, old_id: &str, new_id: &str) {
    match value {
        Value::String(text) if text == old_id => *text = new_id.to_string(),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| replace_id(item, old_id, new_id)),
        Value::Object(object) => object
            .values_mut()
            .for_each(|item| replace_id(item, old_id, new_id)),
        _ => {}
    }
}

fn find_session_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(id) = find_uuid_in_value(&value) {
            return Some(id);
        }
    }
    None
}

fn find_uuid_in_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => find_uuid(text),
        Value::Array(items) => items.iter().find_map(find_uuid_in_value),
        Value::Object(object) => object.values().find_map(find_uuid_in_value),
        _ => None,
    }
}

fn find_uuid(value: &str) -> Option<String> {
    value
        .split(|character: char| !character.is_ascii_hexdigit() && character != '-')
        .find_map(|candidate| {
            uuid::Uuid::parse_str(candidate)
                .ok()
                .map(|id| id.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn rewrites_ids_in_rollout_lines() {
        let content = r#"{"type":"session_meta","payload":{"id":"old","session_id":"old"}}"#;
        let rewritten = rewrite_rollout(content, "old", "new").unwrap();
        assert!(rewritten.contains("\"id\":\"new\""));
        assert!(rewritten.contains("\"session_id\":\"new\""));
    }

    #[test]
    fn session_index_append_preserves_all_concurrent_entries() {
        let temp = tempfile::tempdir().unwrap();
        let index_path = temp.path().join("session_index.jsonl");
        fs::write(&index_path, "{\"id\":\"existing\"}").unwrap();
        let handles = (0..16)
            .map(|index| {
                let path = index_path.clone();
                std::thread::spawn(move || {
                    append_session_index_entry(
                        &path,
                        &json!({ "id": format!("imported-{index}") }),
                    )
                    .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        let contents = fs::read_to_string(index_path).unwrap();
        let ids = contents
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 17);
        assert!(ids.contains("existing"));
        for index in 0..16 {
            assert!(ids.contains(&format!("imported-{index}")));
        }
    }

    #[test]
    fn failed_session_index_append_cleanup_preserves_following_entries() {
        let temp = tempfile::tempdir().unwrap();
        let index_path = temp.path().join("session_index.jsonl");
        let original = b"{\"id\":\"existing\"}\n";
        let failed_prefix = b"{\"id\":\"ghost";
        let following = b"{\"id\":\"following\"}\n";
        let mut contents = original.to_vec();
        contents.extend_from_slice(failed_prefix);
        contents.extend_from_slice(following);
        fs::write(&index_path, contents).unwrap();
        let mut index = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&index_path)
            .unwrap();

        rollback_session_index_append(&mut index, original.len() as u64, failed_prefix).unwrap();

        assert_eq!(
            fs::read(index_path).unwrap(),
            [original.as_slice(), following].concat()
        );
    }

    #[test]
    fn imported_thread_registration_can_be_rolled_back() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let db_path = home.join("state_5.sqlite");
        let db = Connection::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, title TEXT)",
            [],
        )
        .unwrap();
        drop(db);
        let rollout_path = home.join("sessions/imported/rollout-test.jsonl");

        let registered = register_imported_thread(
            home,
            "imported-id",
            &rollout_path,
            "Imported",
            r#"{"type":"session_meta","payload":{"id":"imported-id"}}"#,
            1,
        )
        .unwrap()
        .unwrap();
        rollback_imported_thread(&registered, "imported-id").unwrap();

        let db = Connection::open(db_path).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM threads WHERE id = 'imported-id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
