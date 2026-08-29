import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Archive, Download, Edit3, ExternalLink, FolderOpen, Link, Plus, Save, Search, Trash2, X } from "lucide-react";
import type { FormEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { previewOnlineSkill, searchOnlineSkills } from "../shared/api/skills";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { FilterChip } from "../shared/FilterChip";
import { ListRow } from "../shared/ListRow";
import { logError, logInfo, logWarn } from "../shared/logger";
import { ModalBackdrop, ModalHeader, ModalPanel } from "../shared/Modal";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { SegmentedControl, SegmentedControlItem } from "../shared/SegmentedControl";
import { ToggleRow } from "../shared/ToggleRow";
import { fieldControlClassName, fieldLabelClassName, fieldTextareaClassName, sectionLabelClassName } from "../shared/ui";
import type {
  AgentSkill,
  AgentSkillSource,
  InstallAgentSkillPayload,
  InstallAgentSkillResult,
  OnlineSkill,
  SkillInstallSourceType,
} from "../shared/types";

/** Skills 列表来源筛选，all 用于展示完整合并结果。 */
type SkillSourceFilter = "all" | AgentSkillSource;

/** Skills 弹窗主面板：已安装管理本地能力，发现用于在线目录。 */
type SkillsModalPanel = "installed" | "discover";

/** 发现页推荐分类；query 使用目录能匹配的英文词，标签用中文。 */
const DISCOVER_CHIPS: Array<{ id: string; label: string; query: string; owner?: string }> = [
  { id: "writing", label: "写作", query: "writing" },
  { id: "note", label: "笔记", query: "note" },
  { id: "pdf", label: "PDF", query: "pdf" },
  { id: "organize", label: "整理", query: "organize" },
  { id: "translate", label: "翻译", query: "translate" },
  { id: "official", label: "官方", query: "skill", owner: "anthropics" },
];

/** 在线搜索防抖，避免每个按键都打到 skills.sh。 */
const DISCOVER_SEARCH_DEBOUNCE_MS = 250;

/** 把搜索词和 owner 编成缓存键，空 owner 与未指定来源视为同一次搜索。 */
function discoverSearchCacheKey(query: string, owner: string) {
  return `${query}\0${owner}`;
}

/** 用户目录中的自定义 skill 允许用户编辑和删除。 */
function isUserManagedSkill(skill: AgentSkill) {
  return skill.source === "custom";
}

/** Skill 表单草稿，标签在 UI 中用逗号分隔编辑。 */
interface SkillFormDraft {
  id: string;
  name: string;
  displayName: string;
  description: string;
  instructions: string;
  tagsText: string;
  enabled: boolean;
}

/** Skill 安装表单草稿；本地来源 source 留空时由 Tauri 打开系统选择器。 */
interface SkillInstallDraft {
  sourceType: SkillInstallSourceType;
  source: string;
  enableAfterInstall: boolean;
  replaceExisting: boolean;
}

/** 待确认的 Skill 操作；确认后才执行删除，避免依赖系统 confirm。 */
interface PendingSkillConfirmation extends ConfirmDialogConfig {
  onConfirm: () => Promise<void> | void;
}

/** 安装表单默认值，第三方 skill 默认停用，避免未审阅能力进入 Runtime。 */
const DEFAULT_INSTALL_DRAFT: SkillInstallDraft = {
  sourceType: "url",
  source: "",
  enableAfterInstall: false,
  replaceExisting: false,
};

/** Skills 管理弹窗，提供浏览、筛选、启停和用户自建 skill CRUD。 */
export function SkillsModal({
  skills,
  isBusy,
  onSaveSkill,
  onInstallSkill,
  onToggleSkill,
  onDeleteSkill,
  onOpenUserSkillsFolder,
  onClose,
}: {
  skills: AgentSkill[];
  isBusy: boolean;
  onSaveSkill: (skill: AgentSkill) => Promise<AgentSkill | void> | AgentSkill | void;
  onInstallSkill: (payload: InstallAgentSkillPayload) => Promise<InstallAgentSkillResult> | InstallAgentSkillResult;
  onToggleSkill: (skillId: string, enabled: boolean) => Promise<void> | void;
  onDeleteSkill: (skillId: string) => Promise<void> | void;
  onOpenUserSkillsFolder: () => Promise<void> | void;
  onClose: () => void;
}) {
  /** 搜索词同时匹配名称、说明和标签。 */
  const [searchTerm, setSearchTerm] = useState("");
  /** 来源筛选帮助用户区分内置和用户目录自定义 skill。 */
  const [sourceFilter, setSourceFilter] = useState<SkillSourceFilter>("all");
  /** 标签筛选使用单选，避免多标签组合导致列表空状态难理解。 */
  const [activeTag, setActiveTag] = useState("");
  /** 当前详情面板展示的 skill ID，新建时为空。 */
  const [selectedSkillId, setSelectedSkillId] = useState(skills[0]?.id ?? "");
  /** 表单草稿存在时详情面板切换为新建或编辑模式。 */
  const [formDraft, setFormDraft] = useState<SkillFormDraft | null>(null);
  /** 安装草稿存在时详情面板展示第三方 skill 安装入口。 */
  const [installDraft, setInstallDraft] = useState<SkillInstallDraft | null>(null);
  /** 当前等待用户确认的危险操作，使用应用内弹窗承载。 */
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingSkillConfirmation | null>(null);
  /** 已安装与发现是两个并列入口，避免在线目录冲掉本地管理。 */
  const [panel, setPanel] = useState<SkillsModalPanel>("installed");
  /** 发现页搜索词；空状态只展示分类芯片。 */
  const [discoverQuery, setDiscoverQuery] = useState("");
  /** 可选 GitHub owner，官方芯片会带上 anthropics。 */
  const [discoverOwner, setDiscoverOwner] = useState("");
  /** 当前选中的推荐分类，便于芯片高亮。 */
  const [activeDiscoverChip, setActiveDiscoverChip] = useState("");
  const [discoverResults, setDiscoverResults] = useState<OnlineSkill[]>([]);
  const [discoverError, setDiscoverError] = useState("");
  const [isSearching, setIsSearching] = useState(false);
  const [selectedOnlineSkillId, setSelectedOnlineSkillId] = useState("");
  const [onlinePreviewDescription, setOnlinePreviewDescription] = useState("");
  const [replaceOnlineSkill, setReplaceOnlineSkill] = useState(false);
  /** 会话内缓存已成功的在线搜索，切回已搜过的分类或关键词时不再请求。 */
  const discoverSearchCacheRef = useRef(new Map<string, OnlineSkill[]>());

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
      custom: skills.filter((skill) => skill.source === "custom").length,
    }),
    [skills],
  );
  /** 能力管理摘要只展示数量和安全默认值，避免把 skill 路径写入顶部状态。 */
  const skillSummary = useMemo(
    () => ({
      enabled: skills.filter((skill) => skill.enabled).length,
      custom: skills.filter((skill) => skill.source === "custom").length,
      builtIn: skills.filter((skill) => skill.source === "built-in").length,
    }),
    [skills],
  );
  /** 根据搜索词、来源和标签得到展示列表，后端已保证内置、自定义的合并顺序。 */
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
  /** 发现页当前选中项；结果刷新后尽量保留原选择。 */
  const selectedOnlineSkill =
    discoverResults.find((skill) => skill.id === selectedOnlineSkillId) ?? discoverResults[0];
  const installedOnlineSkill = selectedOnlineSkill
    ? skills.find((skill) => skill.name === selectedOnlineSkill.skillId || skill.name === selectedOnlineSkill.name)
    : undefined;

  /** 切换到发现页时收起本地表单，避免两个面板抢同一详情区。 */
  function handleSelectPanel(nextPanel: SkillsModalPanel) {
    setPanel(nextPanel);
    setFormDraft(null);
    setInstallDraft(null);
  }

  /** 推荐分类写入搜索词和 owner，由防抖 effect 真正发请求。 */
  function handleSelectDiscoverChip(chipId: string) {
    const chip = DISCOVER_CHIPS.find((item) => item.id === chipId);

    if (!chip) {
      return;
    }

    setActiveDiscoverChip(chip.id);
    setDiscoverQuery(chip.query);
    setDiscoverOwner(chip.owner ?? "");
  }

  /** 在系统浏览器打开 skills.sh 详情，主窗口不跟跳。 */
  async function handleOpenOnlineSkillPage(url: string) {
    try {
      if (!isTauri()) {
        const browserWindow = globalThis.open?.(url, "_blank", "noopener,noreferrer");

        if (!browserWindow) {
          throw new Error("浏览器拦截了外部链接。");
        }

        return;
      }

      await openUrl(url);
    } catch (error) {
      logWarn("打开在线 Skill 页面失败。", {
        category: "skill",
        event: "open_online_skill_page",
        status: "failed",
        error,
      });
    }
  }

  /** 发现页安装前二次确认；默认停用，冲突时由替换开关决定。 */
  function handleInstallOnlineSkill(skill: OnlineSkill) {
    if (!skill.installable) {
      return;
    }

    setPendingConfirmation({
      title: "安装在线 Skill",
      message: `安装第三方 Skill「${skill.name}」？安装后默认停用，请先审阅再启用。`,
      confirmLabel: "安装 Skill",
      cancelLabel: "取消",
      onConfirm: async () => {
        const result = await onInstallSkill({
          sourceType: "url",
          source: `https://github.com/${skill.source}`,
          enableAfterInstall: false,
          conflictStrategy: replaceOnlineSkill ? "replace" : "fail",
          skillNames: [skill.skillId],
        });
        const firstInstalledSkill = result.installedSkills[0];

        setPanel("installed");
        setFormDraft(null);
        setInstallDraft(null);

        if (firstInstalledSkill) {
          setSelectedSkillId(firstInstalledSkill.id);
        }
      },
    });
  }

  useEffect(() => {
    if (panel !== "discover") {
      return;
    }

    const query = discoverQuery.trim();
    const owner = discoverOwner.trim();

    if (query.length < 2) {
      setDiscoverResults([]);
      setDiscoverError("");
      setIsSearching(false);
      return;
    }

    const cacheKey = discoverSearchCacheKey(query, owner);

    // 命中会话缓存时直接还原结果，避免切回已搜过的标签再次请求。
    if (discoverSearchCacheRef.current.has(cacheKey)) {
      const cachedSkills = discoverSearchCacheRef.current.get(cacheKey) ?? [];
      setDiscoverResults(cachedSkills);
      setDiscoverError("");
      setIsSearching(false);
      setSelectedOnlineSkillId((currentId) =>
        cachedSkills.some((skill) => skill.id === currentId) ? currentId : (cachedSkills[0]?.id ?? ""),
      );
      return;
    }

    let cancelled = false;
    const timeoutId = window.setTimeout(() => {
      setIsSearching(true);
      void searchOnlineSkills({
        query,
        owner: owner || undefined,
      })
        .then((result) => {
          // 成功结果写入缓存，即使已经切走也保留，方便马上切回来。
          discoverSearchCacheRef.current.set(cacheKey, result.skills);

          if (cancelled) {
            return;
          }

          setDiscoverResults(result.skills);
          setDiscoverError("");
          setSelectedOnlineSkillId((currentId) =>
            result.skills.some((skill) => skill.id === currentId) ? currentId : (result.skills[0]?.id ?? ""),
          );
        })
        .catch((error) => {
          if (cancelled) {
            return;
          }

          setDiscoverResults([]);
          setDiscoverError(error instanceof Error ? error.message : String(error));
        })
        .finally(() => {
          if (!cancelled) {
            setIsSearching(false);
          }
        });
    }, DISCOVER_SEARCH_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timeoutId);
    };
  }, [discoverOwner, discoverQuery, panel]);

  useEffect(() => {
    if (panel !== "discover" || !selectedOnlineSkill) {
      setOnlinePreviewDescription("");
      return;
    }

    if (selectedOnlineSkill.description) {
      setOnlinePreviewDescription(selectedOnlineSkill.description);
      return;
    }

    let cancelled = false;

    void previewOnlineSkill(selectedOnlineSkill.id)
      .then((preview) => {
        if (!cancelled) {
          setOnlinePreviewDescription(preview.description ?? "");
        }
      })
      .catch(() => {
        if (!cancelled) {
          setOnlinePreviewDescription("");
        }
      });

    return () => {
      cancelled = true;
    };
  }, [panel, selectedOnlineSkill]);

  /** 打开新建用户 skill 表单，默认启用。 */
  function handleCreateSkill() {
    setSelectedSkillId("");
    setInstallDraft(null);
    setFormDraft({
      id: "",
      name: "",
      displayName: "",
      description: "",
      instructions: "",
      tagsText: "",
      enabled: true,
    });
  }

  /** 打开编辑用户可管理 skill 表单；内置 skill 不允许编辑说明内容。 */
  function handleEditSkill(skill: AgentSkill) {
    if (!isUserManagedSkill(skill)) {
      return;
    }

    setSelectedSkillId(skill.id);
    setInstallDraft(null);
    setFormDraft({
      id: skill.id,
      name: skill.name,
      displayName: skill.displayName,
      description: skill.description,
      instructions: skill.instructions,
      tagsText: skill.tags.join(", "),
      enabled: skill.enabled,
    });
  }

  /** 打开安装表单，使用安全默认值：安装后停用且不覆盖同名 skill。 */
  function handleOpenInstallSkill() {
    setSelectedSkillId("");
    setFormDraft(null);
    setInstallDraft(DEFAULT_INSTALL_DRAFT);
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
      enabled: formDraft.enabled,
      source: existingSkill?.source ?? "custom",
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

  /** 提交第三方 skill 安装请求，日志只记录类型和策略，不记录 URL 或本地路径。 */
  async function handleSubmitInstall(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    if (!installDraft) {
      return;
    }

    const startedAt = performance.now();
    const payload: InstallAgentSkillPayload = {
      sourceType: installDraft.sourceType,
      source: installDraft.source.trim() || undefined,
      enableAfterInstall: installDraft.enableAfterInstall,
      conflictStrategy: installDraft.replaceExisting ? "replace" : "fail",
    };

    logInfo("设置页提交 Skill 安装。", {
      category: "skill",
      event: "skill_install_submit",
      status: "started",
      metadata: {
        sourceType: payload.sourceType,
        conflictStrategy: payload.conflictStrategy,
        enableAfterInstall: payload.enableAfterInstall,
        hasSource: Boolean(payload.source),
      },
    });

    try {
      const result = await onInstallSkill(payload);
      const firstInstalledSkill = result.installedSkills[0];

      if (firstInstalledSkill) {
        setSelectedSkillId(firstInstalledSkill.id);
      }
      setInstallDraft(null);
      logInfo("设置页 Skill 安装提交完成。", {
        category: "skill",
        event: "skill_install_submit",
        status: "completed",
        durationMs: performance.now() - startedAt,
        metadata: {
          sourceType: result.sourceType,
          installedCount: result.installedCount,
          warningCount: result.warnings.length,
        },
      });
    } catch (error) {
      logError("设置页 Skill 安装提交失败。", {
        category: "skill",
        event: "skill_install_submit",
        status: "failed",
        durationMs: performance.now() - startedAt,
        error,
        metadata: {
          sourceType: payload.sourceType,
          conflictStrategy: payload.conflictStrategy,
        },
      });
    }
  }

  /** 删除用户自建 skill 前二次确认，自定义 skill 会移除用户目录中的对应文件夹。 */
  async function handleDeleteSkill(skill: AgentSkill) {
    if (!isUserManagedSkill(skill)) {
      return;
    }

    setPendingConfirmation({
      title: "删除 Skill",
      message: `删除 Skill「${skill.displayName}」？自定义 Skill 会移除用户 Skills 目录中的对应文件夹。`,
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
    <ModalBackdrop onClose={onClose} className="p-6">
      <ModalPanel
        className="h-[min(760px,calc(100vh-48px))] w-[min(940px,calc(100vw-48px))] grid-rows-[auto_minmax(0,1fr)] max-[980px]:w-[min(920px,calc(100vw-24px))] max-[760px]:h-auto max-[760px]:w-[min(100%,calc(100vw-20px))]"
        aria-label="Skills 能力管理"
      >
        <ModalHeader>
          <div className="min-w-0">
            <p className={sectionLabelClassName}>Skills</p>
            <h2 className="mt-1 mb-0 text-lg leading-tight text-ink-strong">管理 Agent Skills</h2>
            <span className="mt-1 block text-xs text-ink-muted">
              {panel === "discover" ? "从公开目录搜索并安装 Skill，安装后默认停用。" : "启用的 Skill 会作为能力说明进入 Agent 上下文。"}
            </span>
            <SegmentedControl className="mt-2.5" aria-label="Skills 面板">
              <SegmentedControlItem active={panel === "installed"} onClick={() => handleSelectPanel("installed")}>
                已安装
              </SegmentedControlItem>
              <SegmentedControlItem active={panel === "discover"} onClick={() => handleSelectPanel("discover")}>
                发现
              </SegmentedControlItem>
            </SegmentedControl>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {panel === "installed" && (
              <Button variant="ghost" onClick={onOpenUserSkillsFolder} disabled={isBusy}>
                <FolderOpen size={14} />
                打开用户 Skills 文件夹
              </Button>
            )}
            <Button variant="icon" title="关闭 Skills" onClick={onClose}>
              <X size={17} />
            </Button>
          </div>
        </ModalHeader>

        {panel === "discover" ? (
          <div className="grid min-h-0 overflow-hidden grid-cols-[300px_minmax(0,1fr)] max-[980px]:grid-cols-[minmax(220px,280px)_minmax(0,1fr)] max-[760px]:grid-cols-1 max-[760px]:grid-rows-[minmax(180px,38%)_minmax(0,1fr)]">
            <aside className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)_auto] gap-2.5 overflow-hidden border-r border-border bg-warm-panel p-3.5 max-[760px]:border-r-0 max-[760px]:border-b">
              <div className="flex items-center gap-[7px] rounded-[7px] border border-border bg-surface-translucent px-[9px] text-ink-muted">
                <Search size={15} />
                <input
                  className="min-h-[34px] w-full border-0 bg-transparent outline-0"
                  value={discoverQuery}
                  onChange={(event) => {
                    setDiscoverQuery(event.target.value);
                    setActiveDiscoverChip("");
                    setDiscoverOwner("");
                  }}
                  placeholder="搜索在线 Skills，例如 写作、PDF"
                  aria-label="搜索在线 Skills"
                />
              </div>
              <div className="flex flex-wrap gap-1.5" aria-label="推荐分类">
                {DISCOVER_CHIPS.map((chip) => (
                  <FilterChip active={activeDiscoverChip === chip.id} key={chip.id} onClick={() => handleSelectDiscoverChip(chip.id)}>
                    {chip.label}
                  </FilterChip>
                ))}
              </div>
              <div className="grid min-h-0 content-start gap-2 overflow-auto">
                {isSearching && <p className="m-0 text-[13px] text-ink-muted">正在搜索在线 Skills…</p>}
                {!isSearching && discoverError && <p className="m-0 text-[13px] text-ink-muted">{discoverError}</p>}
                {!isSearching && !discoverError && discoverQuery.trim().length < 2 && (
                  <p className="m-0 text-[13px] text-ink-muted">从分类开始，或输入至少两个字搜索公开目录。</p>
                )}
                {!isSearching &&
                  !discoverError &&
                  discoverQuery.trim().length >= 2 &&
                  !discoverResults.length && <p className="m-0 text-[13px] text-ink-muted">没有匹配的在线 Skill。</p>}
                {discoverResults.map((skill) => {
                  const isInstalled = skills.some((item) => item.name === skill.skillId || item.name === skill.name);

                  return (
                    <ListRow
                      className="grid grid-cols-[minmax(0,1fr)_auto] border-border-translucent bg-surface-translucent"
                      active={skill.id === selectedOnlineSkill?.id}
                      key={skill.id}
                      onClick={() => setSelectedOnlineSkillId(skill.id)}
                    >
                      <span className="min-w-0">
                        <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={skill.name} logArea="skills_discover_row_name" />
                        <OverflowTooltipText
                          as="small"
                          className="mt-[3px] block truncate text-xs text-ink-muted"
                          text={`${skill.source} · ${formatInstallCount(skill.installs)}`}
                          logArea="skills_discover_row_source"
                        />
                      </span>
                      <em
                        className={
                          isInstalled
                            ? "rounded-full bg-success-soft px-[7px] py-[3px] text-xs not-italic text-success"
                            : "rounded-full bg-surface-muted px-[7px] py-[3px] text-xs not-italic text-ink-muted"
                        }
                      >
                        {isInstalled ? "已安装" : skill.installable ? "可安装" : "需手动"}
                      </em>
                    </ListRow>
                  );
                })}
              </div>
              <p className="m-0 text-[11px] leading-normal text-ink-muted">搜索只把搜索词发给 skills.sh，不会上传笔记。</p>
            </aside>
            <div className="min-h-0 min-w-0 overflow-auto bg-surface p-4">
              {selectedOnlineSkill ? (
                <OnlineSkillPreviewPanel
                  skill={selectedOnlineSkill}
                  description={onlinePreviewDescription || selectedOnlineSkill.description}
                  isBusy={isBusy}
                  isInstalled={Boolean(installedOnlineSkill)}
                  replaceExisting={replaceOnlineSkill}
                  onReplaceExistingChange={setReplaceOnlineSkill}
                  onInstall={() => handleInstallOnlineSkill(selectedOnlineSkill)}
                  onOpenPage={() => void handleOpenOnlineSkillPage(selectedOnlineSkill.pageUrl)}
                />
              ) : (
                <p className="m-0 text-[13px] text-ink-muted">选择一个在线 Skill 查看简介并安装。</p>
              )}
            </div>
          </div>
        ) : null}
        {panel === "installed" ? (
        <div className="grid min-h-0 overflow-hidden grid-cols-[300px_minmax(0,1fr)] max-[980px]:grid-cols-[minmax(220px,280px)_minmax(0,1fr)] max-[760px]:grid-cols-1 max-[760px]:grid-rows-[minmax(180px,38%)_minmax(0,1fr)]">
          <aside className="grid min-h-0 grid-rows-[auto_auto_auto_auto_auto_auto_minmax(0,1fr)] gap-2.5 overflow-hidden border-r border-border bg-warm-panel p-3.5 max-[760px]:border-r-0 max-[760px]:border-b">
            <div className="grid grid-cols-3 gap-[7px] rounded-control border border-border-translucent bg-surface-translucent p-2.5 text-xs text-ink-muted" aria-label="Skills 摘要">
              <span className="grid gap-0.5 text-center">
                <strong className="text-base text-ink-strong">{skillSummary.enabled}</strong>
                启用
              </span>
              <span className="grid gap-0.5 text-center">
                <strong className="text-base text-ink-strong">{skillSummary.builtIn}</strong>
                内置
              </span>
              <span className="grid gap-0.5 text-center">
                <strong className="text-base text-ink-strong">{skillSummary.custom}</strong>
                自定义
              </span>
            </div>
            <div className="flex items-center gap-[7px] rounded-[7px] border border-border bg-surface-translucent px-[9px] text-ink-muted">
              <Search size={15} />
              <input className="min-h-[34px] w-full border-0 bg-transparent outline-0" value={searchTerm} onChange={(event) => setSearchTerm(event.target.value)} placeholder="搜索 skill" />
            </div>
            <div className="flex flex-wrap gap-1.5" aria-label="Skill 来源筛选">
              {(["all", "built-in", "custom"] as SkillSourceFilter[]).map((source) => (
                <FilterChip active={sourceFilter === source} key={source} onClick={() => setSourceFilter(source)}>
                  {sourceFilterLabel(source)}
                  <span className="text-[11px] text-ink-soft">{sourceCounts[source]}</span>
                </FilterChip>
              ))}
            </div>
            <div className="flex flex-wrap gap-1.5" aria-label="Skill 标签筛选">
              <FilterChip active={!activeTag} onClick={() => setActiveTag("")}>
                全部
              </FilterChip>
              {availableTags.map((tag) => (
                <FilterChip active={activeTag === tag} key={tag} onClick={() => setActiveTag(tag)}>
                  {tag}
                </FilterChip>
              ))}
            </div>
            <Button variant="primary" size="compact" className="w-full" onClick={handleCreateSkill} disabled={isBusy}>
              <Plus size={14} />
              新建 Skill
            </Button>
            <Button variant="ghost" className="w-full" onClick={handleOpenInstallSkill} disabled={isBusy}>
              <Download size={14} />
              安装 Skill
            </Button>
            <div className="grid min-h-0 content-start gap-2 overflow-auto">
              {filteredSkills.map((skill) => (
                <ListRow
                  className="grid grid-cols-[minmax(0,1fr)_auto] border-border-translucent bg-surface-translucent"
                  active={skill.id === selectedSkill?.id && !formDraft && !installDraft}
                  key={skill.id}
                  onClick={() => {
                    setSelectedSkillId(skill.id);
                    setFormDraft(null);
                    setInstallDraft(null);
                  }}
                >
                  <span className="min-w-0">
                    <OverflowTooltipText as="strong" className="block truncate text-ink-strong" text={skill.displayName} logArea="skills_modal_row_name" />
                    <OverflowTooltipText
                      as="small"
                      className="mt-[3px] block truncate text-xs text-ink-muted"
                      text={`${sourceLabel(skill.source)} · ${skillCompatibilityLabel(skill)}`}
                      logArea="skills_modal_row_source"
                    />
                  </span>
                  <em
                    className={
                      skill.enabled
                        ? "rounded-full bg-success-soft px-[7px] py-[3px] text-xs not-italic text-success"
                        : "rounded-full bg-surface-muted px-[7px] py-[3px] text-xs not-italic text-ink-muted"
                    }
                  >
                    {skill.enabled ? "启用" : "停用"}
                  </em>
                </ListRow>
              ))}
              {!filteredSkills.length && <p className="m-0 text-[13px] text-ink-muted">没有匹配的 skill。</p>}
            </div>
          </aside>

          <div className="min-h-0 min-w-0 overflow-auto bg-surface p-4">
            {installDraft ? (
              <SkillInstallForm
                draft={installDraft}
                isBusy={isBusy}
                onChange={setInstallDraft}
                onCancel={() => setInstallDraft(null)}
                onSubmit={handleSubmitInstall}
              />
            ) : formDraft ? (
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
              <p className="m-0 text-[13px] text-ink-muted">请选择一个 skill。</p>
            )}
          </div>
        </div>
        ) : null}
      </ModalPanel>
      {pendingConfirmation && (
        <ConfirmDialog
          {...pendingConfirmation}
          isBusy={isBusy}
          onCancel={() => setPendingConfirmation(null)}
          onConfirm={() => void handleConfirmDialogConfirm()}
        />
      )}
    </ModalBackdrop>
  );
}

/** 发现页右侧预览：名称、来源、安装量、简介和一键安装。 */
function OnlineSkillPreviewPanel({
  skill,
  description,
  isBusy,
  isInstalled,
  replaceExisting,
  onReplaceExistingChange,
  onInstall,
  onOpenPage,
}: {
  skill: OnlineSkill;
  description?: string;
  isBusy: boolean;
  isInstalled: boolean;
  replaceExisting: boolean;
  onReplaceExistingChange: (checked: boolean) => void;
  onInstall: () => void;
  onOpenPage: () => void;
}) {
  return (
    <article className="grid gap-3.5">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className={sectionLabelClassName}>Online Skill</p>
          <OverflowTooltipText as="h3" className="mt-1 mb-0 text-xl leading-tight text-ink-strong [overflow-wrap:anywhere]" text={skill.name} logArea="skills_discover_detail_name" />
          <OverflowTooltipText className="mt-[3px] block text-xs text-ink-muted" text={skill.source} logArea="skills_discover_detail_source" />
        </div>
        <Button variant="ghost" onClick={onOpenPage} disabled={isBusy}>
          <ExternalLink size={14} />
          在 skills.sh 打开
        </Button>
      </div>
      <p className="m-0 text-sm leading-[1.6] text-[#24323c]">{description || "暂无简介，可打开 skills.sh 查看完整说明。"}</p>
      <section className="grid gap-1.5 rounded-[7px] border border-border-translucent bg-warm-panel p-2.5">
        <h4 className="m-0 text-[13px] text-ink-strong">目录信息</h4>
        <p className="m-0 text-xs leading-[1.55] text-ink-muted">{formatInstallCount(skill.installs)} · 第三方来源，安装量仅供参考。</p>
      </section>
      <section className="grid gap-1.5 rounded-[7px] border border-border-translucent bg-warm-panel p-2.5">
        <h4 className="m-0 text-[13px] text-ink-strong">安装边界</h4>
        <p className="m-0 text-xs leading-[1.55] text-ink-muted">
          {skill.installable
            ? "只会安装这一条 Skill，不会把整个仓库装进来。安装后默认停用，脚本仍须声明并审批后才能执行。"
            : "此来源不是 GitHub 仓库，暂不支持一键安装。可打开 skills.sh 查看后改用链接或本地文件夹安装。"}
        </p>
      </section>
      {skill.installable && isInstalled && (
        <ToggleRow checked={replaceExisting} disabled={isBusy} onChange={onReplaceExistingChange}>
          替换同名 Skill
        </ToggleRow>
      )}
      <div className="flex min-w-0 flex-wrap justify-end gap-2">
        <Button variant="primary" size="compact" onClick={onInstall} disabled={isBusy || !skill.installable}>
          <Download size={14} />
          {isInstalled ? "重新安装" : "安装 Skill"}
        </Button>
      </div>
    </article>
  );
}

/** 把安装量格式化为中文短标签。 */
function formatInstallCount(count: number) {
  if (count >= 10_000) {
    const wan = count / 10_000;
    const label = wan >= 10 ? wan.toFixed(0) : wan.toFixed(1).replace(/\.0$/, "");
    return `${label} 万次安装`;
  }

  if (count >= 1_000) {
    return `${(count / 1_000).toFixed(1).replace(/\.0$/, "")} 千次安装`;
  }

  return `${count} 次安装`;
}

/** Skill 详情页，展示完整说明并提供启停开关。 */
function SkillDetail({
  skill,
  isBusy,
  onToggleSkill,
  onEditSkill,
  onDeleteSkill,
}: {
  skill: AgentSkill;
  isBusy: boolean;
  onToggleSkill: (skillId: string, enabled: boolean) => Promise<void> | void;
  onEditSkill: (skill: AgentSkill) => void;
  onDeleteSkill: (skill: AgentSkill) => Promise<void> | void;
}) {
  return (
    <article className="grid gap-3.5">
      <div className="flex items-start justify-between gap-3 max-[760px]:items-start">
        <div className="min-w-0">
          <p className={sectionLabelClassName}>{sourceHeading(skill)}</p>
          <OverflowTooltipText as="h3" className="mt-1 mb-0 text-xl leading-tight text-ink-strong [overflow-wrap:anywhere]" text={skill.displayName} logArea="skills_modal_detail_name" />
          <OverflowTooltipText className="mt-[3px] block text-xs text-ink-muted" text={skill.name} logArea="skills_modal_detail_id" />
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {isUserManagedSkill(skill) && (
            <Button variant="ghost" onClick={() => onEditSkill(skill)} disabled={isBusy}>
              <Edit3 size={14} />
              编辑
            </Button>
          )}
          {isUserManagedSkill(skill) && (
            <Button variant="ghost" tone="danger" onClick={() => onDeleteSkill(skill)} disabled={isBusy}>
              <Trash2 size={14} />
              删除
            </Button>
          )}
        </div>
      </div>
      <p className="m-0 text-sm leading-[1.6] text-[#24323c]">{skill.description}</p>
      {skill.source === "custom" && (
        <section className="grid gap-2 rounded-[7px] border border-border-translucent bg-warm-panel p-2.5">
          <h4 className="m-0 text-[13px] text-ink-strong">SKILL.md 路径</h4>
          <OverflowTooltipText as="code" className="[overflow-wrap:anywhere] font-mono text-xs leading-[1.45] text-ink-muted [word-break:break-word]" text={skill.path ?? skill.relativePath ?? "未返回路径"} logArea="skills_modal_detail_path" />
        </section>
      )}
      <section className="grid gap-2">
        <h4 className="m-0 text-[13px] text-ink-strong">运行兼容性</h4>
        <div
          className={cn(
            "flex items-center justify-between gap-3 rounded-md border border-border bg-surface-warm px-3 py-2.5",
            skill.compatibility?.status === "ready" && "[&_strong]:text-success",
            (skill.compatibility?.status === "missing-runtime" || skill.compatibility?.status === "partial" || skill.compatibility?.status === "unsupported") && "[&_strong]:text-warning",
          )}
        >
          <strong>{skillCompatibilityLabel(skill)}</strong>
          <span className="text-right text-xs text-ink-muted">{skillRuntimeMessage(skill)}</span>
        </div>
        {skill.runtimeManifest && (
          <dl className="m-0">
            <div className="grid grid-cols-[72px_minmax(0,1fr)] gap-2 py-[3px]"><dt className="m-0 min-w-0">运行时</dt><dd className="m-0 min-w-0">{skill.runtimeManifest.runtime}</dd></div>
            <div className="grid grid-cols-[72px_minmax(0,1fr)] gap-2 py-[3px]"><dt className="m-0 min-w-0">入口</dt><dd className="m-0 min-w-0"><code className="[overflow-wrap:anywhere]">{skill.runtimeManifest.entry}</code></dd></div>
            <div className="grid grid-cols-[72px_minmax(0,1fr)] gap-2 py-[3px]"><dt className="m-0 min-w-0">网络</dt><dd className="m-0 min-w-0">{skill.runtimeManifest.networkDomains.length ? skill.runtimeManifest.networkDomains.join(", ") : "关闭"}</dd></div>
          </dl>
        )}
      </section>
      <div className="flex flex-wrap items-center gap-2">
        <ToggleRow checked={skill.enabled} disabled={isBusy} onChange={(checked) => onToggleSkill(skill.id, checked)}>
          启用 Skill
        </ToggleRow>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {skill.tags.map((tag) => (
          <OverflowTooltipText key={tag} className="rounded-full border border-[rgba(230,224,214,0.78)] bg-white/60 px-2 py-1 text-xs text-ink-muted" text={tag} logArea="skills_modal_detail_tag" />
        ))}
      </div>
      <section className="rounded-[7px] border border-border-translucent bg-warm-panel p-3">
        <h4 className="mb-2 mt-0 text-[13px] text-ink-strong">执行说明</h4>
        <p className="m-0 whitespace-pre-wrap text-[13px] leading-[1.6] text-[#24323c]">{skill.instructions}</p>
      </section>
    </article>
  );
}

/** 把 skill 来源转换为列表中的中文标签。 */
function sourceLabel(source: AgentSkillSource) {
  const labels: Record<AgentSkillSource, string> = {
    "built-in": "内置",
    custom: "自定义",
  };

  return labels[source];
}

/** 将后端兼容性状态转换为简短、可操作的中文标签。 */
function skillCompatibilityLabel(skill: AgentSkill) {
  const status = skill.compatibility?.status ?? "instruction-only";
  const labels = {
    "instruction-only": "纯指令",
    ready: "可运行",
    "missing-runtime": "缺运行时",
    "approval-required": "需审批",
    partial: "部分支持",
    unsupported: "不支持",
  } as const;

  return labels[status];
}

/** 运行兼容性说明不暴露绝对运行时路径，只显示版本和后端诊断。 */
function skillRuntimeMessage(skill: AgentSkill) {
  if (!skill.runtimeManifest) {
    return "作为提示词工作流使用，不启动本地进程。";
  }
  const runtime = skill.compatibility?.runtime;
  const warning = skill.compatibility?.warnings[0];

  return warning ?? runtime?.version ?? runtime?.message ?? "等待本机兼容性检测。";
}

/** 把来源筛选值转换为按钮标签。 */
function sourceFilterLabel(source: SkillSourceFilter) {
  if (source === "all") {
    return "全部";
  }

  return sourceLabel(source);
}

/** 详情页来源标题使用英文短标签，用户目录自定义 skill 展示为可管理项。 */
function sourceHeading(skill: AgentSkill) {
  const labels: Record<AgentSkillSource, string> = {
    "built-in": "Built-in Skill",
    custom: "Custom Skill",
  };

  return isUserManagedSkill(skill) ? "Custom Skill" : labels[skill.source];
}

/** 第三方 skill 安装表单，支持 URL、本地文件夹和本地 zip 三种来源。 */
function SkillInstallForm({
  draft,
  isBusy,
  onChange,
  onCancel,
  onSubmit,
}: {
  draft: SkillInstallDraft;
  isBusy: boolean;
  onChange: (draft: SkillInstallDraft) => void;
  onCancel: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  const sourcePlaceholder =
    draft.sourceType === "url"
      ? "https://github.com/owner/repo/tree/main/skill"
      : draft.sourceType === "localFolder"
        ? "留空后选择本地文件夹"
        : "留空后选择本地 .zip";
  const sourceHelp =
    draft.sourceType === "url"
      ? "支持 HTTPS、GitHub tree/blob/repo 链接和 raw SKILL.md。"
      : draft.sourceType === "localFolder"
        ? "文件夹中可以包含一个或多个带 SKILL.md 的 skill 目录。"
        : "仅支持 .zip，安装时会拒绝路径穿越并跳过隐藏目录。";

  /** 更新安装草稿字段；切换来源类型时清空输入，避免旧路径误用于新模式。 */
  function updateDraft(field: keyof SkillInstallDraft, value: string | boolean) {
    if (field === "sourceType") {
      onChange({ ...draft, sourceType: value as SkillInstallSourceType, source: "" });
      return;
    }

    onChange({ ...draft, [field]: value });
  }

  return (
    <form className="grid gap-3.5" onSubmit={onSubmit}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className={sectionLabelClassName}>Install Skill</p>
          <h3 className="mt-1 mb-0 text-xl leading-tight text-ink-strong">安装 Skill</h3>
          <span className="mt-[3px] block text-xs text-ink-muted">第三方 skill 安装后默认停用。</span>
        </div>
      </div>
      <div className="grid grid-cols-3 gap-2" aria-label="Skill 安装来源">
        {(["url", "localFolder", "localArchive"] as SkillInstallSourceType[]).map((sourceType) => {
          const SourceIcon = sourceType === "url" ? Link : sourceType === "localFolder" ? FolderOpen : Archive;

          return (
            <FilterChip
              className="min-w-0 justify-center rounded-control p-2 font-bold"
              active={draft.sourceType === sourceType}
              key={sourceType}
              onClick={() => updateDraft("sourceType", sourceType)}
              disabled={isBusy}
            >
              <SourceIcon size={14} />
              {installSourceLabel(sourceType)}
            </FilterChip>
          );
        })}
      </div>
      <label className={fieldLabelClassName}>
        <span>{draft.sourceType === "url" ? "安装 URL" : "本地来源"}</span>
        <input className={fieldControlClassName} value={draft.source} onChange={(event) => updateDraft("source", event.target.value)} placeholder={sourcePlaceholder} />
      </label>
      <p className="-mt-1.5 mb-0 text-xs leading-normal text-ink-muted">{sourceHelp}</p>
      <div className="flex flex-wrap items-center gap-2">
        <ToggleRow checked={draft.enableAfterInstall} disabled={isBusy} onChange={(checked) => updateDraft("enableAfterInstall", checked)}>
          安装后启用
        </ToggleRow>
        <ToggleRow checked={draft.replaceExisting} disabled={isBusy} onChange={(checked) => updateDraft("replaceExisting", checked)}>
          替换同名 Skill
        </ToggleRow>
      </div>
      <section className="grid gap-1.5 rounded-[7px] border border-border-translucent bg-warm-panel p-2.5">
        <h4 className="m-0 text-[13px] text-ink-strong">安装边界</h4>
        <p className="m-0 text-xs leading-[1.55] text-ink-muted">安装只复制 Skill 包；脚本必须声明 orange-runtime.yaml，并在进阶权限下审批后隔离执行。</p>
      </section>
      <div className="flex min-w-0 flex-wrap justify-end gap-2">
        <Button variant="ghost" onClick={onCancel} disabled={isBusy}>
          取消
        </Button>
        <Button variant="primary" size="compact" type="submit" disabled={isBusy || (draft.sourceType === "url" && !draft.source.trim())}>
          <Download size={14} />
          安装 Skill
        </Button>
      </div>
    </form>
  );
}

/** 把安装来源类型转为用户可读标签。 */
function installSourceLabel(sourceType: SkillInstallSourceType) {
  const labels: Record<SkillInstallSourceType, string> = {
    url: "URL",
    localFolder: "文件夹",
    localArchive: "ZIP",
  };

  return labels[sourceType];
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
    <form className="grid gap-3.5" onSubmit={onSubmit}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className={sectionLabelClassName}>User Skill</p>
          <h3 className="mt-1 mb-0 text-xl leading-tight text-ink-strong">{draft.id ? "编辑 Skill 文件" : "新建 Skill 文件"}</h3>
        </div>
      </div>
      <label className={fieldLabelClassName}>
        <span>显示名称</span>
        <input className={fieldControlClassName} value={draft.displayName} onChange={(event) => updateDraft("displayName", event.target.value)} />
      </label>
      <label className={fieldLabelClassName}>
        <span>标识 name</span>
        <input className={fieldControlClassName} value={draft.name} onChange={(event) => updateDraft("name", event.target.value)} placeholder="my-custom-skill" />
      </label>
      <label className={fieldLabelClassName}>
        <span>描述</span>
        <input className={fieldControlClassName} value={draft.description} onChange={(event) => updateDraft("description", event.target.value)} />
      </label>
      <label className={fieldLabelClassName}>
        <span>执行说明</span>
        <textarea className={cn(fieldTextareaClassName, "min-h-[140px]")} value={draft.instructions} onChange={(event) => updateDraft("instructions", event.target.value)} />
      </label>
      <label className={fieldLabelClassName}>
        <span>标签</span>
        <input className={fieldControlClassName} value={draft.tagsText} onChange={(event) => updateDraft("tagsText", event.target.value)} placeholder="写作, 研究" />
      </label>
      <div className="flex flex-wrap items-center gap-2">
        <ToggleRow checked={draft.enabled} onChange={(checked) => updateDraft("enabled", checked)}>
          启用
        </ToggleRow>
      </div>
      <div className="flex min-w-0 flex-wrap justify-end gap-2">
        <Button variant="ghost" onClick={onCancel} disabled={isBusy}>
          取消
        </Button>
        <Button
          variant="primary"
          size="compact"
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
        </Button>
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
