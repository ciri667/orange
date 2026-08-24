use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/** Skill 启用状态类型，前端用它派生列表筛选和状态标签。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillStatus {
    Enabled,
    Disabled,
}

/** Skill 来源类型；内置 skill 由应用提供，自定义 skill 来自用户 Skills 目录。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentSkillSource {
    BuiltIn,
    Custom,
}

/** Skill 安装来源类型，URL、本地目录和本地压缩包走不同的准备流程。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallSourceType {
    Url,
    LocalFolder,
    LocalArchive,
}

/** Skill 安装遇到同名目录时的处理策略。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallConflictStrategy {
    Fail,
    Replace,
}

/** Agent skill 是可启停的指令型工作流包，首版不携带脚本或外部命令。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub instructions: String,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_manifest: Option<SkillRuntimeManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<SkillCompatibilityReport>,
}

/** 可执行 Skill 的跨平台运行声明。 */
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRuntimeManifest {
    pub runtime: String,
    pub entry: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub network_domains: Vec<String>,
    #[serde(default)]
    pub credential_aliases: Vec<String>,
    #[serde(default)]
    pub artifact_patterns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRuntimeStatus {
    pub runtime: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCompatibilityReport {
    pub status: String,
    pub package_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<SkillRuntimeStatus>,
    #[serde(default)]
    pub warnings: Vec<String>,
}
