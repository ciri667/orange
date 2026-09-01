use super::*;

pub fn default_user_settings() -> UserSettings {
    let now = format_local_datetime();

    UserSettings {
        model_config: ModelConfig {
            enabled: false,
            default_provider_id: model_provider::MIGRATED_DEFAULT_PROVIDER_ID.to_owned(),
            providers: vec![LlmProviderConfig {
                id: model_provider::MIGRATED_DEFAULT_PROVIDER_ID.to_owned(),
                name: "默认 Provider".to_owned(),
                provider: model_provider::DEFAULT_PROVIDER_TYPE.to_owned(),
                api_base: "https://api.openai.com/v1".to_owned(),
                model: "gpt-4o-mini".to_owned(),
                key_reference: MODEL_KEY_REFERENCE.to_owned(),
                enabled: false,
                supports_tools: true,
                requires_api_key: true,
                models: Vec::new(),
                models_fetched_at: None,
                created_at: now.clone(),
                updated_at: now,
            }],
        },
        privacy_policy: "allow-selected-scope".to_owned(),
        write_confirmation_required: true,
        agent_security: Default::default(),
    }
}

/** 返回即时通讯默认配置；默认不启用，必须由用户显式填写凭证和白名单。 */
pub fn default_im_settings() -> ImIntegrationSettings {
    ImIntegrationSettings {
        providers: vec![default_feishu_provider_settings()],
    }
}

/** 构造飞书 provider 默认配置；后续新增 provider 时保留同样的隔离构造函数。 */
pub(crate) fn default_feishu_provider_settings() -> ImProviderSettings {
    ImProviderSettings::from_feishu(FeishuIntegrationSettings {
        enabled: false,
        domain: "feishu".to_owned(),
        app_id: String::new(),
        secret_key_reference: FEISHU_SECRET_KEY_REFERENCE.to_owned(),
        default_knowledge_base_ids: Vec::new(),
        allowed_user_open_ids: Vec::new(),
        allowed_chat_ids: Vec::new(),
        discovered_user_open_ids: Vec::new(),
        discovered_chat_ids: Vec::new(),
        require_mention: true,
        updated_at: format_local_datetime(),
    })
}

pub fn load_user_settings(app: &AppHandle) -> Result<UserSettings, String> {
    let connection = open_database(app)?;
    let payload_json = connection
        .query_row(
            "SELECT payload_json FROM user_settings WHERE key = ?1",
            params![USER_SETTINGS_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok();

    match payload_json {
        // 旧版单 provider JSON 在读取时自动迁移成 provider 集合，避免用户手动重新配置。
        Some(payload_json) => model_provider::parse_or_migrate_user_settings_json(
            &payload_json,
            &format_local_datetime(),
        ),
        None => Ok(default_user_settings()),
    }
}

/** 保存用户模型和隐私设置；密钥本身由单独命令写入系统安全存储。
 *
 * 保存前强制按 providerId 重新计算每个 provider 的 key_reference，
 * 避免前端携带的 key_reference（例如新增 provider 时的占位值）和
 * `save_model_api_key` 实际写入 keyring 的引用不一致，导致密钥“已保存却找不到”。
 * 返回归一化后的设置，供调用方直接回传给前端，避免前端状态和持久化状态出现分歧。 */
pub fn save_user_settings(
    app: &AppHandle,
    settings: &UserSettings,
) -> Result<UserSettings, String> {
    let mut normalized_settings = settings.clone();
    if let Ok(persisted_settings) = load_user_settings(app) {
        // 短时 Skill 信任授权只允许后端执行器写入，普通设置 payload 不能伪造或延长授权。
        normalized_settings.agent_security.trusted_skill_grants =
            persisted_settings.agent_security.trusted_skill_grants;
    }

    model_provider::normalize_model_config(&mut normalized_settings.model_config);
    normalize_agent_security_settings(&mut normalized_settings.agent_security);

    persist_user_settings(app, &normalized_settings)?;

    Ok(normalized_settings)
}

/** 后端在用户实际批准完全模式执行后写入短时 Skill hash 授权。 */
pub(crate) fn save_trusted_skill_grant(
    app: &AppHandle,
    grant: crate::domain::TrustedSkillGrant,
) -> Result<(), String> {
    let mut settings = load_user_settings(app)?;
    settings
        .agent_security
        .trusted_skill_grants
        .retain(|existing| existing.skill_id != grant.skill_id);
    settings.agent_security.trusted_skill_grants.push(grant);
    normalize_agent_security_settings(&mut settings.agent_security);
    persist_user_settings(app, &settings)
}

/** 写入已经过调用方归一化的设置，避免内部授权写入再次走前端字段保护。 */
pub(crate) fn persist_user_settings(
    app: &AppHandle,
    settings: &UserSettings,
) -> Result<(), String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;

    let payload_json =
        serde_json::to_string(settings).map_err(|error| format!("无法序列化用户设置：{error}"))?;

    connection
        .execute(
            "INSERT OR REPLACE INTO user_settings (key, payload_json, updated_at) VALUES (?1, ?2, ?3)",
            params![USER_SETTINGS_KEY, payload_json, "刚刚"],
        )
        .map_err(|error| format!("无法保存用户设置：{error}"))?;

    Ok(())
}

/** 归一化 Agent 权限设置，阻止前端草稿绕过开关或提交不合理资源上限。 */
pub(crate) fn normalize_agent_security_settings(
    settings: &mut crate::domain::AgentSecuritySettings,
) {
    if !settings.advanced_execution_enabled {
        settings.default_level = "basic".to_owned();
        settings.autonomous_mode_enabled = false;
    } else if settings.default_level == "autonomous" && !settings.autonomous_mode_enabled {
        settings.default_level = "advanced".to_owned();
    } else if !matches!(
        settings.default_level.as_str(),
        "basic" | "advanced" | "autonomous"
    ) {
        settings.default_level = "basic".to_owned();
    }

    settings.resource_limits.timeout_seconds =
        settings.resource_limits.timeout_seconds.clamp(5, 1800);
    settings.resource_limits.max_memory_mb = settings.resource_limits.max_memory_mb.clamp(64, 4096);
    settings.resource_limits.max_processes = settings.resource_limits.max_processes.clamp(1, 64);
    settings.resource_limits.max_artifact_mb =
        settings.resource_limits.max_artifact_mb.clamp(1, 1024);
    settings.allowed_network_domains = settings
        .allowed_network_domains
        .iter()
        .map(|domain| domain.trim().to_lowercase())
        .filter(|domain| !domain.is_empty() && !domain.contains('/') && !domain.contains(':'))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
}

/** 跨会话记忆单条内容允许的最大字符数，避免用户写入过长偏好挤占上下文预算。 */
pub(crate) const MAX_KB_MEMORY_ENTRY_CHARS: usize = 800;

/** 单个知识库记忆集合允许的最大条目数，超过部分按 updatedAt 截断丢弃。 */
pub(crate) const MAX_KB_MEMORY_ENTRIES: usize = 32;

/** 旧版 IM 设置结构；只用于读取历史 `{ feishu: ... }` JSON 并迁移到 providers。 */
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LegacyImIntegrationSettings {
    feishu: FeishuIntegrationSettings,
}

/** 从 SQLite 读取即时通讯设置，缺失时返回禁用状态的安全默认值。 */
pub fn load_im_settings(app: &AppHandle) -> Result<ImIntegrationSettings, String> {
    let connection = open_database(app)?;
    let payload_json = connection
        .query_row(
            "SELECT payload_json FROM user_settings WHERE key = ?1",
            params![IM_SETTINGS_KEY],
            |row| row.get::<_, String>(0),
        )
        .ok();

    match payload_json {
        Some(payload_json) => {
            let mut settings = parse_im_settings_payload(&payload_json)?;

            normalize_im_settings(&mut settings);
            Ok(settings)
        }
        None => Ok(default_im_settings()),
    }
}

/** 解析 IM 设置 JSON；优先读取新 provider 结构，旧飞书结构走兼容迁移。 */
pub(crate) fn parse_im_settings_payload(
    payload_json: &str,
) -> Result<ImIntegrationSettings, String> {
    let value: Value = serde_json::from_str(payload_json)
        .map_err(|error| format!("无法解析即时通讯设置：{error}"))?;

    if value.get("providers").is_some() {
        return serde_json::from_value(value)
            .map_err(|error| format!("无法解析即时通讯 provider 设置：{error}"));
    }

    if value.get("feishu").is_some() {
        let legacy_settings: LegacyImIntegrationSettings = serde_json::from_value(value)
            .map_err(|error| format!("无法迁移旧版飞书设置：{error}"))?;

        return Ok(ImIntegrationSettings {
            providers: vec![ImProviderSettings::from_feishu(legacy_settings.feishu)],
        });
    }

    Ok(default_im_settings())
}

/** 保存即时通讯设置；appSecret 不在这个 payload 中，保存前统一归一化白名单和 key 引用。 */
pub fn save_im_settings(
    app: &AppHandle,
    settings: &ImIntegrationSettings,
) -> Result<ImIntegrationSettings, String> {
    let connection = open_database(app)?;
    let _write_guard = lock_database_writer()?;
    let mut normalized_settings = settings.clone();

    normalize_im_settings(&mut normalized_settings);

    let payload_json = serde_json::to_string(&normalized_settings)
        .map_err(|error| format!("无法序列化即时通讯设置：{error}"))?;

    connection
        .execute(
            "INSERT OR REPLACE INTO user_settings (key, payload_json, updated_at) VALUES (?1, ?2, ?3)",
            params![IM_SETTINGS_KEY, payload_json, format_local_datetime()],
        )
        .map_err(|error| format!("无法保存即时通讯设置：{error}"))?;

    Ok(normalized_settings)
}

/** 读取飞书 provider 的扁平运行时设置；不存在时返回可操作错误。 */
pub fn load_feishu_integration_settings(
    app: &AppHandle,
) -> Result<FeishuIntegrationSettings, String> {
    let settings = load_im_settings(app)?;

    settings
        .providers
        .iter()
        .find_map(ImProviderSettings::to_feishu_settings)
        .ok_or_else(|| "未找到飞书 IM provider 配置。".to_owned())
}

/** 记录飞书消息中发现的用户和群，供设置页一键加入白名单；不写日志、不暴露原始 ID。 */
pub fn remember_feishu_discovered_peer(
    app: &AppHandle,
    sender_open_id: &str,
    chat_id: &str,
    is_group_chat: bool,
) -> Result<bool, String> {
    let mut settings = load_im_settings(app)?;
    let sender_open_id = sender_open_id.trim();
    let chat_id = chat_id.trim();
    let mut changed = false;
    let provider = feishu_provider_mut(&mut settings)?;

    if !sender_open_id.is_empty()
        && !provider
            .allowed_user_open_ids
            .iter()
            .any(|id| id == sender_open_id)
        && !provider
            .discovered_user_open_ids
            .iter()
            .any(|id| id == sender_open_id)
    {
        provider
            .discovered_user_open_ids
            .push(sender_open_id.to_owned());
        changed = true;
    }

    if is_group_chat
        && !chat_id.is_empty()
        && !provider.allowed_chat_ids.iter().any(|id| id == chat_id)
        && !provider.discovered_chat_ids.iter().any(|id| id == chat_id)
    {
        provider.discovered_chat_ids.push(chat_id.to_owned());
        changed = true;
    }

    if changed {
        save_im_settings(app, &settings).map(|_| true)
    } else {
        Ok(false)
    }
}

/** 获取可变飞书 provider；调用方只在飞书平台事件中使用。 */
pub(crate) fn feishu_provider_mut(
    settings: &mut ImIntegrationSettings,
) -> Result<&mut ImProviderSettings, String> {
    settings
        .providers
        .iter_mut()
        .find(|provider| provider.provider_id == IM_PROVIDER_FEISHU)
        .ok_or_else(|| "未找到飞书 IM provider 配置。".to_owned())
}

/** 归一化 IM provider 设置，避免空白 ID、重复 provider、重复白名单或错误 key 引用进入持久化配置。 */
pub(crate) fn normalize_im_settings(settings: &mut ImIntegrationSettings) {
    if !settings
        .providers
        .iter()
        .any(|provider| provider.provider_id == IM_PROVIDER_FEISHU)
    {
        settings.providers.push(default_feishu_provider_settings());
    }

    for provider in &mut settings.providers {
        provider.provider_id = provider.provider_id.trim().to_ascii_lowercase();
        provider.default_knowledge_base_ids =
            normalize_identifier_list(&provider.default_knowledge_base_ids);
        provider.allowed_user_open_ids = normalize_identifier_list(&provider.allowed_user_open_ids);
        provider.allowed_chat_ids = normalize_identifier_list(&provider.allowed_chat_ids);
        provider.discovered_user_open_ids =
            normalize_identifier_list(&provider.discovered_user_open_ids)
                .into_iter()
                .filter(|id| !provider.allowed_user_open_ids.contains(id))
                .collect();
        provider.discovered_chat_ids = normalize_identifier_list(&provider.discovered_chat_ids)
            .into_iter()
            .filter(|id| !provider.allowed_chat_ids.contains(id))
            .collect();

        if provider.updated_at.trim().is_empty() || provider.updated_at == "刚刚" {
            provider.updated_at = format_local_datetime();
        }

        match &mut provider.config {
            ImProviderConfig::Feishu(config) => {
                provider.provider_id = IM_PROVIDER_FEISHU.to_owned();
                config.domain = match config.domain.trim().to_ascii_lowercase().as_str() {
                    "lark" => "lark".to_owned(),
                    _ => "feishu".to_owned(),
                };
                config.app_id = config.app_id.trim().to_owned();
                config.secret_key_reference = FEISHU_SECRET_KEY_REFERENCE.to_owned();
            }
        }
    }

    merge_duplicate_im_providers(&mut settings.providers);
}

/** 合并重复 IM provider；保留已启用、已填写字段和白名单，避免运行态误读到默认禁用副本。 */
pub(crate) fn merge_duplicate_im_providers(providers: &mut Vec<ImProviderSettings>) {
    let mut merged: Vec<ImProviderSettings> = Vec::new();

    for provider in providers.drain(..) {
        if let Some(existing) = merged
            .iter_mut()
            .find(|candidate| candidate.provider_id == provider.provider_id)
        {
            merge_im_provider_settings(existing, provider);
        } else {
            merged.push(provider);
        }
    }

    *providers = merged;
}

/** 把同一 providerId 的两个配置合并为一个，布尔开关和列表采用“更完整”的值。 */
pub(crate) fn merge_im_provider_settings(
    target: &mut ImProviderSettings,
    source: ImProviderSettings,
) {
    target.enabled = target.enabled || source.enabled;
    target.require_mention = target.require_mention && source.require_mention;
    target.default_knowledge_base_ids = merge_identifier_lists(
        &target.default_knowledge_base_ids,
        source.default_knowledge_base_ids,
    );
    target.allowed_user_open_ids =
        merge_identifier_lists(&target.allowed_user_open_ids, source.allowed_user_open_ids);
    target.allowed_chat_ids =
        merge_identifier_lists(&target.allowed_chat_ids, source.allowed_chat_ids);
    target.discovered_user_open_ids = merge_identifier_lists(
        &target.discovered_user_open_ids,
        source.discovered_user_open_ids,
    )
    .into_iter()
    .filter(|id| !target.allowed_user_open_ids.contains(id))
    .collect();
    target.discovered_chat_ids =
        merge_identifier_lists(&target.discovered_chat_ids, source.discovered_chat_ids)
            .into_iter()
            .filter(|id| !target.allowed_chat_ids.contains(id))
            .collect();

    if !source.updated_at.trim().is_empty() && source.updated_at != "刚刚" {
        target.updated_at = source.updated_at;
    }

    match (&mut target.config, source.config) {
        (ImProviderConfig::Feishu(target_config), ImProviderConfig::Feishu(source_config)) => {
            let source_has_runtime_config = !source_config.app_id.trim().is_empty();
            let target_missing_runtime_config = target_config.app_id.trim().is_empty();

            // 飞书/Lark 只保留非空 appId 和规范 key 引用；默认禁用副本不能覆盖用户已填写的平台。
            if source_has_runtime_config || target_missing_runtime_config {
                target_config.domain = source_config.domain;
            }
            if source_has_runtime_config {
                target_config.app_id = source_config.app_id;
            }
            target_config.secret_key_reference = FEISHU_SECRET_KEY_REFERENCE.to_owned();
        }
    }
}

/** 合并两个 ID 列表并统一 trim、去空、去重，不把原始 ID 写入日志。 */
pub(crate) fn merge_identifier_lists(current: &[String], next: Vec<String>) -> Vec<String> {
    let mut values = current.to_vec();

    values.extend(next);
    normalize_identifier_list(&values)
}

/** 对配置中的 ID 列表做 trim、去空和去重；不记录原始值，避免日志间接暴露用户/群 ID。 */
pub(crate) fn normalize_identifier_list(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for value in values {
        let trimmed = value.trim();

        if trimmed.is_empty() || !seen.insert(trimmed.to_owned()) {
            continue;
        }

        normalized.push(trimmed.to_owned());
    }

    normalized
}

/**
 * 根据构建目标选择 Keychain service；仅 macOS debug 构建隔离到开发命名空间，
 * 使稳定的本机开发签名永远不会读取正式应用保存的凭据。
 */
pub(crate) fn keyring_service() -> &'static str {
    let service = keyring_service_for_build(cfg!(target_os = "macos"), cfg!(debug_assertions));

    // 只记录 service 名和构建模式；两者均不包含用户密钥或账户标识。
    KEYRING_SERVICE_OBSERVABILITY.get_or_init(|| {
        log::info!(
            target: "orange::keyring",
            "keyring_service_selected service={} development_namespace={}",
            service,
            uses_development_keyring_namespace_without_logging(service),
        );
    });

    service
}

/** 供运行时和测试共用的 service 选择规则，避免 debug/release 行为发生漂移。 */
pub(crate) fn keyring_service_for_build(is_macos: bool, is_debug_build: bool) -> &'static str {
    if is_macos && is_debug_build {
        DEVELOPMENT_KEYRING_SERVICE
    } else {
        PRODUCTION_KEYRING_SERVICE
    }
}

/** 无日志辅助函数，避免 service 选择日志初始化时发生递归调用。 */
pub(crate) fn uses_development_keyring_namespace_without_logging(service: &str) -> bool {
    service == DEVELOPMENT_KEYRING_SERVICE
}

/**
 * 缓存未命中时的 Keychain 查询结果；`requires_migration` 表示命中的是历史命名空间。
 *
 * 该结构只在内存中短暂保存密钥，以便调用方完成迁移与缓存，不会进入日志或 SQLite。
 */
pub(crate) struct KeyringPasswordLookup {
    pub(crate) api_key: Option<String>,
    pub(crate) requires_migration: bool,
}

/**
 * 在当前 Keychain service 中优先查询密钥，并仅允许正式构建回退到历史命名空间。
 *
 * 开发构建保存和读取都使用 `Orange Dev`。因此即使要隔离生产凭据，也必须在禁止
 * 回退之前读取 `Orange Dev`；否则密钥只在进程缓存存活，重启后会被错误判定为丢失。
 */
pub(crate) fn load_keyring_password_after_cache_miss_with<F>(
    current_service: &str,
    key_reference: &str,
    mut read_password: F,
) -> Result<KeyringPasswordLookup, String>
where
    F: FnMut(&str, &str) -> Result<Option<String>, String>,
{
    let normalized_key_reference = normalize_key_reference(key_reference);

    // 所有构建都先读取自身命名空间；这是重启后恢复开发态密钥的关键路径。
    if let Some(api_key) = read_password(current_service, &normalized_key_reference)? {
        return Ok(KeyringPasswordLookup {
            api_key: Some(api_key),
            requires_migration: false,
        });
    }

    // 调试签名只能读取 Orange Dev，不能回退访问正式或品牌升级前的凭据。
    if uses_development_keyring_namespace_without_logging(current_service) {
        return Ok(KeyringPasswordLookup {
            api_key: None,
            requires_migration: false,
        });
    }

    let legacy_key_reference = key_reference
        .strip_prefix("orange-")
        .map(|rest| format!("cici-note-{rest}"))
        .unwrap_or_else(|| key_reference.to_owned());

    // 正式构建兼容品牌升级前的 service 和 account；命中后由调用方回写到当前 service。
    for (service, account) in [
        ("Cici Note", key_reference),
        ("Cici Note", legacy_key_reference.as_str()),
        (PRODUCTION_KEYRING_SERVICE, legacy_key_reference.as_str()),
    ] {
        if let Some(api_key) = read_password(service, account)? {
            return Ok(KeyringPasswordLookup {
                api_key: Some(api_key),
                requires_migration: true,
            });
        }
    }

    Ok(KeyringPasswordLookup {
        api_key: None,
        requires_migration: false,
    })
}

/** 把 BYOK 模型密钥保存到系统安全存储，按 providerId 隔离 key 引用，避免明文进入 SQLite。 */
pub fn save_model_api_key(provider_id: &str, api_key: &str) -> Result<ModelApiKeyStatus, String> {
    ensure_persistent_model_keyring()?;

    let key_reference = model_provider::key_reference_for_provider(provider_id);
    let entry = keyring::Entry::new(keyring_service(), &key_reference)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;

    entry
        .set_password(api_key)
        .map_err(|error| format!("无法保存模型密钥：{error}"))?;

    let saved_api_key = entry
        .get_password()
        .map_err(|error| format!("模型密钥已提交但读回校验失败：{error}"))?;

    // 读回校验只比较是否为空，避免在错误信息或日志中暴露完整密钥内容。
    if saved_api_key.trim().is_empty() {
        return Err("模型密钥已提交但系统安全存储返回空值。".to_owned());
    }

    store_model_api_key_in_cache(&key_reference, &saved_api_key)?;

    Ok(ModelApiKeyStatus {
        provider_id: provider_id.to_owned(),
        key_reference,
        configured: true,
        message: "模型密钥已保存、读回校验通过，并已载入当前桌面进程。".to_owned(),
    })
}

/** 从系统安全存储按 key 引用读取 BYOK 模型密钥；缺失时返回 None。 */
pub fn load_model_api_key(key_reference: &str) -> Result<Option<String>, String> {
    ensure_persistent_model_keyring()?;

    if let Some(api_key) = load_model_api_key_from_cache(key_reference)? {
        return Ok(Some(api_key));
    }

    let lookup = load_keyring_password_after_cache_miss_with(
        keyring_service(),
        key_reference,
        read_keyring_password,
    )?;
    let Some(api_key) = lookup.api_key else {
        return Ok(None);
    };

    let normalized_key_reference = normalize_key_reference(key_reference);
    if lookup.requires_migration {
        // 命中旧条目：迁移写入规范位置，并缓存。
        persist_migrated_keyring_password(&normalized_key_reference, &api_key)?;
    }

    store_model_api_key_in_cache(&normalized_key_reference, &api_key)?;
    Ok(Some(api_key))
}

/** 把传入的 key_reference 规范化为新前缀 orange-，兼容 sqlite 里仍存的 cici-note-* 历史 account。 */
pub(crate) fn normalize_key_reference(key_reference: &str) -> String {
    key_reference
        .strip_prefix("cici-note-")
        .map(|rest| format!("orange-{rest}"))
        .unwrap_or_else(|| key_reference.to_owned())
}

/** 读取指定 service/account 的 keyring 密码；缺失视为 None，其他错误向上抛。 */
pub(crate) fn read_keyring_password(
    service: &str,
    account: &str,
) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(service, account)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;
    match entry.get_password() {
        Ok(api_key) if !api_key.trim().is_empty() => Ok(Some(api_key)),
        Ok(_) => Ok(None),
        Err(error) => {
            let message = error.to_string();
            if is_missing_keyring_entry_error(&message) {
                Ok(None)
            } else {
                Err(format!("无法读取模型密钥：{error}"))
            }
        }
    }
}

/** 把迁移过来的明文密钥写入规范的 keyring service 与 account，错误信息不回显明文内容。 */
pub(crate) fn persist_migrated_keyring_password(
    normalized_key_reference: &str,
    api_key: &str,
) -> Result<(), String> {
    let entry = keyring::Entry::new(keyring_service(), normalized_key_reference)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;
    entry
        .set_password(api_key)
        .map_err(|error| format!("品牌升级迁移写入密钥失败：{error}"))
}

/** 把 IM provider 密钥保存到系统安全存储；错误信息不回显 secret 内容。 */
pub fn save_im_provider_secret(
    provider_id: &str,
    secret: &str,
) -> Result<ImProviderCredentialStatus, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => save_feishu_app_secret(secret),
        _ => Err(format!("暂不支持保存 IM provider {provider_id} 的密钥。")),
    }
}

/** 查询 IM provider 密钥状态；设置页只展示状态，不拿到明文。 */
pub fn load_im_provider_credential_status(
    provider_id: &str,
) -> Result<ImProviderCredentialStatus, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => load_feishu_credential_status(),
        _ => Err(format!(
            "暂不支持读取 IM provider {provider_id} 的凭证状态。"
        )),
    }
}

/** 读取 IM provider 明文密钥；仅供网关和发送 API 在后台流程中使用。 */
pub fn load_im_provider_secret(provider_id: &str) -> Result<Option<String>, String> {
    match provider_id {
        IM_PROVIDER_FEISHU => load_feishu_app_secret(),
        _ => Err(format!("暂不支持读取 IM provider {provider_id} 的密钥。")),
    }
}

/** 把飞书 appSecret 保存到系统安全存储；错误信息不回显 secret 内容。 */
pub fn save_feishu_app_secret(app_secret: &str) -> Result<FeishuCredentialStatus, String> {
    ensure_persistent_model_keyring()?;

    if app_secret.trim().is_empty() {
        return Err("飞书 appSecret 不能为空。".to_owned());
    }

    let entry = keyring::Entry::new(keyring_service(), FEISHU_SECRET_KEY_REFERENCE)
        .map_err(|error| format!("无法打开系统安全存储：{error}"))?;

    entry
        .set_password(app_secret)
        .map_err(|error| format!("无法保存飞书 appSecret：{error}"))?;

    let saved_secret = entry
        .get_password()
        .map_err(|error| format!("飞书 appSecret 已提交但读回校验失败：{error}"))?;

    if saved_secret.trim().is_empty() {
        return Err("飞书 appSecret 已提交但系统安全存储返回空值。".to_owned());
    }

    store_model_api_key_in_cache(FEISHU_SECRET_KEY_REFERENCE, &saved_secret)?;

    Ok(FeishuCredentialStatus {
        provider_id: IM_PROVIDER_FEISHU.to_owned(),
        key_reference: FEISHU_SECRET_KEY_REFERENCE.to_owned(),
        configured: true,
        message: "飞书 appSecret 已保存到系统安全存储。".to_owned(),
    })
}

/** 读取飞书 appSecret，供长连接和发送消息 API 使用；缺失时返回 None。 */
pub fn load_feishu_app_secret() -> Result<Option<String>, String> {
    load_model_api_key(FEISHU_SECRET_KEY_REFERENCE)
}

/** 查询飞书 appSecret 是否可读取；设置页只展示状态，不拿到明文。 */
pub fn load_feishu_credential_status() -> Result<FeishuCredentialStatus, String> {
    let configured = load_feishu_app_secret()?.is_some();
    let message = if configured {
        "系统安全存储中已找到飞书 appSecret。"
    } else {
        "系统安全存储中尚未找到飞书 appSecret。"
    };

    Ok(FeishuCredentialStatus {
        provider_id: IM_PROVIDER_FEISHU.to_owned(),
        key_reference: FEISHU_SECRET_KEY_REFERENCE.to_owned(),
        configured,
        message: message.to_owned(),
    })
}

/** 查询单个 provider 的模型密钥是否已经可读取；不会返回明文密钥。 */
pub fn load_model_api_key_status(
    provider_id: &str,
    key_reference: &str,
) -> Result<ModelApiKeyStatus, String> {
    let configured = load_model_api_key(key_reference)?.is_some();
    let message = if configured {
        "系统安全存储中已找到模型密钥。"
    } else {
        "系统安全存储中尚未找到模型密钥。"
    };

    Ok(ModelApiKeyStatus {
        provider_id: provider_id.to_owned(),
        key_reference: key_reference.to_owned(),
        configured,
        message: message.to_owned(),
    })
}

/** 批量查询设置中每个 provider 的密钥状态，供设置页一次性展示。 */
pub fn load_model_api_key_statuses(
    providers: &[LlmProviderConfig],
) -> Result<Vec<ModelApiKeyStatus>, String> {
    providers
        .iter()
        .map(|provider| load_model_api_key_status(&provider.id, &provider.key_reference))
        .collect()
}

/**
 * 按用户请求读取单个 provider 的明文模型密钥。
 *
 * 必须先在当前设置里找到该 provider，再用 providerId 派生 keyring 引用；
 * 不接受前端传入的 keyReference，避免按任意 account 探测系统安全存储。
 * 派生规则与 `save_model_api_key` 一致，防止设置里残留的占位引用读不到已保存密钥。
 */
pub fn reveal_model_api_key(
    providers: &[LlmProviderConfig],
    provider_id: &str,
) -> Result<RevealedModelApiKey, String> {
    let trimmed_provider_id = provider_id.trim();
    if trimmed_provider_id.is_empty() {
        return Err("模型 Provider ID 不能为空。".to_owned());
    }

    let provider = providers
        .iter()
        .find(|candidate| candidate.id == trimmed_provider_id)
        .ok_or_else(|| "找不到指定的模型 Provider。".to_owned())?;
    let key_reference = model_provider::key_reference_for_provider(&provider.id);
    let api_key = load_model_api_key(&key_reference)?
        .ok_or_else(|| "系统安全存储中尚未找到模型密钥。".to_owned())?;

    Ok(RevealedModelApiKey {
        provider_id: provider.id.clone(),
        api_key,
    })
}

/** 确认当前 keyring 构建使用可跨进程持久化的系统安全存储。 */
pub(crate) fn ensure_persistent_model_keyring() -> Result<(), String> {
    if model_keyring_persists_until_delete() {
        return Ok(());
    }

    Err("当前构建未启用系统安全存储，模型密钥无法跨重启保存。请为 keyring 启用平台后端 feature 后重新构建。".to_owned())
}

/** 判断默认 keyring 后端是否会把密钥保存到磁盘级安全存储。 */
pub(crate) fn model_keyring_persists_until_delete() -> bool {
    matches!(
        keyring::default::default_credential_builder().persistence(),
        keyring::credential::CredentialPersistence::UntilDelete
    )
}

/** 把已验证密钥按 key 引用放入进程内缓存，避免同一桌面会话内反复访问 keychain。 */
pub(crate) fn store_model_api_key_in_cache(
    key_reference: &str,
    api_key: &str,
) -> Result<(), String> {
    let cache = MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cached_api_keys = cache
        .lock()
        .map_err(|_| "模型密钥缓存已损坏。".to_owned())?;

    // 缓存只优化当前进程的重复读取，真实持久化仍完全依赖系统安全存储。
    cached_api_keys.insert(key_reference.to_owned(), api_key.to_owned());

    Ok(())
}

/** 按 key 引用从进程内缓存读取模型密钥；不命中时再访问系统安全存储。 */
pub(crate) fn load_model_api_key_from_cache(key_reference: &str) -> Result<Option<String>, String> {
    let cache = MODEL_API_KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached_api_keys = cache
        .lock()
        .map_err(|_| "模型密钥缓存已损坏。".to_owned())?;

    Ok(cached_api_keys.get(key_reference).cloned())
}

/** 识别不同系统 keyring 后端返回的“条目不存在”错误文案。 */
pub(crate) fn is_missing_keyring_entry_error(message: &str) -> bool {
    let normalized_message = message.to_lowercase();

    normalized_message.contains("no entry found")
        || normalized_message.contains("no matching entry")
        || normalized_message.contains("not found")
        || normalized_message.contains("could not be found")
}
