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

/** Agent Skill 是可启停的指令与资源包；脚本资源本身不获得执行权限。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub instructions: String,
    /** 包内只读资源标识，包括说明、参考、模板与脚本。 */
    #[serde(default)]
    pub resources: Vec<String>,
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
}

/** skills.sh 目录中的一条可发现 Skill，供设置页「发现」列表展示。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSkill {
    pub id: String,
    pub skill_id: String,
    pub name: String,
    pub source: String,
    pub installs: u64,
    pub page_url: String,
    pub installable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/** 在线 Skill 搜索结果；query 回显用户输入，便于核对防抖请求。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSkillSearchResult {
    pub query: String,
    pub skills: Vec<OnlineSkill>,
}

/** 在线 Skill 预览；简介缺失时前端仍可展示名称和来源。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineSkillPreview {
    pub id: String,
    pub page_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
