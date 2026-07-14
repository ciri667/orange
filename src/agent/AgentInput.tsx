import { ArrowRight, BrainCircuit, FileText, Image, Sparkles, X } from "lucide-react";
import {
  useMemo,
  useRef,
  useState,
  type ChangeEventHandler,
  type CompositionEventHandler,
  type KeyboardEventHandler,
} from "react";
import { logDebug } from "../shared/logger";
import {
  FOLLOW_DEFAULT_MODEL_SELECTION,
  getProviderModelSelectionLabel,
} from "../shared/modelSelection";
import { ModelCascadeSelector } from "../shared/ModelCascadeSelector";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { AgentSession, AgentSkill, ModelConfig } from "../shared/types";

/** 兼容旧组件导入的“跟随默认”占位值，实际定义集中在 shared/modelSelection。 */
export const FOLLOW_DEFAULT_VALUE = FOLLOW_DEFAULT_MODEL_SELECTION;
/** 输入法结束组词后的短保护窗口；部分中文输入法会先触发 compositionend，再派发 Enter keydown。 */
const PROMPT_IME_ENTER_GUARD_MS = 150;
/** v1 每轮最多显式激活 3 个 Skill，避免用户误选过多 instructions 挤占上下文。 */
const MAX_EXPLICIT_SKILLS = 3;
/** v1 单轮最多引用 8 个文件，和后端上下文预算上限保持一致。 */
const MAX_MENTIONED_FILES = 8;

/** 当前光标所在行的 slash skill 查询状态，只接受行首 `/query`，避免普通正文中的斜杠误触发。 */
interface SlashSkillQuery {
  lineStart: number;
  cursorIndex: number;
  query: string;
}

/** 已选 Skill chip 的展示数据；skill 缺失时仍保留 ID，方便用户删除并交给后端最终校验。 */
interface SelectedSkillChip {
  id: string;
  displayName: string;
  source: AgentSkill["source"] | "unknown";
}

/**
 * 可 @ 文件的公开展示信息。
 *
 * 候选由工作台依据会话知识库 scope 提供；输入组件不读取文件内容，也不自行访问本机路径。
 */
export interface AgentMentionFile {
  id: string;
  displayName: string;
  /** 相对知识库的路径，用于同名文件消歧。 */
  relativePath: string;
  /** 可选的文件类型标签，例如 markdown、image、pdf。 */
  kind?: "markdown" | "text" | "pdf" | "docx" | "image" | string;
}

/** 当前光标所在行的 @ 文件查询状态，只接受连续的 `@query` token。 */
interface MentionFileQuery {
  tokenStart: number;
  cursorIndex: number;
  query: string;
}

/** 从输入框内容和光标位置解析 slash skill 查询，不返回用户 prompt 正文给日志。 */
function resolveSlashSkillQuery(value: string, cursorIndex: number): SlashSkillQuery | null {
  const safeCursorIndex = Math.max(0, Math.min(cursorIndex, value.length));
  const lineStart = value.lastIndexOf("\n", safeCursorIndex - 1) + 1;
  const linePrefix = value.slice(lineStart, safeCursorIndex);

  if (!linePrefix.startsWith("/")) {
    return null;
  }

  const query = linePrefix.slice(1);

  // slash 命令只匹配当前行的第一个连续 token；出现空白后说明用户已进入普通正文。
  if (/\s/.test(query)) {
    return null;
  }

  return { lineStart, cursorIndex: safeCursorIndex, query };
}

/** 解析光标前的 @ 文件 token；空查询 `@` 也会打开选择器。 */
function resolveMentionFileQuery(value: string, cursorIndex: number): MentionFileQuery | null {
  const safeCursorIndex = Math.max(0, Math.min(cursorIndex, value.length));
  const prefix = value.slice(0, safeCursorIndex);
  const tokenStart = prefix.lastIndexOf("@");

  if (tokenStart < 0) {
    return null;
  }

  // @ 前只能是行首或空白，避免邮件地址和普通单词中的 @ 误触发。
  const precedingCharacter = value[tokenStart - 1];
  const query = value.slice(tokenStart + 1, safeCursorIndex);

  if ((precedingCharacter && !/\s/.test(precedingCharacter)) || /\s/.test(query)) {
    return null;
  }

  return { tokenStart, cursorIndex: safeCursorIndex, query };
}

/** 为前端 picker 构造只含公开元数据的匹配文本，避免读取或记录 skill instructions。 */
function getSkillPickerSearchText(skill: AgentSkill) {
  return [skill.displayName, skill.name, skill.description, ...skill.tags].join(" ").toLowerCase();
}

/** 候选搜索只使用已授权文件的公开元数据，禁止将内容写入前端日志。 */
function getMentionFileSearchText(file: AgentMentionFile) {
  return [file.displayName, file.relativePath, file.kind].filter(Boolean).join(" ").toLowerCase();
}

/** Agent 底部输入区，封装输入法保护、本轮模型选择和 slash Skill picker。 */
export function AgentInput({
  activeSession,
  prompt,
  skills,
  selectedSkillIds,
  mentionedFiles = [],
  selectedMentionedFileIds = [],
  modelConfig,
  turnModelSelection,
  isBusy,
  onPromptChange,
  onSelectedSkillIdsChange,
  onSelectedMentionedFileIdsChange,
  onSubmitPrompt,
  onTurnModelSelectionChange,
}: {
  activeSession: AgentSession;
  prompt: string;
  skills: AgentSkill[];
  selectedSkillIds: string[];
  /** 当前会话 scope 内可被显式引用的已索引文件。 */
  mentionedFiles?: AgentMentionFile[];
  /** 本轮临时选择的 @ 文件 ID；发送成功后由父组件清空。 */
  selectedMentionedFileIds?: string[];
  modelConfig: ModelConfig;
  /** 本轮显式选择的 provider/model，空字符串表示跟随会话/全局默认。 */
  turnModelSelection: string;
  isBusy: boolean;
  onPromptChange: (value: string) => void;
  onSelectedSkillIdsChange: (skillIds: string[]) => void;
  onSelectedMentionedFileIdsChange?: (fileIds: string[]) => void;
  onSubmitPrompt: () => void;
  onTurnModelSelectionChange: (selection: string) => void;
}) {
  /** 已启用的 Provider 列表；未启用的 provider 不出现在选择器中。 */
  const enabledProviders = modelConfig.providers.filter((provider) => provider.enabled);
  /** 全局默认 provider 名称，用于“跟随默认”选项的说明文案。 */
  const defaultProvider = modelConfig.providers.find((provider) => provider.id === modelConfig.defaultProviderId);
  /** 会话当前设置的默认 provider（可能未设置，回退到全局默认）。 */
  const sessionProvider = activeSession.modelProviderId
    ? modelConfig.providers.find((provider) => provider.id === activeSession.modelProviderId)
    : undefined;
  /** 跟随默认选项展示 provider/model，而不是只展示 provider。 */
  const followDefaultLabel = sessionProvider
    ? getProviderModelSelectionLabel(sessionProvider, activeSession.modelId || sessionProvider.model)
    : defaultProvider
      ? getProviderModelSelectionLabel(defaultProvider)
      : "";
  /** 已启用 skill 会以名称和描述进入 system prompt，具体是否使用交给 Agent 判断。 */
  const enabledSkillCount = skills.filter((skill) => skill.enabled).length;
  /** 当前 Agent 输入框是否处于输入法组词状态，弥补不同浏览器 nativeEvent.isComposing 不一致的问题。 */
  const isPromptComposingRef = useRef(false);
  /** Agent textarea 引用，用于读取光标位置并在选择 skill 后恢复焦点。 */
  const promptTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  /** 最近一次输入法组词结束时间，用于过滤 compositionend 后紧邻的候选确认 Enter。 */
  const lastPromptCompositionEndAtRef = useRef(0);
  /** 记录已在组词阶段捕获到 Enter，避免常规事件顺序下过度拦截用户后续发送。 */
  const didHandlePromptComposingEnterRef = useRef(false);
  /** slash picker 是否打开；是否展示还会同时受当前行查询和选择上限约束。 */
  const [isSkillPickerOpen, setIsSkillPickerOpen] = useState(false);
  /** slash picker 当前键盘高亮项索引，用于上下键选择。 */
  const [activeSkillPickerIndex, setActiveSkillPickerIndex] = useState(0);
  /** @ 文件 picker 是否打开。 */
  const [isMentionFilePickerOpen, setIsMentionFilePickerOpen] = useState(false);
  /** @ 文件 picker 的键盘高亮项。 */
  const [activeMentionFilePickerIndex, setActiveMentionFilePickerIndex] = useState(0);
  /** 已选显式 skill 的 chip 数据；缺失项不隐藏，避免用户无法移除已经失效的选择。 */
  const selectedExplicitSkillChips = useMemo(
    () =>
      selectedSkillIds.map<SelectedSkillChip>((skillId) => {
        const skill = skills.find((item) => item.id === skillId);

        return {
          id: skillId,
          displayName: skill?.displayName ?? "未知 Skill",
          source: skill?.source ?? "unknown",
        };
      }),
    [selectedSkillIds, skills],
  );
  /** 已选 @ 文件保留缺失 ID，方便用户移除并让后端作最终授权校验。 */
  const selectedMentionFileChips = useMemo(
    () => selectedMentionedFileIds.map((fileId) => mentionedFiles.find((file) => file.id === fileId) ?? {
      id: fileId,
      displayName: "已失效文件",
      relativePath: "",
    }),
    [mentionedFiles, selectedMentionedFileIds],
  );
  /** 当前光标所在行的 slash 查询，只有 `/query` 这种行首模式才会触发 picker。 */
  const slashSkillQuery = resolveSlashSkillQuery(prompt, promptTextareaRef.current?.selectionStart ?? prompt.length);
  /** 过滤后的可选 skill 列表，只包含已启用且未被选中的 skill。 */
  const selectableSkills = useMemo(() => {
    const selectedSkillIdSet = new Set(selectedSkillIds);
    const query = slashSkillQuery?.query.trim().toLowerCase() ?? "";

    return skills
      .filter((skill) => skill.enabled && !selectedSkillIdSet.has(skill.id))
      .filter((skill) => !query || getSkillPickerSearchText(skill).includes(query))
      .slice(0, 8);
  }, [selectedSkillIds, skills, slashSkillQuery?.query]);
  /** picker 最终展示条件，达到上限或当前行不再是 slash 查询时立即隐藏。 */
  const shouldShowSkillPicker = isSkillPickerOpen && selectedSkillIds.length < MAX_EXPLICIT_SKILLS && Boolean(slashSkillQuery);
  /** 当前 @ token 的检索词。 */
  const mentionFileQuery = resolveMentionFileQuery(prompt, promptTextareaRef.current?.selectionStart ?? prompt.length);
  /** @ 候选仅过滤父组件提供的已授权文件，不在输入组件扩大会话 scope。 */
  const selectableMentionFiles = useMemo(() => {
    const selectedFileIdSet = new Set(selectedMentionedFileIds);
    const query = mentionFileQuery?.query.trim().toLowerCase() ?? "";

    return mentionedFiles
      .filter((file) => !selectedFileIdSet.has(file.id))
      // 单轮“已选”数量受 MAX_MENTIONED_FILES 约束，但候选必须完整展示；
      // 否则按路径排序后，排在后面的根目录文件会永远无法被 @ 到。
      .filter((file) => !query || getMentionFileSearchText(file).includes(query));
  }, [mentionedFiles, mentionFileQuery?.query, selectedMentionedFileIds]);
  /** 文件 picker 不和 slash picker 同时展示，避免键盘焦点语义冲突。 */
  const shouldShowMentionFilePicker =
    isMentionFilePickerOpen &&
    selectedMentionedFileIds.length < MAX_MENTIONED_FILES &&
    Boolean(mentionFileQuery) &&
    !shouldShowSkillPicker;

  /** 打开或关闭 slash picker，并记录不含 prompt/instructions 的可观测日志。 */
  function updateSkillPickerOpen(nextOpen: boolean, source: string, matchCount = selectableSkills.length) {
    setIsSkillPickerOpen(nextOpen);
    setActiveSkillPickerIndex(0);
    logDebug("切换显式 Skill picker。", {
      category: "frontend",
      event: "explicit_skill_picker_toggle",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        source,
        enabledSkillCount,
        selectedSkillCount: selectedSkillIds.length,
        matchCount,
      },
    });
  }

  /** 根据输入框当前值和光标位置同步 slash picker；日志不包含输入正文或查询文本。 */
  function syncSkillPickerFromPrompt(value: string, cursorIndex: number, source: string) {
    const nextQuery = resolveSlashSkillQuery(value, cursorIndex);
    const canOpen = Boolean(nextQuery) && selectedSkillIds.length < MAX_EXPLICIT_SKILLS;

    if (canOpen && !isSkillPickerOpen) {
      updateSkillPickerOpen(true, source);
      return;
    }

    if (!canOpen && isSkillPickerOpen) {
      updateSkillPickerOpen(false, source);
      return;
    }

    if (canOpen) {
      setActiveSkillPickerIndex(0);
    }
  }

  /** 切换 @ 文件 picker；日志只保留数量和触发来源，避免暴露文件名与用户输入。 */
  function updateMentionFilePickerOpen(nextOpen: boolean, source: string, matchCount = selectableMentionFiles.length) {
    setIsMentionFilePickerOpen(nextOpen);
    setActiveMentionFilePickerIndex(0);
    logDebug("切换 @ 文件 picker。", {
      category: "frontend",
      event: "agent_mentioned_file_picker_toggle",
      status: nextOpen ? "opened" : "closed",
      metadata: { source, selectedFileCount: selectedMentionedFileIds.length, matchCount },
    });
  }

  /** 随输入与光标同步 @ picker；slash Skill 和 @ 文件只能有一个处于打开状态。 */
  function syncMentionFilePickerFromPrompt(value: string, cursorIndex: number, source: string) {
    const nextQuery = resolveMentionFileQuery(value, cursorIndex);
    const canOpen = Boolean(nextQuery) && selectedMentionedFileIds.length < MAX_MENTIONED_FILES;

    if (canOpen && !isMentionFilePickerOpen) {
      setIsSkillPickerOpen(false);
      updateMentionFilePickerOpen(true, source);
      return;
    }

    if (!canOpen && isMentionFilePickerOpen) {
      updateMentionFilePickerOpen(false, source);
      return;
    }

    if (canOpen) {
      setActiveMentionFilePickerIndex(0);
    }
  }

  /** Agent 输入框变更处理；只根据光标行的 slash token 打开 picker，不记录正文。 */
  const handlePromptChange: ChangeEventHandler<HTMLTextAreaElement> = (event) => {
    onPromptChange(event.target.value);
    syncSkillPickerFromPrompt(event.target.value, event.target.selectionStart, "input");
    syncMentionFilePickerFromPrompt(event.target.value, event.target.selectionStart, "input");
  };

  /** 通过鼠标或键盘选中一个显式 Skill，并从输入框移除当前 `/query` token。 */
  function handleSelectExplicitSkill(skill: AgentSkill) {
    if (selectedSkillIds.includes(skill.id) || selectedSkillIds.length >= MAX_EXPLICIT_SKILLS) {
      return;
    }

    const textarea = promptTextareaRef.current;
    const queryState = resolveSlashSkillQuery(prompt, textarea?.selectionStart ?? prompt.length);
    const nextSkillIds = [...selectedSkillIds, skill.id].slice(0, MAX_EXPLICIT_SKILLS);

    if (queryState) {
      const beforeSlash = prompt.slice(0, queryState.lineStart);
      const afterSlash = prompt.slice(queryState.cursorIndex);

      onPromptChange(`${beforeSlash}${afterSlash}`);
      requestAnimationFrame(() => {
        const nextCursorIndex = beforeSlash.length;

        promptTextareaRef.current?.focus();
        promptTextareaRef.current?.setSelectionRange(nextCursorIndex, nextCursorIndex);
      });
    } else {
      requestAnimationFrame(() => promptTextareaRef.current?.focus());
    }

    onSelectedSkillIdsChange(nextSkillIds);
    updateSkillPickerOpen(false, "select", selectableSkills.length);
    logDebug("选择显式 Skill。", {
      category: "frontend",
      event: "explicit_skill_select",
      status: "completed",
      metadata: {
        selectedSkillCount: nextSkillIds.length,
        skillSource: skill.source,
        instructionChars: skill.instructions.length,
      },
    });
  }

  /** 移除已选显式 Skill chip；只记录数量和来源，不记录 Skill 正文。 */
  function handleRemoveExplicitSkill(skillId: string) {
    const removedSkill = skills.find((skill) => skill.id === skillId);
    const nextSkillIds = selectedSkillIds.filter((selectedSkillId) => selectedSkillId !== skillId);

    onSelectedSkillIdsChange(nextSkillIds);
    logDebug("移除显式 Skill。", {
      category: "frontend",
      event: "explicit_skill_remove",
      status: "completed",
      metadata: {
        selectedSkillCount: nextSkillIds.length,
        skillSource: removedSkill?.source ?? "unknown",
      },
    });
  }

  /** 选中一个 @ 文件并移除输入框内的查询 token；实际文件内容由后端在 scope 校验后读取。 */
  function handleSelectMentionFile(file: AgentMentionFile) {
    if (!onSelectedMentionedFileIdsChange || selectedMentionedFileIds.includes(file.id) || selectedMentionedFileIds.length >= MAX_MENTIONED_FILES) {
      return;
    }

    const textarea = promptTextareaRef.current;
    const queryState = resolveMentionFileQuery(prompt, textarea?.selectionStart ?? prompt.length);
    const nextFileIds = [...selectedMentionedFileIds, file.id].slice(0, MAX_MENTIONED_FILES);

    if (queryState) {
      const beforeToken = prompt.slice(0, queryState.tokenStart);
      const afterToken = prompt.slice(queryState.cursorIndex);

      onPromptChange(`${beforeToken}${afterToken}`);
      requestAnimationFrame(() => {
        const nextCursorIndex = beforeToken.length;

        promptTextareaRef.current?.focus();
        promptTextareaRef.current?.setSelectionRange(nextCursorIndex, nextCursorIndex);
      });
    }

    onSelectedMentionedFileIdsChange(nextFileIds);
    updateMentionFilePickerOpen(false, "select", selectableMentionFiles.length);
    logDebug("选择 @ 文件。", {
      category: "frontend",
      event: "agent_mentioned_file_select",
      status: "completed",
      metadata: { selectedFileCount: nextFileIds.length, fileKind: file.kind ?? "unknown" },
    });
  }

  /** 移除本轮 @ 文件；仅记录数量与类型，不记录文件名和路径。 */
  function handleRemoveMentionFile(fileId: string) {
    const removedFile = mentionedFiles.find((file) => file.id === fileId);
    const nextFileIds = selectedMentionedFileIds.filter((selectedFileId) => selectedFileId !== fileId);

    onSelectedMentionedFileIdsChange?.(nextFileIds);
    logDebug("移除 @ 文件。", {
      category: "frontend",
      event: "agent_mentioned_file_remove",
      status: "completed",
      metadata: { selectedFileCount: nextFileIds.length, fileKind: removedFile?.kind ?? "unknown" },
    });
  }

  /** 鼠标或光标移动后重新判断 slash picker 是否仍应展示。 */
  function handlePromptCaretChange() {
    const textarea = promptTextareaRef.current;

    if (textarea) {
      syncSkillPickerFromPrompt(prompt, textarea.selectionStart, "caret");
      syncMentionFilePickerFromPrompt(prompt, textarea.selectionStart, "caret");
    }
  }

  /** Agent 输入框开始组词时只更新本地状态，不记录正文内容。 */
  const handlePromptCompositionStart: CompositionEventHandler<HTMLTextAreaElement> = () => {
    isPromptComposingRef.current = true;
    lastPromptCompositionEndAtRef.current = 0;
    didHandlePromptComposingEnterRef.current = false;
  };

  /** Agent 输入框结束组词时开启短保护窗口，兼容输入法确认键和 keydown 顺序反转的情况。 */
  const handlePromptCompositionEnd: CompositionEventHandler<HTMLTextAreaElement> = () => {
    isPromptComposingRef.current = false;
    lastPromptCompositionEndAtRef.current = didHandlePromptComposingEnterRef.current ? 0 : Date.now();
    didHandlePromptComposingEnterRef.current = false;
  };

  /** Agent 输入框快捷键处理器；Enter 提交，Shift+Enter 继续使用 textarea 原生换行。 */
  const handlePromptKeyDown: KeyboardEventHandler<HTMLTextAreaElement> = (event) => {
    if (shouldShowMentionFilePicker && event.key === "ArrowDown") {
      event.preventDefault();
      setActiveMentionFilePickerIndex((currentIndex) =>
        selectableMentionFiles.length ? (currentIndex + 1) % selectableMentionFiles.length : 0,
      );
      return;
    }

    if (shouldShowMentionFilePicker && event.key === "ArrowUp") {
      event.preventDefault();
      setActiveMentionFilePickerIndex((currentIndex) =>
        selectableMentionFiles.length ? (currentIndex - 1 + selectableMentionFiles.length) % selectableMentionFiles.length : 0,
      );
      return;
    }

    if (shouldShowSkillPicker && event.key === "ArrowDown") {
      event.preventDefault();
      setActiveSkillPickerIndex((currentIndex) => (selectableSkills.length ? (currentIndex + 1) % selectableSkills.length : 0));
      return;
    }

    if (shouldShowSkillPicker && event.key === "ArrowUp") {
      event.preventDefault();
      setActiveSkillPickerIndex((currentIndex) =>
        selectableSkills.length ? (currentIndex - 1 + selectableSkills.length) % selectableSkills.length : 0,
      );
      return;
    }

    if ((isSkillPickerOpen || isMentionFilePickerOpen) && event.key === "Escape") {
      event.preventDefault();
      updateSkillPickerOpen(false, "escape");
      updateMentionFilePickerOpen(false, "escape");
      return;
    }

    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }

    const promptLength = prompt.trim().length;
    const timeSinceCompositionEnd = Date.now() - lastPromptCompositionEndAtRef.current;
    const isRecentCompositionEnd =
      lastPromptCompositionEndAtRef.current > 0 && timeSinceCompositionEnd >= 0 && timeSinceCompositionEnd <= PROMPT_IME_ENTER_GUARD_MS;
    const isImeConfirmationEnter = isPromptComposingRef.current || event.nativeEvent.isComposing || event.keyCode === 229 || isRecentCompositionEnd;

    // 输入法组词或刚结束组词时的 Enter 只用于确认候选词，不能触发消息发送。
    if (isImeConfirmationEnter) {
      didHandlePromptComposingEnterRef.current = true;

      if (isRecentCompositionEnd) {
        // compositionend 后补发的 Enter 已完成候选确认，这里阻止 textarea 额外插入换行。
        event.preventDefault();
        lastPromptCompositionEndAtRef.current = 0;
      }

      logDebug("忽略输入法确认用 Enter。", {
        category: "frontend",
        event: "agent_prompt_enter_ime_guard",
        status: "ignored",
        metadata: {
          isNativeComposing: event.nativeEvent.isComposing,
          isTrackedComposing: isPromptComposingRef.current,
          promptLength,
        },
      });
      return;
    }

    if (shouldShowSkillPicker) {
      event.preventDefault();

      if (selectableSkills.length) {
        handleSelectExplicitSkill(selectableSkills[Math.min(activeSkillPickerIndex, selectableSkills.length - 1)]);
      } else {
        updateSkillPickerOpen(false, "empty_enter", 0);
      }

      return;
    }

    if (shouldShowMentionFilePicker) {
      event.preventDefault();

      if (selectableMentionFiles.length) {
        handleSelectMentionFile(selectableMentionFiles[Math.min(activeMentionFilePickerIndex, selectableMentionFiles.length - 1)]);
      } else {
        updateMentionFilePickerOpen(false, "empty_enter", 0);
      }

      return;
    }

    event.preventDefault();
    logDebug("通过回车快捷键提交 Agent 输入。", {
      category: "frontend",
      event: "agent_prompt_enter_submit",
      status: promptLength ? "submitted" : "ignored",
      metadata: {
        hasActivePendingChange: activeSession.pendingChange?.status === "pending",
        messageCount: activeSession.messages.length,
        promptLength,
        explicitSkillCount: selectedSkillIds.length,
        mentionedFileCount: selectedMentionedFileIds.length,
      },
    });

    // 空输入只吞掉回车，避免产生无意义空行；真正发送仍复用按钮的同一业务入口。
    if (!promptLength) {
      return;
    }

    onSubmitPrompt();
  };

  return (
    <footer className="agent-input">
      <div className="agent-input-toolbar">
        <div className="agent-input-toolbar-start">
          <div className="skill-select" aria-label="当前启用 Skills">
            <Sparkles size={14} />
            <span>Skill</span>
            <strong>{enabledSkillCount} 个已启用</strong>
          </div>
          {selectedExplicitSkillChips.length > 0 && (
            <div className="selected-skill-chips" aria-label="本轮显式激活 Skills">
              {selectedExplicitSkillChips.map((skill) => (
                <span className={`selected-skill-chip ${skill.source === "unknown" ? "missing" : ""}`} key={skill.id}>
                  <Sparkles size={12} />
                  <OverflowTooltipText text={skill.displayName} logArea="agent_selected_skill_chip" />
                  <button type="button" aria-label={`移除 ${skill.displayName}`} onClick={() => handleRemoveExplicitSkill(skill.id)}>
                    <X size={12} />
                  </button>
                </span>
              ))}
            </div>
          )}
          {selectedMentionFileChips.length > 0 && (
            <div className="selected-mention-file-chips" aria-label="本轮 @ 文件">
              {selectedMentionFileChips.map((file) => (
                <span className={`selected-mention-file-chip ${file.relativePath ? "" : "missing"}`} key={file.id}>
                  {file.kind === "image" ? <Image size={12} /> : <FileText size={12} />}
                  <OverflowTooltipText text={file.displayName} logArea="agent_mentioned_file_chip" />
                  <button type="button" aria-label={`移除 ${file.displayName}`} onClick={() => handleRemoveMentionFile(file.id)}>
                    <X size={12} />
                  </button>
                </span>
              ))}
            </div>
          )}
        </div>
        {modelConfig.enabled && enabledProviders.length > 0 && (
          <div className="turn-model-select" aria-label="本轮使用的模型">
            <BrainCircuit size={14} />
            <ModelCascadeSelector
              value={turnModelSelection}
              providers={enabledProviders}
              defaultLabel={`本轮：跟随会话默认${followDefaultLabel ? `（${followDefaultLabel}）` : ""}`}
              triggerPrefix="本轮："
              ariaLabel="本轮使用的模型"
              onChange={onTurnModelSelectionChange}
              logArea="agent_turn_model_cascade"
            />
          </div>
        )}
      </div>
      <div className="agent-input-main">
        {shouldShowMentionFilePicker && (
          <div className="mention-file-picker-popover" role="listbox" aria-label="选择本轮 @ 文件">
            {selectableMentionFiles.length ? (
              selectableMentionFiles.map((file, index) => (
                <button
                  className={`mention-file-picker-option ${index === activeMentionFilePickerIndex ? "active" : ""}`}
                  key={file.id}
                  type="button"
                  role="option"
                  aria-selected={index === activeMentionFilePickerIndex}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => handleSelectMentionFile(file)}
                >
                  {file.kind === "image" ? <Image size={15} /> : <FileText size={15} />}
                  <span>
                    <OverflowTooltipText as="strong" text={file.displayName} logArea="agent_mentioned_file_picker_name" />
                    <OverflowTooltipText as="small" text={file.relativePath} logArea="agent_mentioned_file_picker_path" />
                  </span>
                  {file.kind && <em>{file.kind}</em>}
                </button>
              ))
            ) : (
              <div className="mention-file-picker-empty">没有匹配的已授权文件</div>
            )}
          </div>
        )}
        {shouldShowSkillPicker && (
          <div className="skill-picker-popover" role="listbox" aria-label="选择本轮显式 Skill">
            {selectableSkills.length ? (
              selectableSkills.map((skill, index) => (
                <button
                  className={`skill-picker-option ${index === activeSkillPickerIndex ? "active" : ""}`}
                  key={skill.id}
                  type="button"
                  role="option"
                  aria-selected={index === activeSkillPickerIndex}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => handleSelectExplicitSkill(skill)}
                >
                  <span>
                    <OverflowTooltipText as="strong" text={skill.displayName} logArea="agent_skill_picker_name" />
                    <OverflowTooltipText as="small" text={skill.name} logArea="agent_skill_picker_id" />
                  </span>
                  <em>{skill.source === "built-in" ? "内置" : "自定义"}</em>
                </button>
              ))
            ) : (
              <div className="skill-picker-empty">没有匹配的已启用 Skill</div>
            )}
          </div>
        )}
        <textarea
          ref={promptTextareaRef}
          value={prompt}
          onChange={handlePromptChange}
          onClick={handlePromptCaretChange}
          onKeyUp={handlePromptCaretChange}
          onFocus={handlePromptCaretChange}
          onCompositionStart={handlePromptCompositionStart}
          onCompositionEnd={handlePromptCompositionEnd}
          onKeyDown={handlePromptKeyDown}
          placeholder="输入 @ 引用文件，/ 选择本轮 Skill；Agent 仍可按需检索知识库"
          aria-label="Agent 输入"
          disabled={isBusy}
        />
      </div>
      <button className="primary-button compact agent-send-button" type="button" onClick={onSubmitPrompt} disabled={isBusy}>
        <ArrowRight size={16} />
        发送
      </button>
    </footer>
  );
}
