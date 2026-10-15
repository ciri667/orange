//! Markdown 混合检索与后台向量生成。FTS、结构化过滤和引用仍复用本地 chunk。
pub(crate) mod provider;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/** 查询优先：有查询正在等待网络时，不开始下一个后台批次。 */
static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
pub(crate) struct QueryGuard;
impl QueryGuard {
    pub fn new() -> Self {
        ACTIVE_QUERIES.fetch_add(1, Ordering::SeqCst);
        Self
    }
}
impl Drop for QueryGuard {
    fn drop(&mut self) {
        ACTIVE_QUERIES.fetch_sub(1, Ordering::SeqCst);
    }
}

/** 唯一应用级 worker。任务在 SQLite 中持久化，进程结束不丢失待处理状态。 */
pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let startup = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let connection = crate::storage::open_database(&startup)?;
            let _guard = crate::storage::lock_database_writer()?;
            connection
                .execute("UPDATE semantic_index_state SET active=0", [])
                .map_err(|_| "无法恢复索引状态。".to_owned())?;
            Ok::<_, String>(())
        })
        .await;
        let mut previous = Vec::new();
        loop {
            if ACTIVE_QUERIES.load(Ordering::SeqCst) == 0 {
                let work_app = app.clone();
                if let Ok(Ok((settings, jobs))) = tauri::async_runtime::spawn_blocking(move || {
                    crate::storage::next_semantic_jobs(&work_app)
                })
                .await
                {
                    if !jobs.is_empty() {
                        emit_progress(&app, &mut previous).await;
                        process_batch(&app, &settings, jobs, &mut previous).await;
                    }
                }
            }
            emit_progress(&app, &mut previous).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn emit_progress(app: &AppHandle, previous: &mut Vec<crate::domain::SemanticIndexStatus>) {
    let status_app = app.clone();
    if let Ok(Ok(statuses)) = tauri::async_runtime::spawn_blocking(move || {
        crate::storage::load_semantic_index_statuses(&status_app)
    })
    .await
    {
        if *previous != statuses {
            let _ = app.emit("semantic-index-progress", &statuses);
            *previous = statuses;
        }
    }
}

async fn process_batch(
    app: &AppHandle,
    settings: &crate::domain::UserSettings,
    jobs: Vec<crate::storage::SemanticJob>,
    previous: &mut Vec<crate::domain::SemanticIndexStatus>,
) {
    let validate_app = app.clone();
    let jobs = match tauri::async_runtime::spawn_blocking(move || {
        crate::storage::validate_semantic_jobs(&validate_app, &jobs)
    })
    .await
    {
        Ok(Ok(jobs)) if !jobs.is_empty() => jobs,
        _ => return,
    };
    let key_settings = settings.semantic_search.clone();
    let key = tauri::async_runtime::spawn_blocking(move || {
        crate::storage::load_semantic_key(&key_settings)
    })
    .await;
    let key = match key {
        Ok(Ok(key)) => key,
        _ => {
            finish(
                app,
                jobs,
                Err(provider::EmbeddingError {
                    message: "请先保存向量服务密钥。",
                    retryable: false,
                    batch_unsupported: false,
                    configuration: true,
                }),
            )
            .await;
            return;
        }
    };
    let inputs = jobs.iter().map(|j| j.input.clone()).collect::<Vec<_>>();
    let result = provider::embed(
        &settings.semantic_search,
        &key,
        &inputs,
        Duration::from_secs(60),
    )
    .await;
    if result.as_ref().is_err_and(|e| e.batch_unsupported) {
        for job in jobs {
            // 兼容不支持数组的服务，同时让前台查询优先取得下一次请求机会。
            while ACTIVE_QUERIES.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let validate_app = app.clone();
            let candidate = job.clone();
            let allowed = tauri::async_runtime::spawn_blocking(move || {
                crate::storage::validate_semantic_jobs(&validate_app, &[candidate])
            })
            .await;
            if !matches!(allowed,Ok(Ok(ref valid)) if !valid.is_empty()) {
                continue;
            }
            let result = provider::embed(
                &settings.semantic_search,
                &key,
                &[job.input.clone()],
                Duration::from_secs(60),
            )
            .await;
            let configuration_error = result.as_ref().is_err_and(|e| e.configuration);
            finish(app, vec![job], result).await;
            emit_progress(app, previous).await;
            if configuration_error {
                break;
            }
        }
    } else {
        finish(app, jobs, result).await;
    }
}

async fn finish(
    app: &AppHandle,
    jobs: Vec<crate::storage::SemanticJob>,
    result: Result<Vec<Vec<f32>>, provider::EmbeddingError>,
) {
    let finish_app = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::storage::finish_semantic_batch(&finish_app, &jobs, result)
    })
    .await;
}
