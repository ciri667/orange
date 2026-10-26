//! Skill 只是说明及资源；读取脚本不会授予执行权。
use crate::{domain::AgentSkill, agent_tools::{AgentToolContext, ToolExecutionResult}};
use serde_json::{json, Value};
use std::{fs, path::{Component, Path}};

/** 显式选择与主动 read 共用解析；不为内置 Skill 伪造本机路径。 */
pub fn read_resource(skills: &[AgentSkill], id: &str, relative: Option<&str>) -> Result<Value, String> {
    let skill = skills.iter().find(|s| s.id == id && s.enabled).ok_or("Skill 不存在或已禁用")?;
    let relative = relative.unwrap_or("SKILL.md");
    let path = Path::new(relative);
    if path.is_absolute() || path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("Skill 资源必须是包内相对路径，禁止路径逃逸。".into());
    }
    let content = if skill.source == super::BUILT_IN_SKILL_SOURCE {
        if relative != "SKILL.md" { return Err("该内置 Skill 没有此随附资源。".into()); }
        skill.instructions.clone()
    } else {
        let main = Path::new(skill.path.as_deref().ok_or("Skill 缺少主文件")?);
        let root = main.parent().ok_or("Skill 缺少包目录")?;
        crate::file_changes::reject_links(root)?;
        let target = root.join(path); crate::file_changes::reject_links(&target)?;
        let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        let canonical = fs::canonicalize(&target).map_err(|e| e.to_string())?;
        if !canonical.starts_with(&canonical_root) { return Err("Skill 链接指向包外，已拒绝。".into()); }
        let metadata=fs::metadata(&canonical).map_err(|e|e.to_string())?;
        if !metadata.is_file() || metadata.len() > 1024*1024 { return Err("Skill 资源必须是小于 1 MiB 的文本文件。".into()); }
        let raw = fs::read_to_string(canonical).map_err(|e|e.to_string())?;
        if relative == "SKILL.md" { super::catalog::parse_skill_markdown(&raw)?.instructions } else { raw }
    };
    Ok(json!({"target":"skill", "skillId":skill.id, "source":skill.source, "resource":relative,
        "contentHash":crate::storage::hash_content(&content), "content":content,
        "notice":"资源只读；脚本执行必须使用 exec 并重新通过当前会话权限与操作审批。"}))
}

pub fn execute(context: &mut AgentToolContext<'_>, args: &Value) -> ToolExecutionResult {
    let result = context.app.ok_or("当前环境无法读取 Skill 目录".to_owned()).and_then(|app| {
        let connection=crate::storage::open_database(app)?;
        let skills=super::load_agent_skills(app,&connection)?;
        read_resource(&skills,args["skillId"].as_str().ok_or("缺少 skillId")?,args["path"].as_str())
    });
    match result {
        Ok(payload) => ToolExecutionResult {success:true,summary:"已加载 Skill 只读资源。".into(),payload,citations:vec![],audit_fragment:None},
        Err(e) => ToolExecutionResult::failed(&e),
    }
}

/** 回合内加载缓存只用于上下文去重。每次模型请求前重新验证启停与正文版本。 */
#[derive(Default)]
pub struct LoadedResources(std::collections::BTreeMap<(String,String),Value>);
impl LoadedResources {
    pub fn remember(&mut self, payload: &Value) -> bool {
        let Some(id)=payload["skillId"].as_str() else {return false};
        let Some(resource)=payload["resource"].as_str() else {return false};
        let key=(id.to_owned(),resource.to_owned());
        let duplicate=self.0.get(&key).is_some_and(|old|old["contentHash"]==payload["contentHash"]);
        self.0.insert(key,payload.clone()); duplicate
    }
    pub fn restore(&mut self, app: &tauri::AppHandle, messages: &mut [Value]) -> Result<(),String> {
        let connection=crate::storage::open_database(app)?;
        let skills=super::load_agent_skills(app,&connection)?;
        self.0.retain(|(id,path),old| {
            let valid=read_resource(&skills,id,Some(path)).is_ok_and(|new|new["contentHash"]==old["contentHash"]);
            if !valid {
                let stale=old["content"].as_str().unwrap_or("");
                for message in messages.iter_mut() {
                    if let Some(content)=message["content"].as_str() {
                        let replacement=if let Ok(mut payload)=serde_json::from_str::<Value>(content) {
                            if payload["skillId"]==*id { payload["content"]=json!("资源已禁用或更新，请重新加载。"); payload.to_string() } else { content.to_owned() }
                        } else if !stale.is_empty() { content.replace(stale,"[Skill 已禁用或更新，请重新加载]") } else {content.to_owned()};
                        message["content"]=json!(replacement);
                    }
                }
            }
            valid
        });
        if let Some(system)=messages.first_mut() {
            let original=system["content"].as_str().unwrap_or("").split("\n【本回合已加载 Skill 资源】").next().unwrap_or("").to_owned();
            system["content"]=json!(if self.0.is_empty() {original} else {format!("{original}\n【本回合已加载 Skill 资源】\n{}",self.0.values().cloned().collect::<Vec<_>>().iter().map(Value::to_string).collect::<Vec<_>>().join("\n"))});
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabled_builtin_resources_are_readonly_without_fabricated_paths() {
        let mut skills=super::super::built_in_skills();
        let id=skills[0].id.clone();
        let resource=read_resource(&skills,&id,None).unwrap();
        assert_eq!(resource["source"],"built-in");
        assert!(resource["content"].as_str().is_some_and(|s|!s.is_empty()));
        assert!(read_resource(&skills,&id,Some("../outside")).is_err());
        skills[0].enabled=false; assert!(read_resource(&skills,&id,None).is_err());
    }
    #[test]
    fn custom_resources_support_scripts_without_execution_manifest_and_reject_escape() {
        let dir=tempfile::tempdir().unwrap(); fs::create_dir(dir.path().join("scripts")).unwrap();
        fs::write(dir.path().join("SKILL.md"),"---\nname: demo\ndescription: test\n---\n任务指令").unwrap();
        fs::write(dir.path().join("scripts/demo.ps1"),"Write-Output demo").unwrap();
        let mut skill=super::super::built_in_skills().remove(0); skill.source="custom".into();
        skill.path=Some(dir.path().join("SKILL.md").to_string_lossy().into_owned());
        let id=skill.id.clone(); let skills=vec![skill];
        assert_eq!(read_resource(&skills,&id,Some("scripts/demo.ps1")).unwrap()["content"],"Write-Output demo");
        assert!(read_resource(&skills,&id,Some("../outside.txt")).is_err());
        assert!(read_resource(&skills,&id,Some(&dir.path().join("SKILL.md").to_string_lossy())).is_err());
        let mut loaded=LoadedResources::default(); let resource=read_resource(&skills,&id,None).unwrap();
        assert!(!loaded.remember(&resource)); assert!(loaded.remember(&resource));
        fs::write(dir.path().join("SKILL.md"),"---\nname: demo\ndescription: test\n---\n更新指令").unwrap();
        assert!(!loaded.remember(&read_resource(&skills,&id,None).unwrap()));
    }
}
