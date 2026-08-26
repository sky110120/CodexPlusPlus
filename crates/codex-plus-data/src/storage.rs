use crate::BackupStore;
use codex_plus_core::models::{DeleteResult, DeleteStatus, SessionRef};
use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const GLOBAL_STATE_RELATIVE_PATHS: &[&str] = &[
    ".codex-global-state.json",
    ".codex-global-state.json.bak",
    "backups_state/app-state-sync/latest-safe-state.json",
];
const THREAD_REFERENCE_TABLES: &[(&str, &str)] = &[
    ("local_thread_catalog", "thread_id"),
    ("thread_timeline_ledger", "thread_id"),
    ("automation_runs", "thread_id"),
    ("inbox_items", "thread_id"),
];

pub fn delete_local_from_paths(
    db_paths: impl IntoIterator<Item = PathBuf>,
    backup_store: BackupStore,
    session: &SessionRef,
    codex_home: Option<&Path>,
) -> DeleteResult {
    let db_paths = db_paths.into_iter().collect::<Vec<_>>();
    if let Err(error) = preflight_child_session_markers(&db_paths, &session.session_id) {
        return failed(&session.session_id, error.to_string());
    }
    let codex_home = codex_home
        .map(Path::to_path_buf)
        .or_else(|| infer_codex_home_from_paths(&db_paths));
    let mut allowed_db_paths = db_paths.clone();
    if let Some(home) = codex_home.as_deref() {
        extend_unique_paths(
            &mut allowed_db_paths,
            codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(home),
        );
    }
    let mut result = failed(
        &session.session_id,
        "Thread not found in local storage".to_string(),
    );
    let mut deleted_count = 0usize;
    let mut backup_tokens = Vec::new();
    for db_path in db_paths {
        let mut adapter = SQLiteStorageAdapter::new(db_path, backup_store.clone())
            .with_allowed_db_paths(allowed_db_paths.clone());
        if let Some(home) = codex_home.as_deref() {
            adapter = adapter.with_codex_home(home);
        }
        let candidate_result = adapter.delete_local(session);
        if matches!(candidate_result.status, DeleteStatus::LocalDeleted) {
            deleted_count += 1;
            if let Some(token) = candidate_result.undo_token.as_ref() {
                backup_tokens.extend(parse_undo_tokens(token));
            }
            result = candidate_result;
        } else if deleted_count == 0 {
            result = candidate_result;
        }
    }
    if deleted_count > 0 {
        if deleted_count > 1 {
            result.message = format!("已从 {deleted_count} 个本地存储删除");
        }
        if !backup_tokens.is_empty() {
            result.undo_token = Some(format_undo_tokens(&backup_tokens));
            if backup_tokens.len() > 1 {
                result.backup_path = None;
            }
        }
    }
    // 纯 API 模式（model_provider = "custom"）下 threads 表是空的，上面每个库都查不到
    // 记录，于是直接返回「Thread not found in local storage」而会话行仍留在列表里
    // ——因为 UI 读的是 session_index.jsonl，那条记录没人清（#1998）。
    //
    // 数据库里没有不代表索引里没有，这里退一步清索引：真清掉了就算删除成功，
    // 索引里也没有才是真的找不到。
    if deleted_count == 0
        && matches!(result.status, DeleteStatus::Failed)
        && let Some(home) = codex_home
    {
        let thread_id = normalize_codex_thread_id(&session.session_id);
        match crate::provider_sync::remove_session_index_entry(&home, &thread_id) {
            Ok(removed) if removed > 0 => {
                result.status = DeleteStatus::LocalDeleted;
                result.message = format!("已从 session_index.jsonl 清理 {removed} 条记录");
            }
            Ok(_) => {}
            Err(error) => {
                result.message =
                    format!("{}；session_index.jsonl 清理失败：{error}", result.message);
            }
        }
    }
    result
}

fn preflight_child_session_markers(db_paths: &[PathBuf], session_id: &str) -> anyhow::Result<()> {
    let thread_id = normalize_codex_thread_id(session_id);
    let mut child_ids = HashSet::new();
    for db_path in db_paths {
        if !db_path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if !has_table(&db, "thread_spawn_edges")?
            || !has_columns(
                &db,
                "thread_spawn_edges",
                &["parent_thread_id", "child_thread_id"],
            )?
        {
            continue;
        }
        let mut stmt = db.prepare(
            "SELECT DISTINCT child_thread_id FROM thread_spawn_edges WHERE parent_thread_id = ?1 AND COALESCE(child_thread_id, '') <> ''",
        )?;
        for child_id in stmt.query_map([&thread_id], |row| row.get::<_, String>(0))? {
            child_ids.insert(child_id?);
        }
    }
    if child_ids.is_empty() {
        return Ok(());
    }

    let mut durable_child_ids = HashSet::new();
    let mut explicit_user_thread_ids = HashSet::new();
    let mut rollout_paths = Vec::new();
    for db_path in db_paths {
        if !db_path.is_file() {
            continue;
        }
        let db = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if has_table(&db, "agent_job_items")?
            && has_columns(&db, "agent_job_items", &["assigned_thread_id"])?
        {
            let mut stmt = db.prepare(
                "SELECT DISTINCT assigned_thread_id FROM agent_job_items WHERE COALESCE(assigned_thread_id, '') <> ''",
            )?;
            for assigned_thread_id in stmt.query_map([], |row| row.get::<_, String>(0))? {
                let assigned_thread_id = assigned_thread_id?;
                if child_ids.contains(&assigned_thread_id) {
                    durable_child_ids.insert(assigned_thread_id);
                }
            }
        }

        let columns = table_columns(&db, "threads")?
            .into_iter()
            .collect::<HashSet<_>>();
        if columns.contains("id") {
            let source = optional_column_expression(&columns, "source", "''");
            let thread_source = optional_column_expression(&columns, "thread_source", "NULL");
            let rollout_path = optional_column_expression(&columns, "rollout_path", "''");
            let sql = format!(
                "SELECT id, {source}, {thread_source}, {rollout_path} FROM threads WHERE COALESCE(id, '') <> ''"
            );
            let mut stmt = db.prepare(&sql)?;
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1).unwrap_or_default(),
                    row.get::<_, Option<String>>(2).unwrap_or(None),
                    row.get::<_, String>(3).unwrap_or_default(),
                ))
            })? {
                let (candidate_id, source, thread_source, rollout_path) = row?;
                if !child_ids.contains(&candidate_id) {
                    continue;
                }
                if crate::provider_sync::thread_source_is_user(thread_source.as_deref()) {
                    explicit_user_thread_ids.insert(candidate_id.clone());
                } else if crate::provider_sync::thread_source_marks_non_root(
                    thread_source.as_deref(),
                ) || crate::provider_sync::source_marks_non_root_agent(&source)
                {
                    durable_child_ids.insert(candidate_id.clone());
                }
                if !rollout_path.trim().is_empty() {
                    rollout_paths.push((candidate_id, PathBuf::from(rollout_path)));
                }
            }
        }

        let catalog_columns = table_columns(&db, "local_thread_catalog")?
            .into_iter()
            .collect::<HashSet<_>>();
        if catalog_columns.contains("thread_id") {
            let source_kind = optional_column_expression(&catalog_columns, "source_kind", "''");
            let thread_source =
                optional_column_expression(&catalog_columns, "thread_source", "NULL");
            let sql = format!(
                "SELECT thread_id, {source_kind}, {thread_source} FROM local_thread_catalog WHERE COALESCE(thread_id, '') <> ''"
            );
            let mut stmt = db.prepare(&sql)?;
            for row in stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1).unwrap_or_default(),
                    row.get::<_, Option<String>>(2).unwrap_or(None),
                ))
            })? {
                let (candidate_id, source_kind, thread_source) = row?;
                if child_ids.contains(&candidate_id)
                    && (crate::provider_sync::thread_source_marks_non_root(
                        thread_source.as_deref(),
                    ) || crate::provider_sync::source_marks_non_root_agent(&source_kind))
                {
                    durable_child_ids.insert(candidate_id);
                }
            }
        }
    }

    for (child_id, rollout_path) in rollout_paths {
        if rollout_session_meta_marks_non_root(&rollout_path, &child_id)? {
            durable_child_ids.insert(child_id);
        }
    }

    let mut edge_only_child_ids = child_ids
        .difference(&durable_child_ids)
        .filter(|child_id| !explicit_user_thread_ids.contains(*child_id))
        .cloned()
        .collect::<Vec<_>>();
    edge_only_child_ids.sort();
    if edge_only_child_ids.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "删除已阻止：发现 {} 个仅由主会话关联边标记的子进程会话；本次未创建备份或修改数据。子进程 ID：{}",
        edge_only_child_ids.len(),
        edge_only_child_ids.join(", ")
    )
}

fn rollout_session_meta_marks_non_root(path: &Path, thread_id: &str) -> anyhow::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let file = File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(payload) = record.get("payload").and_then(Value::as_object) else {
            continue;
        };
        if payload
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id != thread_id)
        {
            continue;
        }
        if payload.get("source").is_some_and(|source| {
            crate::provider_sync::source_marks_non_root_agent(&source.to_string())
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CleanupThreadReferenceResult {
    pub sqlite_rows_removed: usize,
    pub session_index_lines_removed: usize,
    pub global_state_files_changed: usize,
    pub global_state_references_removed: usize,
}

pub fn cleanup_thread_reference_state(
    session_id: &str,
) -> anyhow::Result<CleanupThreadReferenceResult> {
    cleanup_thread_reference_state_for_home(
        &codex_plus_core::codex_sqlite::default_codex_home_dir(),
        session_id,
    )
}

pub fn cleanup_thread_reference_state_for_home(
    home: &Path,
    session_id: &str,
) -> anyhow::Result<CleanupThreadReferenceResult> {
    let thread_id = normalize_codex_thread_id(session_id);
    let mut result = CleanupThreadReferenceResult::default();
    for db_path in codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(home) {
        if !db_path.is_file() {
            continue;
        }
        let db = Connection::open(&db_path)?;
        for (table, column) in THREAD_REFERENCE_TABLES {
            if !has_table(&db, table)? || !has_columns(&db, table, &[column])? {
                continue;
            }
            result.sqlite_rows_removed += db.execute(
                &format!("DELETE FROM {table} WHERE {column} = ?1"),
                [&thread_id],
            )?;
        }
    }

    let session_index = home.join("session_index.jsonl");
    if session_index.is_file() {
        let original = fs::read_to_string(&session_index)?;
        let kept = original
            .lines()
            .filter(|line| !session_index_line_matches(line, &thread_id))
            .collect::<Vec<_>>();
        let next = if kept.is_empty() {
            String::new()
        } else {
            format!("{}\n", kept.join("\n"))
        };
        if next != original {
            codex_plus_core::settings::atomic_write(&session_index, next.as_bytes())?;
            result.session_index_lines_removed =
                original.lines().count().saturating_sub(kept.len());
        }
    }

    let global_state =
        codex_plus_core::codex_app_state::remove_thread_references_from_state(home, &[thread_id])?;
    result.global_state_files_changed = global_state.files_changed;
    result.global_state_references_removed = global_state.references_removed;
    Ok(result)
}

fn session_index_line_matches(line: &str, thread_id: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_string))
        .is_some_and(|id| id == thread_id)
}

fn session_index_lines_for_thread(home: &Path, thread_id: &str) -> anyhow::Result<Vec<String>> {
    let path = home.join("session_index.jsonl");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .filter(|line| session_index_line_matches(line, thread_id))
        .map(ToString::to_string)
        .collect())
}

fn global_state_file_backups(home: &Path, thread_id: &str) -> anyhow::Result<Vec<Value>> {
    let mut backups = Vec::new();
    for relative_path in GLOBAL_STATE_RELATIVE_PATHS {
        let path = home.join(relative_path);
        if !path.is_file() {
            continue;
        }
        let original = fs::read(&path)?;
        let mut value: Value = serde_json::from_slice(&original)?;
        if remove_thread_references_from_value(&mut value, thread_id) == 0 {
            continue;
        }
        let cleaned = serde_json::to_vec_pretty(&value)?;
        backups.push(json!({
            "relative_path": relative_path,
            "original_b64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                original,
            ),
            "cleaned_b64": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                cleaned,
            ),
        }));
    }
    Ok(backups)
}

fn reference_database_backups(
    home: &Path,
    thread_id: &str,
    primary_db_path: &Path,
) -> anyhow::Result<Vec<Value>> {
    let mut backups = Vec::new();
    for db_path in codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(home) {
        if !db_path.is_file() || paths_refer_to_same_file(&db_path, primary_db_path) {
            continue;
        }
        let db = Connection::open(&db_path)?;
        let mut tables = Map::new();
        for (table, column) in THREAD_REFERENCE_TABLES {
            if !has_table(&db, table)? || !has_columns(&db, table, &[*column])? {
                continue;
            }
            let rows = select_dicts(
                &db,
                &format!("SELECT * FROM {table} WHERE {column} = ?1"),
                &[&thread_id],
            )?;
            if !rows.is_empty() {
                tables.insert((*table).to_string(), Value::Array(rows));
            }
        }
        if !tables.is_empty() {
            backups.push(json!({
                "source_db": db_path,
                "tables": tables,
            }));
        }
    }
    Ok(backups)
}

fn remove_thread_references_from_value(value: &mut Value, thread_id: &str) -> usize {
    match value {
        Value::Array(items) => {
            let mut removed = 0;
            let mut kept = Vec::with_capacity(items.len());
            for mut item in items.drain(..) {
                if item.as_str() == Some(thread_id) {
                    removed += 1;
                    continue;
                }
                removed += remove_thread_references_from_value(&mut item, thread_id);
                kept.push(item);
            }
            *items = kept;
            removed
        }
        Value::Object(object) => {
            let mut removed = 0;
            let mut kept = Map::new();
            for (key, mut item) in std::mem::take(object) {
                if thread_reference_key_matches(&key, thread_id) {
                    removed += 1;
                    continue;
                }
                removed += remove_thread_references_from_value(&mut item, thread_id);
                kept.insert(key, item);
            }
            *object = kept;
            removed
        }
        _ => 0,
    }
}

fn thread_reference_key_matches(key: &str, thread_id: &str) -> bool {
    key == thread_id
        || key.ends_with(&format!(":{thread_id}"))
        || key.ends_with(&format!("%3A{thread_id}"))
}

fn infer_codex_home_from_paths(paths: &[PathBuf]) -> Option<PathBuf> {
    let default_home = codex_plus_core::codex_sqlite::default_codex_home_dir();
    let default_paths =
        codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(&default_home);
    if paths.iter().any(|path| {
        default_paths
            .iter()
            .any(|candidate| paths_refer_to_same_file(path, candidate))
    }) {
        return Some(default_home);
    }
    paths
        .iter()
        .find_map(|path| infer_structural_codex_home_from_db_path(path))
}

fn infer_codex_home_from_db_path(path: &Path) -> Option<PathBuf> {
    infer_codex_home_from_paths(&[path.to_path_buf()])
}

fn infer_structural_codex_home_from_db_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some("sqlite") {
        return parent.parent().map(Path::to_path_buf);
    }
    (path.file_name().and_then(|name| name.to_str()) == Some("state_5.sqlite"))
        .then(|| parent.to_path_buf())
}

fn extend_unique_paths(paths: &mut Vec<PathBuf>, additions: impl IntoIterator<Item = PathBuf>) {
    for path in additions {
        if !paths
            .iter()
            .any(|candidate| paths_refer_to_same_file(candidate, &path))
        {
            paths.push(path);
        }
    }
}

fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[derive(Debug, Clone)]
pub struct SQLiteStorageAdapter {
    db_path: PathBuf,
    backup_store: BackupStore,
    allowed_db_paths: Vec<PathBuf>,
    codex_home: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaKind {
    GenericSessions,
    CodexThreads,
    CodexAutomationRuns,
}

fn codex_thread_filter(db: &Connection) -> anyhow::Result<String> {
    let mut subagent_filters = Vec::new();
    let explicit_user = if has_columns(db, "threads", &["thread_source"])? {
        Some("LOWER(TRIM(COALESCE(threads.thread_source, ''))) = 'user'")
    } else {
        None
    };
    if has_table(db, "thread_spawn_edges")?
        && has_columns(db, "thread_spawn_edges", &["child_thread_id"])?
    {
        let relation =
            "NOT EXISTS (SELECT 1 FROM thread_spawn_edges e WHERE e.child_thread_id = threads.id)";
        subagent_filters.push(match explicit_user {
            Some(user) => format!("({user} OR {relation})"),
            None => relation.to_string(),
        });
    }
    if has_table(db, "agent_job_items")?
        && has_columns(db, "agent_job_items", &["assigned_thread_id"])?
    {
        let relation =
            "NOT EXISTS (SELECT 1 FROM agent_job_items j WHERE j.assigned_thread_id = threads.id)";
        subagent_filters.push(match explicit_user {
            Some(user) => format!("({user} OR {relation})"),
            None => relation.to_string(),
        });
    }
    Ok(if subagent_filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", subagent_filters.join(" AND "))
    })
}

fn sqlite_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSession {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub model_provider: String,
    pub archived: bool,
    pub updated_at_ms: Option<i64>,
    pub rollout_path: String,
    pub db_path: String,
}

#[derive(Debug, Clone)]
struct OwnedSqlValue(SqlValue);

impl ToSql for OwnedSqlValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(self.0.clone()))
    }
}

impl SQLiteStorageAdapter {
    pub fn new(db_path: impl Into<PathBuf>, backup_store: BackupStore) -> Self {
        let db_path = db_path.into();
        let codex_home = infer_codex_home_from_db_path(&db_path);
        let mut allowed_db_paths = vec![db_path.clone()];
        if let Some(home) = codex_home.as_deref() {
            extend_unique_paths(
                &mut allowed_db_paths,
                codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(home),
            );
        }
        Self {
            allowed_db_paths,
            db_path,
            backup_store,
            codex_home,
        }
    }

    pub fn with_allowed_db_paths(mut self, db_paths: impl IntoIterator<Item = PathBuf>) -> Self {
        for db_path in db_paths {
            if !self.allowed_db_paths.contains(&db_path) {
                self.allowed_db_paths.push(db_path);
            }
        }
        self
    }

    pub fn with_codex_home(mut self, codex_home: impl Into<PathBuf>) -> Self {
        let codex_home = codex_home.into();
        extend_unique_paths(
            &mut self.allowed_db_paths,
            codex_plus_core::codex_sqlite::codex_thread_reference_db_paths_from_home(&codex_home),
        );
        self.codex_home = Some(codex_home);
        self
    }

    pub fn delete_local(&self, session: &SessionRef) -> DeleteResult {
        if !self.db_path.exists() {
            return failed(
                &session.session_id,
                format!("Database not found: {}", self.db_path.to_string_lossy()),
            );
        }
        let result = (|| -> anyhow::Result<DeleteResult> {
            let mut db = Connection::open(&self.db_path)?;
            match schema_kind(&db)? {
                Some(SchemaKind::GenericSessions) => self.delete_generic_session(&mut db, session),
                Some(SchemaKind::CodexThreads) => self.delete_codex_thread(&mut db, session),
                Some(SchemaKind::CodexAutomationRuns) => {
                    self.delete_codex_automation_run(&mut db, session)
                }
                None => Ok(failed(
                    &session.session_id,
                    "Unsupported local storage schema".to_string(),
                )),
            }
        })();
        let mut result = result.unwrap_or_else(|err| failed(&session.session_id, err.to_string()));
        // 删成功就一并清 session_index.jsonl。
        //
        // 放在这个统一出口而不是各个 delete_* 里：三种 schema 里原先只有
        // delete_codex_thread 清了索引，另外两种删掉数据库行却把索引条目留着，
        // 于是重启后 UI 从索引读，会话又冒出来，再删再冒（#1979）。放在出口
        // 处理，将来加新 schema 也不会漏。
        //
        // delete_codex_thread 里那次调用保留：它需要把清理失败并进自己那条
        // 「数据库已删但文件删除失败」的消息里；这里对已清理过的再调一次是幂等的
        // （条目已不在，返回 0）。
        if matches!(result.status, DeleteStatus::LocalDeleted)
            && let Some(home) = self.codex_home.as_deref()
        {
            let thread_id = normalize_codex_thread_id(&session.session_id);
            if let Err(error) = crate::provider_sync::remove_session_index_entry(home, &thread_id) {
                result.message =
                    format!("{}；session_index.jsonl 清理失败：{error}", result.message);
            }
        }
        result
    }

    pub fn list_local_sessions(&self) -> anyhow::Result<Vec<LocalSession>> {
        self.list_local_sessions_limited(usize::MAX)
    }

    pub fn list_local_sessions_limited(&self, limit: usize) -> anyhow::Result<Vec<LocalSession>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let db = Connection::open(&self.db_path)?;
        match schema_kind(&db)? {
            Some(SchemaKind::CodexThreads) => self.list_codex_threads(&db, limit),
            Some(SchemaKind::CodexAutomationRuns) => self.list_codex_automation_runs(&db, limit),
            _ => anyhow::bail!("Unsupported local storage schema"),
        }
    }

    fn list_codex_threads(
        &self,
        db: &Connection,
        limit: usize,
    ) -> anyhow::Result<Vec<LocalSession>> {
        let columns = table_columns(&db, "threads")?
            .into_iter()
            .collect::<HashSet<_>>();
        let title = optional_column_expression(&columns, "title", "''");
        let cwd = optional_column_expression(&columns, "cwd", "''");
        let model_provider = optional_column_expression(&columns, "model_provider", "''");
        let archived = optional_column_expression(&columns, "archived", "0");
        let updated_at_ms = if columns.contains("updated_at_ms") {
            "updated_at_ms"
        } else if columns.contains("updated_at") {
            "updated_at * 1000"
        } else if columns.contains("created_at_ms") {
            "created_at_ms"
        } else {
            "NULL"
        };
        let rollout_path = optional_column_expression(&columns, "rollout_path", "''");
        let child_thread_filter = codex_thread_filter(db)?;
        let sql = format!(
            "SELECT id, {title}, {cwd}, {model_provider}, {archived}, {updated_at_ms}, {rollout_path}
             FROM threads
             {child_thread_filter}
             ORDER BY COALESCE({updated_at_ms}, 0) DESC, id DESC
             LIMIT ?1"
        );
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map([sqlite_limit(limit)], |row| {
            Ok(LocalSession {
                id: row.get(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                cwd: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                model_provider: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                archived: row.get::<_, Option<i64>>(4)?.unwrap_or_default() != 0,
                updated_at_ms: row.get(5)?,
                rollout_path: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                db_path: self.db_path.to_string_lossy().to_string(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn list_codex_automation_runs(
        &self,
        db: &Connection,
        limit: usize,
    ) -> anyhow::Result<Vec<LocalSession>> {
        let columns = table_columns(db, "automation_runs")?
            .into_iter()
            .collect::<HashSet<_>>();
        let title = optional_column_expression(&columns, "thread_title", "''");
        let cwd = optional_column_expression(&columns, "source_cwd", "''");
        let status = optional_column_expression(&columns, "status", "''");
        let updated_at = optional_column_expression(&columns, "updated_at", "NULL");
        let created_at = optional_column_expression(&columns, "created_at", "NULL");
        let sql = format!(
            "SELECT thread_id, {title}, {cwd}, {status}, {updated_at}, {created_at}
             FROM automation_runs
             WHERE COALESCE(thread_id, '') <> ''
             ORDER BY COALESCE({updated_at}, {created_at}, 0) DESC, thread_id DESC
             LIMIT ?1"
        );
        let mut stmt = db.prepare(&sql)?;
        let rows = stmt.query_map([sqlite_limit(limit)], |row| {
            let updated_at_ms = row
                .get::<_, Option<i64>>(4)?
                .or(row.get::<_, Option<i64>>(5)?);
            Ok(LocalSession {
                id: row.get(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                cwd: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                model_provider: String::new(),
                archived: row
                    .get::<_, Option<String>>(3)?
                    .map(|status| status.eq_ignore_ascii_case("archived"))
                    .unwrap_or(false),
                updated_at_ms,
                rollout_path: String::new(),
                db_path: self.db_path.to_string_lossy().to_string(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn undo(&self, token: &str) -> DeleteResult {
        let result = (|| -> anyhow::Result<DeleteResult> {
            let backups = undo_backups(&self.backup_store, token)?;
            let session_id = backups[0]["session_id"].as_str().unwrap_or("").to_string();
            restore_backups(
                &backups,
                &self.db_path,
                &self.allowed_db_paths,
                self.codex_home.as_deref(),
            )?;
            Ok(DeleteResult {
                status: DeleteStatus::Undone,
                session_id,
                message: "Local session restored from backup".to_string(),
                undo_token: Some(token.to_string()),
                backup_path: None,
            })
        })();
        result.unwrap_or_else(|err| failed_with_undo("", err.to_string(), token, None))
    }

    pub fn find_archived_thread_by_title(&self, title: &str) -> Option<SessionRef> {
        let db = Connection::open(&self.db_path).ok()?;
        if schema_kind(&db).ok().flatten() != Some(SchemaKind::CodexThreads)
            || !has_columns(&db, "threads", &["archived"]).ok()?
        {
            return None;
        }
        let mut stmt = db
            .prepare(
                "SELECT id, title FROM threads
                 WHERE archived = 1 AND (title = ?1 OR title LIKE ?2 OR ?1 LIKE '%' || title || '%')
                 ORDER BY archived_at DESC LIMIT 1",
            )
            .ok()?;
        let mut rows = stmt.query((title, format!("%{title}%"))).ok()?;
        let row = rows.next().ok().flatten()?;
        let id: String = row.get(0).ok()?;
        let row_title: Option<String> = row.get(1).ok()?;
        SessionRef::new(id, row_title.unwrap_or_else(|| title.to_string())).ok()
    }

    pub fn codex_thread_usage_history(&self, session: &SessionRef) -> serde_json::Value {
        if !self.db_path.exists() {
            return json!({
                "status": "failed",
                "session_id": session.session_id,
                "message": format!("Database not found: {}", self.db_path.to_string_lossy()),
                "history": []
            });
        }
        let result = (|| -> anyhow::Result<Value> {
            let db = Connection::open(&self.db_path)?;
            if schema_kind(&db)? != Some(SchemaKind::CodexThreads)
                || !has_columns(&db, "threads", &["rollout_path"])?
            {
                return Ok(json!({
                    "status": "failed",
                    "session_id": session.session_id,
                    "message": "Unsupported local storage schema",
                    "history": []
                }));
            }
            let thread_id = normalize_codex_thread_id(&session.session_id);
            let rollout_path: Option<String> = db
                .query_row(
                    "SELECT rollout_path FROM threads WHERE id = ?1",
                    [&thread_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(rollout_path) = rollout_path.filter(|path| !path.trim().is_empty()) else {
                return Ok(json!({
                    "status": "failed",
                    "session_id": thread_id,
                    "message": "Thread rollout path is empty",
                    "history": []
                }));
            };
            let rollout = PathBuf::from(&rollout_path);
            if !rollout.is_file() {
                return Ok(json!({
                    "status": "failed",
                    "session_id": thread_id,
                    "message": format!("rollout file not found: {rollout_path}"),
                    "history": []
                }));
            }
            let history = read_rollout_usage_history(&rollout, &thread_id)?;
            Ok(json!({
                "status": "ok",
                "session_id": thread_id,
                "rollout_path": rollout_path,
                "history": history,
            }))
        })();
        result.unwrap_or_else(|err| {
            json!({
                "status": "failed",
                "session_id": session.session_id,
                "message": err.to_string(),
                "history": []
            })
        })
    }

    fn prepare_thread_reference_backup(
        &self,
        thread_id: &str,
        primary_tables: &mut Map<String, Value>,
    ) -> anyhow::Result<()> {
        let Some(home) = self.codex_home.as_deref() else {
            return Ok(());
        };

        let session_index_lines = session_index_lines_for_thread(home, thread_id)?;
        if !session_index_lines.is_empty() {
            primary_tables.insert(
                "__session_index".to_string(),
                Value::Array(session_index_lines.into_iter().map(Value::String).collect()),
            );
        }

        let global_state_files = global_state_file_backups(home, thread_id)?;
        if !global_state_files.is_empty() {
            primary_tables.insert(
                "__global_state_files".to_string(),
                Value::Array(global_state_files),
            );
        }
        let reference_databases = reference_database_backups(home, thread_id, &self.db_path)?;
        if !reference_databases.is_empty() {
            primary_tables.insert(
                "__reference_databases".to_string(),
                Value::Array(reference_databases),
            );
        }
        Ok(())
    }

    fn cleanup_thread_references_after_delete(
        &self,
        thread_id: &str,
        undo_token: &str,
        backup_path: Option<&Path>,
    ) -> Option<DeleteResult> {
        let home = self.codex_home.as_deref()?;
        cleanup_thread_reference_state_for_home(home, thread_id)
            .err()
            .map(|error| {
                failed_with_undo(
                    thread_id,
                    format!("本地会话已删除，但引用状态清理失败：{error}"),
                    undo_token,
                    backup_path,
                )
            })
    }

    fn delete_generic_session(
        &self,
        db: &mut Connection,
        session: &SessionRef,
    ) -> anyhow::Result<DeleteResult> {
        let sessions = select_dicts(
            db,
            "SELECT * FROM sessions WHERE id = ?1",
            &[&session.session_id],
        )?;
        if sessions.is_empty() {
            return Ok(failed(
                &session.session_id,
                "Session not found in local storage".to_string(),
            ));
        }
        let messages = if has_table(db, "messages")? {
            select_dicts(
                db,
                "SELECT * FROM messages WHERE session_id = ?1",
                &[&session.session_id],
            )?
        } else {
            Vec::new()
        };
        let token = self.backup_store.write_backup(
            &session.session_id,
            &self.db_path,
            json!({"sessions": sessions, "messages": messages}),
        )?;
        let backup_path = self.backup_store.path_for(&token);
        let delete_result = (|| -> anyhow::Result<()> {
            let tx = db.transaction()?;
            if has_table(&tx, "messages")? {
                tx.execute(
                    "DELETE FROM messages WHERE session_id = ?1",
                    [&session.session_id],
                )?;
            }
            tx.execute("DELETE FROM sessions WHERE id = ?1", [&session.session_id])?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(err) = delete_result {
            return Ok(failed_with_undo(
                &session.session_id,
                err.to_string(),
                &token,
                Some(&backup_path),
            ));
        }
        Ok(local_deleted(&session.session_id, &token, &backup_path))
    }

    fn delete_codex_thread(
        &self,
        db: &mut Connection,
        session: &SessionRef,
    ) -> anyhow::Result<DeleteResult> {
        let thread_id = normalize_codex_thread_id(&session.session_id);
        let thread_rows = select_dicts(db, "SELECT * FROM threads WHERE id = ?1", &[&thread_id])?;
        if thread_rows.is_empty() {
            return Ok(failed(
                &session.session_id,
                "Thread not found in local storage".to_string(),
            ));
        }
        let mut tables = Map::new();
        tables.insert("threads".to_string(), Value::Array(thread_rows));
        backup_related_rows(
            db,
            &mut tables,
            "thread_dynamic_tools",
            "thread_id = ?1",
            &[&thread_id],
        )?;
        backup_related_rows(
            db,
            &mut tables,
            "thread_goals",
            "thread_id = ?1",
            &[&thread_id],
        )?;
        backup_related_rows(
            db,
            &mut tables,
            "thread_spawn_edges",
            "parent_thread_id = ?1 OR child_thread_id = ?1",
            &[&thread_id],
        )?;
        backup_related_rows(
            db,
            &mut tables,
            "stage1_outputs",
            "thread_id = ?1",
            &[&thread_id],
        )?;
        backup_related_rows(
            db,
            &mut tables,
            "agent_job_items",
            "assigned_thread_id = ?1",
            &[&thread_id],
        )?;
        let file_backups = rollout_file_backups(tables.get("threads").and_then(Value::as_array));
        if !file_backups.is_empty() {
            tables.insert("__files".to_string(), Value::Array(file_backups.clone()));
        }
        self.prepare_thread_reference_backup(&thread_id, &mut tables)?;
        let token =
            self.backup_store
                .write_backup(&thread_id, &self.db_path, Value::Object(tables))?;
        let backup_path = self.backup_store.path_for(&token);
        let undo_token = token.clone();
        let bundled_backup_path = Some(backup_path.as_path());
        let delete_result = (|| -> anyhow::Result<()> {
            let tx = db.transaction()?;
            delete_related_rows(&tx, "thread_dynamic_tools", "thread_id = ?1", &[&thread_id])?;
            delete_related_rows(&tx, "thread_goals", "thread_id = ?1", &[&thread_id])?;
            delete_related_rows(
                &tx,
                "thread_spawn_edges",
                "parent_thread_id = ?1 OR child_thread_id = ?1",
                &[&thread_id],
            )?;
            delete_related_rows(&tx, "stage1_outputs", "thread_id = ?1", &[&thread_id])?;
            if has_table(&tx, "agent_job_items")?
                && has_columns(&tx, "agent_job_items", &["assigned_thread_id"])?
            {
                tx.execute(
                    "UPDATE agent_job_items SET assigned_thread_id = NULL WHERE assigned_thread_id = ?1",
                    [&thread_id],
                )?;
            }
            tx.execute("DELETE FROM threads WHERE id = ?1", [&thread_id])?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(err) = delete_result {
            return Ok(failed_with_undo(
                &thread_id,
                err.to_string(),
                &undo_token,
                bundled_backup_path,
            ));
        }
        let mut file_errors = Vec::new();
        for file in file_backups {
            if let Some(path) = file.get("path").and_then(Value::as_str) {
                if let Err(err) = fs::remove_file(path) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        file_errors.push(format!("{path}: {err}"));
                    }
                }
            }
        }
        if !file_errors.is_empty() {
            return Ok(DeleteResult {
                status: DeleteStatus::Failed,
                session_id: thread_id,
                message: format!(
                    "本地数据库已删除，但文件删除失败：{}",
                    file_errors.join("; ")
                ),
                undo_token: Some(undo_token),
                backup_path: bundled_backup_path.map(|path| path.to_string_lossy().to_string()),
            });
        }
        if let Some(result) = self.cleanup_thread_references_after_delete(
            &thread_id,
            &undo_token,
            bundled_backup_path,
        ) {
            return Ok(result);
        }
        Ok(local_deleted_with_undo(
            &thread_id,
            &undo_token,
            bundled_backup_path,
        ))
    }

    fn delete_codex_automation_run(
        &self,
        db: &mut Connection,
        session: &SessionRef,
    ) -> anyhow::Result<DeleteResult> {
        let thread_id = normalize_codex_thread_id(&session.session_id);
        let mut tables = Map::new();
        backup_related_rows(
            db,
            &mut tables,
            "automation_runs",
            "thread_id = ?1",
            &[&thread_id],
        )?;
        backup_related_rows(
            db,
            &mut tables,
            "inbox_items",
            "thread_id = ?1",
            &[&thread_id],
        )?;
        if tables.values().all(|rows| {
            rows.as_array()
                .map(|items| items.is_empty())
                .unwrap_or(true)
        }) {
            return Ok(failed(
                &session.session_id,
                "Thread not found in local storage".to_string(),
            ));
        }
        self.prepare_thread_reference_backup(&thread_id, &mut tables)?;
        let token =
            self.backup_store
                .write_backup(&thread_id, &self.db_path, Value::Object(tables))?;
        let backup_path = self.backup_store.path_for(&token);
        let undo_token = token.clone();
        let bundled_backup_path = Some(backup_path.as_path());
        let delete_result = (|| -> anyhow::Result<()> {
            let tx = db.transaction()?;
            delete_related_rows(&tx, "automation_runs", "thread_id = ?1", &[&thread_id])?;
            delete_related_rows(&tx, "inbox_items", "thread_id = ?1", &[&thread_id])?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(err) = delete_result {
            return Ok(failed_with_undo(
                &thread_id,
                err.to_string(),
                &undo_token,
                bundled_backup_path,
            ));
        }
        if let Some(result) = self.cleanup_thread_references_after_delete(
            &thread_id,
            &undo_token,
            bundled_backup_path,
        ) {
            return Ok(result);
        }
        Ok(local_deleted_with_undo(
            &thread_id,
            &undo_token,
            bundled_backup_path,
        ))
    }
}

fn optional_column_expression<'a>(
    columns: &HashSet<String>,
    column: &'a str,
    fallback: &'a str,
) -> &'a str {
    if columns.contains(column) {
        column
    } else {
        fallback
    }
}

fn failed(session_id: &str, message: String) -> DeleteResult {
    DeleteResult {
        status: DeleteStatus::Failed,
        session_id: session_id.to_string(),
        message,
        undo_token: None,
        backup_path: None,
    }
}

fn local_deleted(session_id: &str, token: &str, backup_path: &Path) -> DeleteResult {
    local_deleted_with_undo(session_id, token, Some(backup_path))
}

fn local_deleted_with_undo(
    session_id: &str,
    undo_token: &str,
    backup_path: Option<&Path>,
) -> DeleteResult {
    DeleteResult {
        status: DeleteStatus::LocalDeleted,
        session_id: session_id.to_string(),
        message: "已从本地存储删除".to_string(),
        undo_token: Some(undo_token.to_string()),
        backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
    }
}

fn read_rollout_usage_history(rollout_path: &Path, thread_id: &str) -> anyhow::Result<Vec<Value>> {
    let file = File::open(rollout_path)?;
    let reader = BufReader::new(file);
    let mut current_turn_id = String::new();
    let mut history = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "turn_context" => {
                current_turn_id = value
                    .get("payload")
                    .and_then(|payload| payload.get("turn_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            "event_msg" => {
                let payload = match value.get("payload") {
                    Some(payload)
                        if payload.get("type").and_then(Value::as_str) == Some("token_count") =>
                    {
                        payload
                    }
                    _ => continue,
                };
                let info = match payload.get("info") {
                    Some(info) => info,
                    None => continue,
                };
                let last = info.get("last_token_usage");
                let total = info.get("total_token_usage");
                let model_context_window = info
                    .get("model_context_window")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let input_tokens = last
                    .and_then(|usage| usage.get("input_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let output_tokens = last
                    .and_then(|usage| usage.get("output_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let total_tokens = last
                    .and_then(|usage| usage.get("total_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| {
                        total
                            .and_then(|usage| usage.get("total_tokens"))
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                    });
                let cached_tokens = last
                    .and_then(|usage| usage.get("cached_input_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let context_used = total
                    .and_then(|usage| usage.get("total_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(total_tokens);
                if input_tokens <= 0 && output_tokens <= 0 && total_tokens <= 0 && context_used <= 0
                {
                    continue;
                }
                history.push(json!({
                    "source": "rollout-history",
                    "conversation_id": format!("local:{thread_id}"),
                    "turn_id": current_turn_id,
                    "observed_at": value.get("timestamp").and_then(Value::as_str).unwrap_or_default(),
                    "usage": {
                        "inputTokens": input_tokens,
                        "outputTokens": output_tokens,
                        "totalTokens": total_tokens,
                        "cachedTokens": cached_tokens,
                        "cacheReadTokens": 0,
                        "cacheCreationTokens": 0,
                        "contextUsed": context_used,
                        "contextLimit": model_context_window,
                        "hasBreakdown": input_tokens > 0 || output_tokens > 0 || cached_tokens > 0,
                    }
                }));
            }
            _ => {}
        }
    }

    Ok(history)
}

fn failed_with_undo(
    session_id: &str,
    message: String,
    token: &str,
    backup_path: Option<&Path>,
) -> DeleteResult {
    DeleteResult {
        status: DeleteStatus::Failed,
        session_id: session_id.to_string(),
        message,
        undo_token: Some(token.to_string()),
        backup_path: backup_path.map(|path| path.to_string_lossy().to_string()),
    }
}

fn normalize_codex_thread_id(session_id: &str) -> String {
    session_id
        .strip_prefix("local:")
        .unwrap_or(session_id)
        .to_string()
}

fn undo_backups(backup_store: &BackupStore, token: &str) -> anyhow::Result<Vec<Value>> {
    let tokens = parse_undo_tokens(token);
    if tokens.is_empty() {
        anyhow::bail!("empty undo token");
    }
    tokens
        .into_iter()
        .map(|token| backup_store.read_backup(&token))
        .collect()
}

fn parse_undo_tokens(token: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(token).unwrap_or_else(|_| vec![token.to_string()])
}

fn format_undo_tokens(tokens: &[String]) -> String {
    if tokens.len() == 1 {
        tokens[0].clone()
    } else {
        json!(tokens).to_string()
    }
}

fn restore_backups(
    backups: &[Value],
    fallback_db_path: &Path,
    allowed_db_paths: &[PathBuf],
    codex_home: Option<&Path>,
) -> anyhow::Result<()> {
    for backup in backups {
        let Some(tables) = backup["tables"].as_object() else {
            continue;
        };
        let source_db = backup_source_db(backup, fallback_db_path, allowed_db_paths)?;
        let db = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        validate_restore_tables(tables)?;
        detect_restore_conflicts(&db, tables)?;
        detect_file_restore_conflicts(tables)?;
        preflight_session_index_restore(tables, codex_home)?;
        preflight_global_state_restore(tables, codex_home)?;
        preflight_reference_database_restore(tables, fallback_db_path, allowed_db_paths)?;
        preflight_restore_rows(&db, tables)?;
    }

    for backup in backups {
        let Some(tables) = backup["tables"].as_object() else {
            continue;
        };
        let source_db = backup_source_db(backup, fallback_db_path, allowed_db_paths)?;
        let mut db = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let tx = db.transaction()?;
        restore_rows(&tx, tables)?;
        tx.commit()?;
        if let Some(files) = tables.get("__files").and_then(Value::as_array) {
            for file in files {
                let Some(path) = file.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let Some(content) = file.get("content_b64").and_then(Value::as_str) else {
                    continue;
                };
                let bytes =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)?;
                if Path::new(path).is_file() {
                    let current = fs::read(path)?;
                    if current == bytes {
                        continue;
                    }
                    anyhow::bail!("restore conflict: file changed after preflight: {path}");
                }
                if let Some(parent) = Path::new(path).parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, bytes)?;
            }
        }
    }

    for backup in backups {
        let Some(tables) = backup["tables"].as_object() else {
            continue;
        };
        restore_session_index(tables, codex_home)?;
        restore_global_state_files(tables, codex_home)?;
        restore_reference_databases(tables, fallback_db_path, allowed_db_paths)?;
    }
    Ok(())
}

fn preflight_reference_database_restore(
    tables: &Map<String, Value>,
    fallback_db_path: &Path,
    allowed_db_paths: &[PathBuf],
) -> anyhow::Result<()> {
    let Some(backups) = tables
        .get("__reference_databases")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    for backup in backups {
        let Some(reference_tables) = backup.get("tables").and_then(Value::as_object) else {
            anyhow::bail!("invalid reference database backup");
        };
        validate_reference_restore_tables(reference_tables)?;
        let source_db = backup_source_db(backup, fallback_db_path, allowed_db_paths)?;
        let db = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        detect_restore_conflicts(&db, reference_tables)?;
        preflight_restore_rows(&db, reference_tables)?;
    }
    Ok(())
}

fn restore_reference_databases(
    tables: &Map<String, Value>,
    fallback_db_path: &Path,
    allowed_db_paths: &[PathBuf],
) -> anyhow::Result<()> {
    let Some(backups) = tables
        .get("__reference_databases")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    for backup in backups {
        let Some(reference_tables) = backup.get("tables").and_then(Value::as_object) else {
            anyhow::bail!("invalid reference database backup");
        };
        validate_reference_restore_tables(reference_tables)?;
        let source_db = backup_source_db(backup, fallback_db_path, allowed_db_paths)?;
        let mut db = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let tx = db.transaction()?;
        restore_rows(&tx, reference_tables)?;
        tx.commit()?;
    }
    Ok(())
}

fn validate_reference_restore_tables(tables: &Map<String, Value>) -> anyhow::Result<()> {
    for table in tables.keys() {
        if !THREAD_REFERENCE_TABLES
            .iter()
            .any(|(allowed, _)| table == allowed)
        {
            anyhow::bail!("unknown reference restore table: {table}");
        }
    }
    Ok(())
}

fn preflight_session_index_restore(
    tables: &Map<String, Value>,
    codex_home: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(entries) = tables.get("__session_index").and_then(Value::as_array) else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    let home = codex_home
        .ok_or_else(|| anyhow::anyhow!("Codex home is required to restore session_index.jsonl"))?;
    let path = home.join("session_index.jsonl");
    if path.exists() && !path.is_file() {
        anyhow::bail!("session_index.jsonl restore path is not a file");
    }
    if path.is_file() {
        let _ = String::from_utf8(fs::read(&path)?)?;
    }
    for entry in entries {
        let line = entry
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid session_index.jsonl backup entry"))?;
        if session_index_line_id(line).is_none() {
            anyhow::bail!("invalid session_index.jsonl backup line");
        }
    }
    Ok(())
}

fn restore_session_index(
    tables: &Map<String, Value>,
    codex_home: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(entries) = tables.get("__session_index").and_then(Value::as_array) else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    let home = codex_home
        .ok_or_else(|| anyhow::anyhow!("Codex home is required to restore session_index.jsonl"))?;
    let path = home.join("session_index.jsonl");
    let mut text = if path.is_file() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let mut existing_ids = text
        .lines()
        .filter_map(session_index_line_id)
        .collect::<HashSet<_>>();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let mut expected_ids = Vec::new();
    let mut changed = false;
    for entry in entries {
        let line = entry
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid session_index.jsonl backup entry"))?;
        let id = session_index_line_id(line)
            .ok_or_else(|| anyhow::anyhow!("invalid session_index.jsonl backup line"))?;
        expected_ids.push(id.clone());
        if existing_ids.insert(id) {
            text.push_str(line);
            text.push('\n');
            changed = true;
        }
    }
    if changed {
        codex_plus_core::settings::atomic_write(&path, text.as_bytes())?;
    }
    let restored = fs::read_to_string(&path)?;
    let restored_ids = restored
        .lines()
        .filter_map(session_index_line_id)
        .collect::<HashSet<_>>();
    if expected_ids.iter().any(|id| !restored_ids.contains(id)) {
        anyhow::bail!("session_index.jsonl restore verification failed");
    }
    Ok(())
}

fn session_index_line_id(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("id")?
        .as_str()
        .map(ToString::to_string)
}

fn preflight_global_state_restore(
    tables: &Map<String, Value>,
    codex_home: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(entries) = tables.get("__global_state_files").and_then(Value::as_array) else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    let home = codex_home
        .ok_or_else(|| anyhow::anyhow!("Codex home is required to restore global state"))?;
    for entry in entries {
        let (path, original, cleaned) = decode_global_state_backup(home, entry)?;
        let current = fs::read(&path)?;
        if current != original && current != cleaned {
            anyhow::bail!(
                "restore conflict: Codex global state changed after deletion: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn restore_global_state_files(
    tables: &Map<String, Value>,
    codex_home: Option<&Path>,
) -> anyhow::Result<()> {
    let Some(entries) = tables.get("__global_state_files").and_then(Value::as_array) else {
        return Ok(());
    };
    if entries.is_empty() {
        return Ok(());
    }
    let home = codex_home
        .ok_or_else(|| anyhow::anyhow!("Codex home is required to restore global state"))?;
    for entry in entries {
        let (path, original, cleaned) = decode_global_state_backup(home, entry)?;
        let current = fs::read(&path)?;
        if current == original {
            continue;
        }
        if current != cleaned {
            anyhow::bail!(
                "restore conflict: Codex global state changed after deletion: {}",
                path.display()
            );
        }
        codex_plus_core::settings::atomic_write(&path, &original)?;
        if fs::read(&path)? != original {
            anyhow::bail!(
                "Codex global state restore verification failed: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn decode_global_state_backup(
    home: &Path,
    entry: &Value,
) -> anyhow::Result<(PathBuf, Vec<u8>, Vec<u8>)> {
    let relative_path = entry
        .get("relative_path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("invalid global state backup path"))?;
    if !GLOBAL_STATE_RELATIVE_PATHS.contains(&relative_path) {
        anyhow::bail!("unexpected global state backup path: {relative_path}");
    }
    let decode = |key: &str| -> anyhow::Result<Vec<u8>> {
        let value = entry
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("invalid global state backup content"))?;
        Ok(base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            value,
        )?)
    };
    Ok((
        home.join(relative_path),
        decode("original_b64")?,
        decode("cleaned_b64")?,
    ))
}

fn preflight_restore_rows(db: &Connection, tables: &Map<String, Value>) -> anyhow::Result<()> {
    db.execute_batch("SAVEPOINT codex_plus_restore_preflight")?;
    let restore_result = restore_rows(db, tables);
    let rollback_result = db.execute_batch(
        "ROLLBACK TO codex_plus_restore_preflight; RELEASE codex_plus_restore_preflight",
    );
    restore_result?;
    rollback_result?;
    Ok(())
}

fn restore_rows(db: &Connection, tables: &Map<String, Value>) -> anyhow::Result<()> {
    for (table, rows) in tables {
        if table.starts_with("__") {
            continue;
        }
        let Some(rows) = rows.as_array() else {
            continue;
        };
        for row in rows {
            if let Some(row) = row.as_object() {
                if table == "agent_job_items" && update_existing_agent_job_item(db, row)? {
                    continue;
                }
                match restore_row_state(db, table, row)? {
                    RestoreRowState::Matching => continue,
                    RestoreRowState::Conflict => {
                        anyhow::bail!("restore conflict: {table} row differs from backup")
                    }
                    RestoreRowState::Missing => {}
                }
                insert_row(db, table, row)?;
            }
        }
    }
    Ok(())
}

fn backup_source_db(
    backup: &Value,
    fallback_db_path: &Path,
    allowed_db_paths: &[PathBuf],
) -> anyhow::Result<PathBuf> {
    let source_db = backup["source_db"]
        .as_str()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback_db_path.to_path_buf());
    if !source_db.is_file() {
        anyhow::bail!(
            "Backup source database not found: {}",
            source_db.to_string_lossy()
        );
    }
    let source_db = fs::canonicalize(source_db)?;
    let allowed = allowed_db_paths
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .any(|path| path == source_db);
    if !allowed {
        anyhow::bail!("Backup source database is not an allowed local storage path");
    }
    Ok(source_db)
}

fn schema_kind(db: &Connection) -> anyhow::Result<Option<SchemaKind>> {
    if has_table(db, "sessions")? && has_columns(db, "sessions", &["id", "title"])? {
        if has_table(db, "messages")? && !has_columns(db, "messages", &["session_id"])? {
            return Ok(None);
        }
        return Ok(Some(SchemaKind::GenericSessions));
    }
    if has_table(db, "threads")? && has_columns(db, "threads", &["id", "title", "rollout_path"])? {
        return Ok(Some(SchemaKind::CodexThreads));
    }
    if has_table(db, "automation_runs")? && has_columns(db, "automation_runs", &["thread_id"])? {
        return Ok(Some(SchemaKind::CodexAutomationRuns));
    }
    Ok(None)
}

fn has_table(db: &Connection, table: &str) -> anyhow::Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .is_ok())
}

fn has_columns(db: &Connection, table: &str, columns: &[&str]) -> anyhow::Result<bool> {
    let existing: HashSet<String> = table_columns(db, table)?.into_iter().collect();
    Ok(columns.iter().all(|column| existing.contains(*column)))
}

fn table_columns(db: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = db.prepare(&format!(
        "PRAGMA table_info(\"{}\")",
        table.replace('"', "\"\"")
    ))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn select_dicts(db: &Connection, sql: &str, params: &[&dyn ToSql]) -> anyhow::Result<Vec<Value>> {
    let mut stmt = db.prepare(sql)?;
    let columns: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let rows = stmt.query_map(params, |row| {
        let mut data = Map::new();
        for (index, column) in columns.iter().enumerate() {
            data.insert(column.clone(), sql_value_to_json(row.get_ref(index)?));
        }
        Ok(Value::Object(data))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn validate_restore_tables(tables: &Map<String, Value>) -> anyhow::Result<()> {
    let allowed = [
        "sessions",
        "messages",
        "threads",
        "thread_dynamic_tools",
        "thread_goals",
        "thread_spawn_edges",
        "stage1_outputs",
        "agent_job_items",
        "automation_runs",
        "inbox_items",
        "__files",
        "__session_index",
        "__global_state_files",
        "__reference_databases",
    ];
    for table in tables.keys() {
        if !allowed.contains(&table.as_str()) {
            anyhow::bail!("unknown restore table: {table}");
        }
    }
    Ok(())
}

fn detect_restore_conflicts(db: &Connection, tables: &Map<String, Value>) -> anyhow::Result<()> {
    for (table, rows) in tables {
        if table.starts_with("__") {
            continue;
        }
        let Some(rows) = rows.as_array() else {
            continue;
        };
        for row in rows {
            let Some(row) = row.as_object() else {
                continue;
            };
            if table == "agent_job_items" {
                continue;
            }
            if matches!(
                restore_row_state(db, table, row)?,
                RestoreRowState::Conflict
            ) {
                anyhow::bail!("restore conflict: {table} row differs from backup");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreRowState {
    Missing,
    Matching,
    Conflict,
}

fn restore_row_state(
    db: &Connection,
    table: &str,
    row: &Map<String, Value>,
) -> anyhow::Result<RestoreRowState> {
    if !has_table(db, table)? {
        return Ok(RestoreRowState::Missing);
    }
    let available_columns = table_columns(db, table)?
        .into_iter()
        .collect::<HashSet<_>>();
    if row.keys().any(|column| !available_columns.contains(column)) {
        return Ok(RestoreRowState::Missing);
    }
    let key_columns = restore_conflict_key_columns(db, table, row)?;
    if key_columns.is_empty() {
        return Ok(RestoreRowState::Missing);
    }
    let where_clause = key_columns
        .iter()
        .enumerate()
        .map(|(index, column)| format!("\"{}\" = ?{}", column.replace('"', "\"\""), index + 1))
        .collect::<Vec<_>>()
        .join(" AND ");
    let values = key_columns
        .iter()
        .map(|column| OwnedSqlValue(json_to_sql_value(&row[column])))
        .collect::<Vec<_>>();
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let columns = row.keys().collect::<Vec<_>>();
    let selected = columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = db.prepare(&format!(
        "SELECT {selected} FROM \"{table}\" WHERE {where_clause}"
    ))?;
    let candidates = statement.query_map(refs.as_slice(), |candidate| {
        let mut existing = Map::new();
        for (index, column) in columns.iter().enumerate() {
            existing.insert(
                (*column).clone(),
                sql_value_to_json(candidate.get_ref(index)?),
            );
        }
        Ok(existing)
    })?;
    let candidates = candidates.collect::<rusqlite::Result<Vec<_>>>()?;
    if candidates.is_empty() {
        Ok(RestoreRowState::Missing)
    } else if candidates.iter().any(|candidate| candidate == row) {
        Ok(RestoreRowState::Matching)
    } else {
        Ok(RestoreRowState::Conflict)
    }
}

fn restore_conflict_key_columns(
    db: &Connection,
    table: &str,
    row: &Map<String, Value>,
) -> anyhow::Result<Vec<String>> {
    let primary = table_primary_key_columns(db, table)?
        .into_iter()
        .filter(|column| row.contains_key(column))
        .collect::<Vec<_>>();
    if !primary.is_empty() {
        return Ok(primary);
    }
    let wanted: &[&str] = match table {
        "sessions" | "threads" => &["id"],
        "messages" => &["id"],
        "automation_runs" | "inbox_items" => &["thread_id"],
        "local_thread_catalog" => &["thread_id"],
        "thread_timeline_ledger" => &["thread_id", "sequence"],
        "thread_dynamic_tools" => &["thread_id", "tool_name"],
        "thread_goals" => &["thread_id", "goal"],
        "thread_spawn_edges" => &["parent_thread_id", "child_thread_id"],
        "stage1_outputs" => &["thread_id"],
        _ => &[],
    };
    let keys = wanted
        .iter()
        .filter(|column| row.contains_key(**column))
        .map(|column| (*column).to_string())
        .collect::<Vec<_>>();
    if table == "messages" && keys.is_empty() {
        Ok(row
            .contains_key("session_id")
            .then(|| vec!["session_id".to_string()])
            .unwrap_or_default())
    } else {
        Ok(keys)
    }
}

fn table_primary_key_columns(db: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    if !has_table(db, table)? {
        return Ok(Vec::new());
    }
    let mut statement = db.prepare(&format!(
        "PRAGMA table_info(\"{}\")",
        table.replace('"', "\"\"")
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
    })?;
    let mut columns = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(order, _)| *order > 0)
        .collect::<Vec<_>>();
    columns.sort_by_key(|(order, _)| *order);
    Ok(columns.into_iter().map(|(_, name)| name).collect())
}

fn detect_file_restore_conflicts(tables: &Map<String, Value>) -> anyhow::Result<()> {
    let Some(files) = tables.get("__files").and_then(Value::as_array) else {
        return Ok(());
    };
    let allowed_paths = allowed_backup_file_paths(tables);
    for file in files {
        if let Some(path) = file.get("path").and_then(Value::as_str) {
            if !allowed_paths.contains(path) {
                anyhow::bail!("unexpected backup file path: {path}");
            }
            if Path::new(path).exists() {
                let content = file
                    .get("content_b64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("backup file content is missing: {path}"))?;
                let expected =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)?;
                if fs::read(path)? != expected {
                    anyhow::bail!("restore conflict: file differs from backup: {path}");
                }
            } else if let Some(content) = file.get("content_b64").and_then(Value::as_str) {
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)?;
            }
        }
    }
    Ok(())
}

fn allowed_backup_file_paths(tables: &Map<String, Value>) -> HashSet<String> {
    tables
        .get("threads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("rollout_path").and_then(Value::as_str))
        .filter(|path| !path.trim().is_empty())
        .map(ToString::to_string)
        .collect()
}

fn insert_row(db: &Connection, table: &str, row: &Map<String, Value>) -> anyhow::Result<()> {
    let columns: Vec<&String> = row.keys().collect();
    if columns.is_empty() {
        return Ok(());
    }
    let quoted = columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let marks = (0..columns.len())
        .map(|index| format!("?{}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let values = columns
        .iter()
        .map(|column| OwnedSqlValue(json_to_sql_value(&row[*column])))
        .collect::<Vec<_>>();
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    db.execute(
        &format!("INSERT INTO \"{table}\" ({quoted}) VALUES ({marks})"),
        refs.as_slice(),
    )?;
    Ok(())
}

fn update_existing_agent_job_item(
    db: &Connection,
    row: &Map<String, Value>,
) -> anyhow::Result<bool> {
    let Some(id) = row.get("id") else {
        return Ok(false);
    };
    if !row.contains_key("assigned_thread_id") || !has_table(db, "agent_job_items")? {
        return Ok(false);
    }
    let id_value = OwnedSqlValue(json_to_sql_value(id));
    let current_assignment = db.query_row(
        "SELECT assigned_thread_id FROM agent_job_items WHERE id = ?1 LIMIT 1",
        [&id_value as &dyn ToSql],
        |row| row.get::<_, Option<String>>(0),
    );
    let current_assignment = match current_assignment {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    let expected_assignment = row["assigned_thread_id"].as_str().map(ToString::to_string);
    if current_assignment == expected_assignment {
        return Ok(true);
    }
    if current_assignment.is_some() {
        anyhow::bail!("restore conflict: agent_job_items row already assigned");
    }
    let assigned = OwnedSqlValue(json_to_sql_value(&row["assigned_thread_id"]));
    db.execute(
        "UPDATE agent_job_items SET assigned_thread_id = ?1 WHERE id = ?2 AND assigned_thread_id IS NULL",
        [&assigned as &dyn ToSql, &id_value as &dyn ToSql],
    )?;
    Ok(true)
}

fn backup_related_rows(
    db: &Connection,
    tables: &mut Map<String, Value>,
    table: &str,
    where_clause: &str,
    params: &[&dyn ToSql],
) -> anyhow::Result<()> {
    if has_table(db, table)? {
        let rows = select_dicts(
            db,
            &format!("SELECT * FROM \"{table}\" WHERE {where_clause}"),
            params,
        )?;
        tables.insert(table.to_string(), Value::Array(rows));
    }
    Ok(())
}

fn delete_related_rows(
    db: &Connection,
    table: &str,
    where_clause: &str,
    params: &[&dyn ToSql],
) -> anyhow::Result<()> {
    if has_table(db, table)? {
        db.execute(
            &format!("DELETE FROM \"{table}\" WHERE {where_clause}"),
            params,
        )?;
    }
    Ok(())
}

fn rollout_file_backups(thread_rows: Option<&Vec<Value>>) -> Vec<Value> {
    thread_rows
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("rollout_path").and_then(Value::as_str))
        .filter_map(|path| {
            let bytes = fs::read(path).ok()?;
            Some(json!({
                "path": path,
                "content_b64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
            }))
        })
        .collect()
}

fn sql_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => json!(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => json!(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            value
        )),
    }
}

fn json_to_sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(i64::from(*value)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                SqlValue::Integer(value)
            } else if let Some(value) = number.as_f64() {
                SqlValue::Real(value)
            } else {
                SqlValue::Text(number.to_string())
            }
        }
        Value::String(value) => SqlValue::Text(value.clone()),
        other => SqlValue::Text(other.to_string()),
    }
}
