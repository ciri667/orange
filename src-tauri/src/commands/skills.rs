use super::common::*;

/** 读取内置和用户自建 skills，内置定义会合并用户保存的启停偏好。 */
#[tauri::command]
pub async fn load_agent_skills(app: AppHandle) -> Result<Vec<AgentSkill>, String> {
    run_blocking("读取 Skills", move || {
        let connection = storage::open_database(&app)?;

        skills::load_agent_skills(&app, &connection)
    })
    .await
}

/** 打开橘记 用户 Skills 文件夹，浏览器开发态由前端 mock 只展示路径。 */
#[tauri::command]
pub async fn open_user_skills_folder(app: AppHandle) -> Result<String, String> {
    let skills_app = app.clone();
    let started_at = Instant::now();
    let result = run_blocking("打开用户 Skills 文件夹", move || {
        let skills_root = skills::user_skills_root(&skills_app)?;

        open_folder_in_system(&skills_root)?;

        Ok(skills_root.to_string_lossy().to_string())
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "open_user_skills_folder",
                "completed",
                "已打开用户 Skills 文件夹。",
            )
            .duration(started_at.elapsed()),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "open_user_skills_folder",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 新增或编辑用户自建 skill；内置 skill 只能通过启停入口修改状态。 */
#[tauri::command]
pub async fn save_agent_skill(
    app: AppHandle,
    payload: SaveAgentSkillPayload,
) -> Result<AgentSkill, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill.id.clone();
    let skill_name = payload.skill.name.clone();
    let started_at = Instant::now();
    let result = run_blocking("保存 Skill", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::save_user_skill(&skills_app, &connection, payload.skill)
    })
    .await;

    match &result {
        Ok(saved_skill) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "save_agent_skill",
                "completed",
                "已保存 Skill。",
            )
            .duration(started_at.elapsed())
            .entity("skill", saved_skill.id.clone())
            .metadata(
                json!({ "name": saved_skill.name.clone(), "source": saved_skill.source.clone() }),
            ),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "save_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id)
            .metadata(json!({ "name": skill_name })),
        ),
    }

    result
}

/** 启停 skill；启用的 skill 会以名称和描述进入 Agent system prompt。 */
#[tauri::command]
pub async fn toggle_agent_skill(
    app: AppHandle,
    payload: ToggleAgentSkillPayload,
) -> Result<AgentSkill, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill_id.clone();
    let enabled = payload.enabled;
    let started_at = Instant::now();
    let result = run_blocking("更新 Skill 状态", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::toggle_agent_skill(&skills_app, &connection, &payload.skill_id, payload.enabled)
    })
    .await;

    match &result {
        Ok(skill) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "toggle_agent_skill",
                "completed",
                "已更新 Skill 状态。",
            )
            .duration(started_at.elapsed())
            .entity("skill", skill.id.clone())
            .metadata(json!({ "enabled": skill.enabled })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "toggle_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id)
            .metadata(json!({ "enabled": enabled })),
        ),
    }

    result
}

/** 删除用户自建 skill；内置 skill 必须保留供用户重新启用。 */
#[tauri::command]
pub async fn delete_agent_skill(
    app: AppHandle,
    payload: DeleteAgentSkillPayload,
) -> Result<Vec<AgentSkill>, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill_id.clone();
    let started_at = Instant::now();
    let result = run_blocking("删除 Skill", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::delete_user_skill(&skills_app, &connection, &payload.skill_id)?;
        skills::load_agent_skills(&skills_app, &connection)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "delete_agent_skill",
                "completed",
                "已删除用户 Skill。",
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id.clone()),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "delete_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id),
        ),
    }

    result
}

/** 安装第三方 Skill 包；默认停用，用户审阅后再手动启用。 */
#[tauri::command]
pub async fn install_agent_skill(
    app: AppHandle,
    payload: InstallAgentSkillPayload,
) -> Result<InstallAgentSkillResult, String> {
    let source_type = payload.source_type.clone();
    let conflict_strategy = payload.conflict_strategy.clone();
    let enable_after_install = payload.enable_after_install;
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Skill,
            "install_agent_skill",
            "started",
            "开始安装第三方 Skill。",
        )
        .operation_id(operation_id.clone())
        .metadata(json!({
            "sourceType": source_type.clone(),
            "conflictStrategy": conflict_strategy.clone(),
            "enableAfterInstall": enable_after_install,
        })),
    );

    let prepare_result = prepare_skill_install_source(&app, &payload).await;
    let result = match prepare_result {
        Ok(prepared_source) => {
            let install_app = app.clone();
            let install_source_type = source_type.clone();
            let install_conflict_strategy = conflict_strategy.clone();
            let install_enable_after_install = enable_after_install;

            run_blocking("安装 Skill", move || {
                let connection = storage::open_database(&install_app)?;
                let skills_root = skills::user_skills_root(&install_app)?;

                skills::install_agent_skills_from_prepared_root(
                    &connection,
                    &skills_root,
                    prepared_source.root_path(),
                    skills::SkillInstallOptions {
                        source_type: install_source_type,
                        source_summary: prepared_source.source_summary().to_owned(),
                        enable_after_install: install_enable_after_install,
                        conflict_strategy: install_conflict_strategy,
                    },
                )
            })
            .await
        }
        Err(error) => Err(error),
    };

    match &result {
        Ok(install_result) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "install_agent_skill",
                "completed",
                "已安装第三方 Skill。",
            )
            .operation_id(operation_id)
            .duration(started_at.elapsed())
            .metadata(json!({
                "sourceType": install_result.source_type.clone(),
                "sourceSummary": install_result.source_summary.clone(),
                "installedCount": install_result.installed_count,
                "fileCount": install_result.file_count,
                "warningCount": install_result.warnings.len(),
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "install_agent_skill",
                "failed",
                error,
            )
            .operation_id(operation_id)
            .duration(started_at.elapsed())
            .metadata(json!({
                "sourceType": source_type,
                "conflictStrategy": conflict_strategy,
                "enableAfterInstall": enable_after_install,
            })),
        ),
    }

    result
}

/** 已准备好的安装来源，TempDir 持有临时目录生命周期直到后台安装结束。 */
pub(super) enum PreparedSkillInstallSource {
    Borrowed {
        path: PathBuf,
        source_summary: String,
    },
    Temp {
        temp_dir: tempfile::TempDir,
        source_summary: String,
    },
}

impl PreparedSkillInstallSource {
    /** 返回统一安装管线可读取的根目录。 */
    fn root_path(&self) -> &Path {
        match self {
            PreparedSkillInstallSource::Borrowed { path, .. } => path.as_path(),
            PreparedSkillInstallSource::Temp { temp_dir, .. } => temp_dir.path(),
        }
    }

    /** 返回已脱敏的来源摘要，用于日志、UI 和安装元数据。 */
    fn source_summary(&self) -> &str {
        match self {
            PreparedSkillInstallSource::Borrowed { source_summary, .. }
            | PreparedSkillInstallSource::Temp { source_summary, .. } => source_summary,
        }
    }
}

/** 根据 payload 准备安装来源；本地来源未传路径时打开系统选择器。 */
pub(super) async fn prepare_skill_install_source(
    app: &AppHandle,
    payload: &InstallAgentSkillPayload,
) -> Result<PreparedSkillInstallSource, String> {
    match payload.source_type.as_str() {
        "url" => {
            prepare_url_skill_install_source(payload.source.as_deref().unwrap_or_default()).await
        }
        "localFolder" => {
            let path = match payload
                .source
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                Some(path) => PathBuf::from(path),
                None => pick_skill_folder(app).await?,
            };

            if !path.exists() || !path.is_dir() {
                return Err("请选择有效的 Skill 文件夹。".to_owned());
            }

            Ok(PreparedSkillInstallSource::Borrowed {
                source_summary: summarize_local_install_source(&path),
                path,
            })
        }
        "localArchive" => {
            let path = match payload
                .source
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                Some(path) => PathBuf::from(path),
                None => pick_skill_archive(app).await?,
            };

            if !path.exists() || !path.is_file() {
                return Err("请选择有效的 Skill zip 文件。".to_owned());
            }

            let bytes = read_limited_file(&path, skills::MAX_REMOTE_SKILL_ARCHIVE_BYTES)?;
            let temp_dir = skills::prepare_skill_archive_bytes(&bytes)?;

            Ok(PreparedSkillInstallSource::Temp {
                source_summary: summarize_local_install_source(&path),
                temp_dir,
            })
        }
        _ => Err("未知的 Skill 安装来源类型。".to_owned()),
    }
}

/** 下载远程 Skill 来源并转换成统一的临时目录。 */
pub(super) async fn prepare_url_skill_install_source(
    url: &str,
) -> Result<PreparedSkillInstallSource, String> {
    let download = skills::resolve_skill_url_download(url)?;
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("无法创建 Skill 下载客户端：{error}"))?
        .get(&download.url)
        .header(
            reqwest::header::ACCEPT,
            "text/markdown, application/zip, */*",
        )
        .send()
        .await
        .map_err(|error| format!("下载 Skill 失败：{error}"))?;

    if !response.status().is_success() {
        return Err(format!("下载 Skill 失败：HTTP {}", response.status()));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    let is_archive_download = matches!(download.kind, skills::SkillUrlDownloadKind::Archive)
        || content_type.contains("zip")
        || download.url.ends_with(".zip");
    let max_bytes = if is_archive_download {
        skills::MAX_REMOTE_SKILL_ARCHIVE_BYTES
    } else {
        skills::MAX_REMOTE_SKILL_MARKDOWN_BYTES
    };
    let bytes = read_limited_response_bytes(response, max_bytes, is_archive_download).await?;
    let temp_dir = if is_archive_download {
        skills::prepare_skill_archive_bytes(&bytes)?
    } else {
        let markdown = String::from_utf8(bytes)
            .map_err(|_| "远程 Skill 内容不是有效 UTF-8 文本。".to_owned())?;

        skills::prepare_single_skill_markdown(&markdown)?
    };

    Ok(PreparedSkillInstallSource::Temp {
        source_summary: download.source_summary,
        temp_dir,
    })
}

/** 按最大字节数读取远程响应体，Content-Length 缺失时也能在流式读取过程中截断。 */
pub(super) async fn read_limited_response_bytes(
    mut response: reqwest::Response,
    max_bytes: usize,
    is_archive: bool,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_bytes as u64)
    {
        return Err(remote_skill_size_limit_message(is_archive));
    }

    let mut bytes = Vec::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取 Skill 下载内容失败：{error}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(remote_skill_size_limit_message(is_archive));
        }

        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

/** 返回远程下载大小限制提示，避免多个下载分支各自硬编码文案。 */
pub(super) fn remote_skill_size_limit_message(is_archive: bool) -> String {
    if is_archive {
        "远程 Skill 压缩包超过 25MB，已阻止安装。".to_owned()
    } else {
        "远程 SKILL.md 超过 1MB，已阻止安装。".to_owned()
    }
}

/** 打开系统目录选择器选择待安装 Skill 文件夹。 */
pub(super) async fn pick_skill_folder(app: &AppHandle) -> Result<PathBuf, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择 Skill 文件夹")
        .pick_folder(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    receiver
        .recv()
        .await
        .flatten()
        .and_then(|path| path.as_path().map(PathBuf::from))
        .ok_or_else(|| "未选择 Skill 文件夹。".to_owned())
}

/** 打开系统文件选择器选择待安装 Skill zip。 */
pub(super) async fn pick_skill_archive(app: &AppHandle) -> Result<PathBuf, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择 Skill zip 文件")
        .add_filter("Zip archive", &["zip"])
        .pick_file(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    receiver
        .recv()
        .await
        .flatten()
        .and_then(|path| path.as_path().map(PathBuf::from))
        .ok_or_else(|| "未选择 Skill zip 文件。".to_owned())
}

/** 读取本地压缩包并限制最大字节数，避免大文件通过 IPC 之外的路径阻塞安装。 */
pub(super) fn read_limited_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法读取 Skill 文件元数据：{error}"))?;

    if metadata.len() > max_bytes as u64 {
        return Err("Skill zip 文件超过 25MB，已阻止安装。".to_owned());
    }

    let mut file =
        fs::File::open(path).map_err(|error| format!("无法读取 Skill zip 文件：{error}"))?;
    let mut bytes = Vec::new();

    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 Skill zip 文件：{error}"))?;

    if bytes.len() > max_bytes {
        return Err("Skill zip 文件超过 25MB，已阻止安装。".to_owned());
    }

    Ok(bytes)
}

/** 生成本地安装来源摘要，只保留文件或目录名，避免日志写入绝对路径。 */
pub(super) fn summarize_local_install_source(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| format!("local:{name}"))
        .unwrap_or_else(|| "local".to_owned())
}
