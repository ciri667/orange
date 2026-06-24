import { Edit3, FolderOpen, Plus, Save, Search, Trash2, X } from "lucide-react";
import type { FormEvent } from "react";
import { useMemo, useState } from "react";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import type { AgentSkill, AgentSkillSource } from "../shared/types";

/** Skills 列表来源筛选，all 用于展示完整合并结果。 */
type SkillSourceFilter = "all" | AgentSkillSource;

/** 用户目录中的文件式 skill 和旧版 user skill 都允许用户管理。 */
function isUserManagedSkill(skill: AgentSkill) {
  return skill.source === "file" || skill.source === "user";
}

/** Skill 表单草稿，标签和触发词在 UI 中用逗号分隔编辑。 */
interface SkillFormDraft {
  id: string;
  name: string;
  displayName: string;
  description: string;
  instructions: string;
  tagsText: string;
  triggersText: string;
  enabled: boolean;
  allowAutoInvoke: boolean;
}

/** 待确认的 Skill 操作；确认后才执行删除，避免依赖系统 confirm。 */
interface PendingSkillConfirmation extends ConfirmDialogConfig {
  onConfirm: () => Promise<void> | void;
}

/** Skills 管理弹窗，提供浏览、筛选、启停和用户自建 skill CRUD。 */
export function SkillsModal({
  skills,
  isBusy,
  onSaveSkill,
  onToggleSkill,
  onDeleteSkill,
  onOpenUserSkillsFolder,
  onClose,
}: {
  skills: AgentSkill[];
  isBusy: boolean;
  onSaveSkill: (skill: AgentSkill) => Promise<AgentSkill | void> | AgentSkill | void;
  onToggleSkill: (skillId: string, enabled: boolean, allowAutoInvoke?: boolean) => Promise<void> | void;
  onDeleteSkill: (skillId: string) => Promise<void> | void;
  onOpenUserSkillsFolder: () => Promise<void> | void;
  onClose: () => void;
}) {
  /** 搜索词同时匹配名称、说明、标签和触发词。 */
  const [searchTerm, setSearchTerm] = useState("");
  /** 来源筛选帮助用户区分内置、文件扫描和 UI 创建的 skill。 */
  const [sourceFilter, setSourceFilter] = useState<SkillSourceFilter>("all");
  /** 标签筛选使用单选，避免多标签组合导致列表空状态难理解。 */
  const [activeTag, setActiveTag] = useState("");
  /** 当前详情面板展示的 skill ID，新建时为空。 */
  const [selectedSkillId, setSelectedSkillId] = useState(skills[0]?.id ?? "");
  /** 表单草稿存在时详情面板切换为新建或编辑模式。 */
  const [formDraft, setFormDraft] = useState<SkillFormDraft | null>(null);
  /** 当前等待用户确认的危险操作，使用应用内弹窗承载。 */
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingSkillConfirmation | null>(null);

  /** 可用标签来自当前 skill 列表，便于用户快速按能力类别筛选。 */
  const availableTags = useMemo(
    () => Array.from(new Set(skills.flatMap((skill) => skill.tags))).sort((left, right) => left.localeCompare(right)),
    [skills],
  );
  /** 来源数量用于筛选按钮上的轻量提示，和后端合并顺序保持解耦。 */
  const sourceCounts = useMemo(
    () => ({
      all: skills.length,
      "built-in": skills.filter((skill) => skill.source === "built-in").length,
      file: skills.filter((skill) => skill.source === "file").length,
      user: skills.filter((skill) => skill.source === "user").length,
    }),
    [skills],
  );
  /** 根据搜索词、来源和标签得到展示列表，后端已保证内置、文件、用户的合并顺序。 */
  const filteredSkills = useMemo(
    () =>
      skills.filter((skill) => {
        const normalizedSearch = searchTerm.trim().toLowerCase();
        const searchableText = [
          skill.name,
          skill.displayName,
          skill.description,
          skill.instructions,
          skill.path ?? "",
          skill.relativePath ?? "",
          ...skill.tags,
          ...skill.triggers,
          ...Object.values(skill.metadata ?? {}),
        ]
          .join(" ")
          .toLowerCase();
        const matchesSearch = !normalizedSearch || searchableText.includes(normalizedSearch);
        const matchesSource = sourceFilter === "all" || skill.source === sourceFilter;
        const matchesTag = !activeTag || skill.tags.includes(activeTag);

        return matchesSearch && matchesSource && matchesTag;
      }),
    [activeTag, searchTerm, skills, sourceFilter],
  );
  /** 当前详情 skill；列表过滤后仍保留原选择，避免搜索时误清空表单。 */
  const selectedSkill = skills.find((skill) => skill.id === selectedSkillId) ?? filteredSkills[0] ?? skills[0];

  /** 打开新建用户 skill 表单，默认启用自动触发但不预填触发词。 */
  function handleCreateSkill() {
    setSelectedSkillId("");
    setFormDraft({
      id: "",
      name: "",
      displayName: "",
      description: "",
      instructions: "",
      tagsText: "",
      triggersText: "",
      enabled: true,
      allowAutoInvoke: true,
    });
  }

  /** 打开编辑用户可管理 skill 表单；内置 skill 不允许编辑说明内容。 */
  function handleEditSkill(skill: AgentSkill) {
    if (!isUserManagedSkill(skill)) {
      return;
    }

    setSelectedSkillId(skill.id);
    setFormDraft({
      id: skill.id,
      name: skill.name,
      displayName: skill.displayName,
      description: skill.description,
      instructions: skill.instructions,
      tagsText: skill.tags.join(", "),
      triggersText: skill.triggers.join(", "),
      enabled: skill.enabled,
      allowAutoInvoke: skill.allowAutoInvoke,
    });
  }

  /** 提交用户 skill 表单，并把逗号分隔文本归一化为数组。 */
  async function handleSubmitSkill(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    if (!formDraft) {
      return;
    }

    const now = new Date().toLocaleString();
    const existingSkill = selectedSkill?.id === formDraft.id ? selectedSkill : undefined;
    const skill: AgentSkill = {
      id: formDraft.id,
      name: formDraft.name,
      displayName: formDraft.displayName,
      description: formDraft.description,
      instructions: formDraft.instructions,
      tags: splitTerms(formDraft.tagsText),
      triggers: splitTerms(formDraft.triggersText),
      enabled: formDraft.enabled,
      source: existingSkill?.source ?? "user",
      allowAutoInvoke: formDraft.allowAutoInvoke,
      createdAt: existingSkill?.createdAt ?? now,
      updatedAt: now,
      path: existingSkill?.path,
      relativePath: existingSkill?.relativePath,
      metadata: existingSkill?.metadata,
    };

    const savedSkill = await onSaveSkill(skill);

    if (savedSkill) {
      setSelectedSkillId(savedSkill.id);
    }
    setFormDraft(null);
  }

  /** 删除用户自建 skill 前二次确认，文件式 skill 会移除用户目录中的对应文件夹。 */
  async function handleDeleteSkill(skill: AgentSkill) {
    if (!isUserManagedSkill(skill)) {
      return;
    }

    setPendingConfirmation({
      title: "删除 Skill",
      message: `删除 Skill「${skill.displayName}」？文件式 Skill 会移除用户 Skills 目录中的对应文件夹。`,
      confirmLabel: "删除 Skill",
      cancelLabel: "取消",
      tone: "danger",
      onConfirm: async () => {
        await onDeleteSkill(skill.id);
        setSelectedSkillId(skills.find((item) => item.id !== skill.id)?.id ?? "");
      },
    });
  }

  /** 执行已确认的 Skill 危险操作，并在业务完成后关闭确认弹窗。 */
  async function handleConfirmDialogConfirm() {
    const confirmation = pendingConfirmation;

    if (!confirmation) {
      return;
    }

    await confirmation.onConfirm();
    setPendingConfirmation(null);
  }

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="skills-modal" aria-label="Skills 能力管理" onMouseDown={(event) => event.stopPropagation()}>
        <header className="modal-header skills-modal-header">
          <div>
            <p className="section-label">Skills</p>
            <h2>管理 Agent Skills</h2>
          </div>
          <div className="skills-modal-actions">
            <button className="ghost-button" type="button" onClick={onOpenUserSkillsFolder} disabled={isBusy}>
              <FolderOpen size={14} />
              打开用户 Skills 文件夹
            </button>
            <button className="icon-button" type="button" title="关闭 Skills" onClick={onClose}>
              <X size={17} />
            </button>
          </div>
        </header>

        <div className="skills-modal-body">
          <aside className="skills-list-pane">
            <div className="skills-search">
              <Search size={15} />
              <input value={searchTerm} onChange={(event) => setSearchTerm(event.target.value)} placeholder="搜索 skill" />
            </div>
            <div className="skill-source-filter" aria-label="Skill 来源筛选">
              {(["all", "built-in", "file", "user"] as SkillSourceFilter[]).map((source) => (
                <button
                  className={sourceFilter === source ? "active" : ""}
                  key={source}
                  type="button"
                  onClick={() => setSourceFilter(source)}
                >
                  {sourceFilterLabel(source)}
                  <span>{sourceCounts[source]}</span>
                </button>
              ))}
            </div>
            <div className="skill-tag-filter" aria-label="Skill 标签筛选">
              <button className={!activeTag ? "active" : ""} type="button" onClick={() => setActiveTag("")}>
                全部
              </button>
              {availableTags.map((tag) => (
                <button className={activeTag === tag ? "active" : ""} key={tag} type="button" onClick={() => setActiveTag(tag)}>
                  {tag}
                </button>
              ))}
            </div>
            <button className="primary-button compact skill-new-button" type="button" onClick={handleCreateSkill} disabled={isBusy}>
              <Plus size={14} />
              新建 Skill
            </button>
            <div className="skills-list">
              {filteredSkills.map((skill) => (
                <button
                  className={`skill-row ${skill.id === selectedSkill?.id && !formDraft ? "active" : ""}`}
                  key={skill.id}
                  type="button"
                  onClick={() => {
                    setSelectedSkillId(skill.id);
                    setFormDraft(null);
                  }}
                >
                  <span>
                    <strong>{skill.displayName}</strong>
                    <small>{sourceLabel(skill.source)}</small>
                  </span>
                  <em className={skill.enabled ? "enabled" : "disabled"}>{skill.enabled ? "启用" : "停用"}</em>
                </button>
              ))}
              {!filteredSkills.length && <p className="skills-empty">没有匹配的 skill。</p>}
            </div>
          </aside>

          <div className="skill-detail-pane">
            {formDraft ? (
              <SkillForm
                draft={formDraft}
                isBusy={isBusy}
                onChange={setFormDraft}
                onCancel={() => setFormDraft(null)}
                onSubmit={handleSubmitSkill}
              />
            ) : selectedSkill ? (
              <SkillDetail
                skill={selectedSkill}
                isBusy={isBusy}
                onToggleSkill={onToggleSkill}
                onEditSkill={handleEditSkill}
                onDeleteSkill={handleDeleteSkill}
              />
            ) : (
              <p className="skills-empty">请选择一个 skill。</p>
            )}
          </div>
        </div>
      </section>
      {pendingConfirmation && (
        <ConfirmDialog
          {...pendingConfirmation}
          isBusy={isBusy}
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void handleConfirmDialogConfirm()}
        />
      )}
    </div>
  );
}

/** Skill 详情页，展示完整说明并提供启停与自动触发开关。 */
function SkillDetail({
  skill,
  isBusy,
  onToggleSkill,
  onEditSkill,
  onDeleteSkill,
}: {
  skill: AgentSkill;
  isBusy: boolean;
  onToggleSkill: (skillId: string, enabled: boolean, allowAutoInvoke?: boolean) => Promise<void> | void;
  onEditSkill: (skill: AgentSkill) => void;
  onDeleteSkill: (skill: AgentSkill) => Promise<void> | void;
}) {
  return (
    <article className="skill-detail">
      <div className="skill-detail-header">
        <div>
          <p className="section-label">{sourceHeading(skill)}</p>
          <h3>{skill.displayName}</h3>
          <span>{skill.name}</span>
        </div>
        <div className="skill-detail-actions">
          {isUserManagedSkill(skill) && (
            <button className="ghost-button" type="button" onClick={() => onEditSkill(skill)} disabled={isBusy}>
              <Edit3 size={14} />
              编辑
            </button>
          )}
          {isUserManagedSkill(skill) && (
            <button className="ghost-button danger-action" type="button" onClick={() => onDeleteSkill(skill)} disabled={isBusy}>
              <Trash2 size={14} />
              删除
            </button>
          )}
        </div>
      </div>
      <p>{skill.description}</p>
      {skill.source === "file" && (
        <section className="skill-path-block">
          <h4>SKILL.md 路径</h4>
          <code>{skill.path ?? skill.relativePath ?? "未返回路径"}</code>
        </section>
      )}
      <div className="skill-switches">
        <label className="toggle-row">
          <input
            checked={skill.enabled}
            onChange={(event) => onToggleSkill(skill.id, event.target.checked)}
            type="checkbox"
            disabled={isBusy}
          />
          <span>启用 Skill</span>
        </label>
        <label className="toggle-row">
          <input
            checked={skill.allowAutoInvoke}
            onChange={(event) => onToggleSkill(skill.id, skill.enabled, event.target.checked)}
            type="checkbox"
            disabled={isBusy || !skill.enabled}
          />
          <span>允许自动触发</span>
        </label>
      </div>
      <div className="skill-tags">
        {skill.tags.map((tag) => (
          <span key={tag}>{tag}</span>
        ))}
      </div>
      <section className="skill-instructions">
        <h4>执行说明</h4>
        <p>{skill.instructions}</p>
      </section>
      <section className="skill-instructions">
        <h4>触发词</h4>
        <p>{skill.triggers.length ? skill.triggers.join("、") : "未设置触发词。"}</p>
      </section>
    </article>
  );
}

/** 把 skill 来源转换为列表中的中文标签。 */
function sourceLabel(source: AgentSkillSource) {
  const labels: Record<AgentSkillSource, string> = {
    "built-in": "内置",
    file: "文件",
    user: "用户",
  };

  return labels[source];
}

/** 把来源筛选值转换为按钮标签。 */
function sourceFilterLabel(source: SkillSourceFilter) {
  if (source === "all") {
    return "全部";
  }

  return sourceLabel(source);
}

/** 详情页来源标题使用英文短标签，用户目录文件 skill 和只读外部文件 skill 分开说明。 */
function sourceHeading(skill: AgentSkill) {
  const labels: Record<AgentSkillSource, string> = {
    "built-in": "Built-in Skill",
    file: "File Skill",
    user: "User Skill",
  };

  return isUserManagedSkill(skill) ? "User Skill" : labels[skill.source];
}

/** 用户 skill 新建和编辑表单，字段与后端 AgentSkill 保持一一对应。 */
function SkillForm({
  draft,
  isBusy,
  onChange,
  onCancel,
  onSubmit,
}: {
  draft: SkillFormDraft;
  isBusy: boolean;
  onChange: (draft: SkillFormDraft) => void;
  onCancel: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  /** 更新单个表单字段，避免每个输入框重复展开整个草稿对象。 */
  function updateDraft(field: keyof SkillFormDraft, value: string | boolean) {
    onChange({ ...draft, [field]: value });
  }

  return (
    <form className="skill-form" onSubmit={onSubmit}>
      <div className="skill-detail-header">
        <div>
          <p className="section-label">User Skill</p>
          <h3>{draft.id ? "编辑 Skill 文件" : "新建 Skill 文件"}</h3>
        </div>
      </div>
      <label>
        <span>显示名称</span>
        <input value={draft.displayName} onChange={(event) => updateDraft("displayName", event.target.value)} />
      </label>
      <label>
        <span>标识 name</span>
        <input value={draft.name} onChange={(event) => updateDraft("name", event.target.value)} placeholder="my-custom-skill" />
      </label>
      <label>
        <span>描述</span>
        <input value={draft.description} onChange={(event) => updateDraft("description", event.target.value)} />
      </label>
      <label>
        <span>执行说明</span>
        <textarea value={draft.instructions} onChange={(event) => updateDraft("instructions", event.target.value)} />
      </label>
      <label>
        <span>标签</span>
        <input value={draft.tagsText} onChange={(event) => updateDraft("tagsText", event.target.value)} placeholder="写作, 研究" />
      </label>
      <label>
        <span>触发词</span>
        <input
          value={draft.triggersText}
          onChange={(event) => updateDraft("triggersText", event.target.value)}
          placeholder="改写, 总结, 引用"
        />
      </label>
      <div className="skill-switches">
        <label className="toggle-row">
          <input checked={draft.enabled} onChange={(event) => updateDraft("enabled", event.target.checked)} type="checkbox" />
          <span>启用</span>
        </label>
        <label className="toggle-row">
          <input
            checked={draft.allowAutoInvoke}
            onChange={(event) => updateDraft("allowAutoInvoke", event.target.checked)}
            type="checkbox"
          />
          <span>允许自动触发</span>
        </label>
      </div>
      <div className="modal-actions">
        <button className="ghost-button" type="button" onClick={onCancel} disabled={isBusy}>
          取消
        </button>
        <button
          className="primary-button compact"
          type="submit"
          disabled={
            isBusy ||
            !draft.name.trim() ||
            !draft.displayName.trim() ||
            !draft.description.trim() ||
            !draft.instructions.trim()
          }
        >
          <Save size={14} />
          保存为 SKILL.md
        </button>
      </div>
    </form>
  );
}

/** 把逗号、顿号或换行分隔文本转为去重后的词条数组。 */
function splitTerms(value: string) {
  const seenTerms = new Set<string>();

  return value
    .split(/[,，、\n]/)
    .map((term) => term.trim())
    .filter(Boolean)
    .filter((term) => {
      const key = term.toLowerCase();

      if (seenTerms.has(key)) {
        return false;
      }

      seenTerms.add(key);
      return true;
    });
}
