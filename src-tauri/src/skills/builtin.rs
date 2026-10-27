//! 随应用发布的 Skill 文件同步；模型和设置页始终读取 ~/.orange/skills 中的副本。
use std::{collections::BTreeSet, fs, io::Write, path::{Component, Path, PathBuf}, sync::Mutex};
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

pub(super) const BUILTIN_DIRECTORY: &str = "builtin";
const MANIFEST_FILE: &str = ".bundled-files.json";
/** 多个会话可能同时加载目录，串行同步防止互相替换临时文件或更新清单。 */
static SYNC_LOCK: Mutex<()> = Mutex::new(());

/** 源文件同时供 Vite 浏览器开发态和 Tauri 打包使用，正文只有这一份。 */
#[cfg(any(debug_assertions, test))]
pub(super) fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../src/shared/skills/builtin")
}

/** 发布包优先使用 resource_dir；只有开发构建允许回退仓库文件。 */
pub(super) fn sync(app: &AppHandle, skills_root: &Path) -> Result<(), String> {
    let packaged = app.path().resource_dir()
        .map_err(|error| format!("无法获取 Skill 资源目录：{error}"))?
        .join("skills").join(BUILTIN_DIRECTORY);
    let source = if packaged.is_dir() {
        packaged
    } else {
        #[cfg(debug_assertions)]
        { source_root() }
        #[cfg(not(debug_assertions))]
        { return Err(format!("找不到内置 Skill 发布资源：{}", packaged.display())); }
    };
    sync_from(&source, skills_root)
}

/** 同步受管理的内置文件；相同内容不重写，已从发布包移除的文件按清单清理。 */
pub(super) fn sync_from(source: &Path, skills_root: &Path) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().map_err(|_| "内置 Skill 同步锁不可用")?;
    let target_root = skills_root.join(BUILTIN_DIRECTORY);
    crate::file_changes::reject_links(&target_root)?;
    crate::file_changes::reject_links(source)?;
    if !source.is_dir() { return Err("内置 Skill 资源目录不存在。".into()); }
    fs::create_dir_all(&target_root).map_err(|error| error.to_string())?;

    let manifest = target_root.join(MANIFEST_FILE);
    crate::file_changes::reject_links(&manifest)?;
    let previous: BTreeSet<String> = fs::read(&manifest).ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let mut current = BTreeSet::new();
    // 完整遍历发布资源，不接受符号链接、目录联接点或设备文件。
    for entry in WalkDir::new(source).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(|error| format!("无法读取内置 Skill 资源：{error}"))?;
        crate::file_changes::reject_links(entry.path())?;
        let relative = entry.path().strip_prefix(source).map_err(|error| error.to_string())?;
        let target = target_root.join(relative);
        crate::file_changes::reject_links(&target)?;
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        } else if entry.file_type().is_file() {
            let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
            write_if_changed(&target, &bytes)?;
            current.insert(relative.to_string_lossy().replace('\\', "/"));
        } else {
            return Err("内置 Skill 资源只能包含普通文件和目录。".into());
        }
    }

    // 只移除上一版清单管理过的普通文件，不清理用户额外放入的资料。
    for removed in previous.difference(&current) {
        let relative = Path::new(removed);
        if relative.is_absolute() || relative.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err("内置 Skill 同步清单含有无效路径。".into());
        }
        let target = target_root.join(relative);
        crate::file_changes::reject_links(&target)?;
        if target.is_file() { fs::remove_file(target).map_err(|error| error.to_string())?; }
    }
    write_if_changed(&manifest, &serde_json::to_vec(&current).map_err(|error| error.to_string())?)
}

/** 同目录临时文件原子替换，避免并行读取观察到半份 SKILL.md。 */
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if fs::read(path).is_ok_and(|existing| existing == bytes) { return Ok(()); }
    let parent = path.parent().ok_or("无法解析 Skill 文件目录")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary.write_all(bytes).map_err(|error| error.to_string())?;
    temporary.persist(path).map_err(|error| format!("无法同步内置 Skill 文件 {}：{error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 内置同步只管理发布清单，不能删除用户放进 builtin 目录的附加文件。 */
    #[test]
    fn sync_from_updates_managed_files_and_preserves_user_files() {
        let source = tempfile::tempdir().expect("source");
        let target = tempfile::tempdir().expect("target");
        fs::create_dir_all(source.path().join("note-research")).expect("source skill");
        fs::write(source.path().join("note-research/SKILL.md"), "v1").expect("source file");
        sync_from(source.path(), target.path()).expect("first sync");

        let target_skill = target.path().join(BUILTIN_DIRECTORY).join("note-research/SKILL.md");
        assert_eq!(fs::read_to_string(&target_skill).unwrap(), "v1");
        fs::write(target.path().join(BUILTIN_DIRECTORY).join("user-note.txt"), "keep")
            .expect("user file");

        fs::write(source.path().join("note-research/SKILL.md"), "v2").expect("update source");
        fs::create_dir_all(source.path().join("draft-from-context")).expect("second skill");
        fs::write(source.path().join("draft-from-context/SKILL.md"), "v1").expect("second file");
        sync_from(source.path(), target.path()).expect("second sync");

        assert_eq!(fs::read_to_string(&target_skill).unwrap(), "v2");
        assert_eq!(fs::read_to_string(target.path().join(BUILTIN_DIRECTORY).join("user-note.txt")).unwrap(), "keep");
    }

    /** 发布包移除的文件会按上一版清单清理，避免旧内置 Skill 继续出现在目录中。 */
    #[test]
    fn sync_from_removes_files_removed_from_bundle() {
        let source = tempfile::tempdir().expect("source");
        let target = tempfile::tempdir().expect("target");
        fs::write(source.path().join("SKILL.md"), "v1").expect("source file");
        sync_from(source.path(), target.path()).expect("first sync");
        fs::remove_file(source.path().join("SKILL.md")).expect("remove source");
        sync_from(source.path(), target.path()).expect("second sync");
        assert!(!target.path().join(BUILTIN_DIRECTORY).join("SKILL.md").exists());
    }
}
