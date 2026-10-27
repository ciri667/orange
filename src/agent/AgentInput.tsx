import { ArrowRight, BrainCircuit, FileText, Image, ImagePlus, Sparkles, Square } from "lucide-react";
import { VoiceDictationButton } from "../shared/VoiceDictationButton";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEventHandler,
  type ClipboardEventHandler,
  type CompositionEventHandler,
  type DragEventHandler,
  type KeyboardEventHandler,
} from "react";
import { Button } from "../shared/Button";
import { Chip } from "../shared/Chip";
import { cn } from "../shared/cn";
import { ImageLightbox } from "../shared/ImageLightbox";
import { logDebug } from "../shared/logger";
import { getProviderModelSelectionLabel } from "../shared/modelSelection";
import { ModelCascadeSelector } from "../shared/ModelCascadeSelector";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { useDismissable } from "../shared/useDismissable";
import type {
  AgentSecuritySettings,
  AgentSession,
  AgentSkill,
  BuiltinCommandDescriptor,
  ModelConfig,
} from "../shared/types";
import {
  fileToConversationImageDraft,
  collectImageFilesFromDataTransfer,
  MAX_CONVERSATION_IMAGES,
  revokeConversationImagePreview,
  validateConversationImageFiles,
  type ConversationImageDraft,
} from "./conversationImages";
import { AgentSecurityLevelControl, PlanModeToggle, PlanReadyBanner } from "./AgentPanelSections";
import { assistantReplyLooksLikePlan } from "../shared/planMode";

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

/** 指令候选的匹配文本；别名也参与匹配，方便用旧名或简写命中。 */
function getCommandPickerSearchText(command: BuiltinCommandDescriptor) {
  return [command.name, ...command.aliases, command.description].join(" ").toLowerCase();
}

/**
 * `/` 弹窗的统一候选：内置指令与 Skill 混排在同一个列表里。
 * 指令在前、Skill 在后，键盘高亮索引在两者之间连续移动。
 */
type SlashPickerItem =
  | { kind: "command"; key: string; command: BuiltinCommandDescriptor }
  | { kind: "skill"; key: string; skill: AgentSkill };

/** `/` 弹窗的展示分组；只用于渲染标题和切分列表，键盘仍走 slashPickerItems 的连续索引。 */
interface SlashPickerSection {
  kind: SlashPickerItem["kind"];
  title: string;
  /** 该分组第一项在 slashPickerItems 中的下标，用于把连续索引换算成组内索引。 */
  startIndex: number;
  items: SlashPickerItem[];
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
  draftImages = [],
  modelConfig,
  agentSecurity,
  turnModelSelection,
  builtinCommands = [],
  builtinCommandNotice,
  onDismissBuiltinCommandNotice,
  isBusy,
  queuedFollowUp,
  onPromptChange,
  onSelectedSkillIdsChange,
  onSelectedMentionedFileIdsChange,
  onDraftImagesChange,
  onNotice,
  onSubmitPrompt,
  onRunBuiltinCommand,
  onAbortTurn,
  onClearQueuedFollowUp,
  onTurnModelSelectionChange,
  onSecurityLevelChange,
  onPlanModeChange,
  onExecutePlan,
}: {
  activeSession: AgentSession;
  prompt: string;
  skills: AgentSkill[];
  selectedSkillIds: string[];
  /** 当前会话 scope 内可被显式引用的已索引文件。 */
  mentionedFiles?: AgentMentionFile[];
  /** 本轮临时选择的 @ 文件 ID；发送成功后由父组件清空。 */
  selectedMentionedFileIds?: string[];
  /** 尚未发送的对话图片草稿；发送成功后由父组件清空。 */
  draftImages?: ConversationImageDraft[];
  modelConfig: ModelConfig;
  agentSecurity?: AgentSecuritySettings;
  /** 本轮显式选择的 provider/model，空字符串表示跟随会话/全局默认。 */
  turnModelSelection: string;
  /** 后端注册表下发、当前端可用的内置指令；与 Skill 一起进入 `/` 弹窗。 */
  builtinCommands?: BuiltinCommandDescriptor[];
  isBusy: boolean;
  /** 当前回合结束后才会发给模型的下一条用户指令。 */
  queuedFollowUp?: string | null;
  onPromptChange: (value: string) => void;
  onSelectedSkillIdsChange: (skillIds: string[]) => void;
  onSelectedMentionedFileIdsChange?: (fileIds: string[]) => void;
  onDraftImagesChange?: (images: ConversationImageDraft[]) => void;
  onNotice?: (message: string) => void;
  onSubmitPrompt: () => void;
  /** 在 `/` 弹窗中选中指令并回车时调用；由工作台负责分派到本地指令执行。 */
  onRunBuiltinCommand?: (command: BuiltinCommandDescriptor) => void;
  /** 上一条内置指令的结果回执；贴近输入框展示。 */
  builtinCommandNotice?: { text: string; tone: "info" | "error" } | null;
  /** 关闭指令回执。 */
  onDismissBuiltinCommandNotice?: () => void;
  /** 中断当前正在跑的 Agent 回合。 */
  onAbortTurn?: () => void;
  /** 取消尚未进入模型的排队指令。 */
  onClearQueuedFollowUp?: () => void;
  onTurnModelSelectionChange: (selection: string) => void;
  onSecurityLevelChange: (level: AgentSession["securityLevel"]) => void;
  /** 切换规划模式；开启后 Agent 只用只读工具先出执行计划。 */
  onPlanModeChange: (planMode: boolean) => void;
  /** 用户批准计划：关闭规划模式并自动发送执行回合。 */
  onExecutePlan: () => void;
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
  /** `/` 候选列表容器，用于把键盘高亮项滚动进可视区域。 */
  const skillPickerListRef = useRef<HTMLDivElement | null>(null);
  const imageFileInputRef = useRef<HTMLInputElement | null>(null);
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
  /** 点击待发送缩略图后打开的大图；草稿被移除后同步关掉。 */
  const [previewImage, setPreviewImage] = useState<{ src: string; alt: string } | null>(null);
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
      .slice(0, MAX_EXPLICIT_SKILLS * 2);
  }, [selectedSkillIds, skills, slashSkillQuery?.query]);
  /** 过滤后的内置指令候选；指令优先于 Skill 展示，因此单独截断份数。 */
  const selectableCommands = useMemo(() => {
    const query = slashSkillQuery?.query.trim().toLowerCase() ?? "";

    return builtinCommands.filter((command) => !query || getCommandPickerSearchText(command).includes(query));
  }, [builtinCommands, slashSkillQuery?.query]);
  /**
   * `/` 弹窗的统一候选列表：指令在前、Skill 在后。
   * 键盘只有一套高亮索引，跨两个分组连续移动，避免新增第二套导航状态。
   */
  const slashPickerItems = useMemo<SlashPickerItem[]>(
    () => [
      ...selectableCommands.map<SlashPickerItem>((command) => ({ kind: "command", key: `command:${command.id}`, command })),
      ...selectableSkills.map<SlashPickerItem>((skill) => ({ kind: "skill", key: `skill:${skill.id}`, skill })),
    ],
    [selectableCommands, selectableSkills],
  );
  /** 把统一候选列表按“指令 / 技能”切成两个带标题的分组；空分组不渲染标题。 */
  const slashPickerSections = useMemo<SlashPickerSection[]>(() => {
    const sections: SlashPickerSection[] = [];

    if (selectableCommands.length) {
      sections.push({ kind: "command", title: "指令", startIndex: 0, items: slashPickerItems.slice(0, selectableCommands.length) });
    }

    if (selectableSkills.length) {
      sections.push({
        kind: "skill",
        title: "技能",
        startIndex: selectableCommands.length,
        items: slashPickerItems.slice(selectableCommands.length),
      });
    }

    return sections;
  }, [selectableCommands.length, selectableSkills.length, slashPickerItems]);
  /** picker 最终展示条件，达到上限或当前行不再是 slash 查询时立即隐藏。 */
  const shouldShowSkillPicker = isSkillPickerOpen && selectedSkillIds.length < MAX_EXPLICIT_SKILLS && Boolean(slashSkillQuery);
  /** 候选列表可滚动，键盘移动高亮后必须把它带进可视区域，否则看不到当前选中项。 */
  useEffect(() => {
    if (!shouldShowSkillPicker) {
      return;
    }

    const container = skillPickerListRef.current;
    const activeItem = container?.querySelector<HTMLElement>(`[data-picker-index="${activeSkillPickerIndex}"]`);

    activeItem?.scrollIntoView({ block: "nearest" });
  }, [activeSkillPickerIndex, shouldShowSkillPicker]);
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

  /** 记录高亮已为其重置的查询文本，避免方向键触发的 caret 同步把高亮打回第一项。 */
  const lastSkillPickerQueryRef = useRef<string | null>(null);

  /**
   * 用户主动关掉弹窗（Esc / 点击输入区以外）时，记录当时的输入文本。
   *
   * 关闭后 `/`、`@` token 仍留在输入框里，随后的 caret 同步会判定「可以打开」并把弹窗立刻弹回来；
   * 记住关闭时的文本即可只在正文真的变了之后才允许重新自动弹出。
   */
  const dismissedPickerPromptRef = useRef<{ slash: string | null; mention: string | null }>({ slash: null, mention: null });

  /** 打开或关闭 slash picker，并记录不含 prompt/instructions 的可观测日志。 */
  function updateSkillPickerOpen(nextOpen: boolean, source: string, matchCount = slashPickerItems.length) {
    setIsSkillPickerOpen(nextOpen);
    setActiveSkillPickerIndex(0);
    lastSkillPickerQueryRef.current = null;
    logDebug("切换 slash picker。", {
      category: "frontend",
      event: "explicit_skill_picker_toggle",
      status: nextOpen ? "opened" : "closed",
      metadata: {
        source,
        enabledSkillCount,
        selectedSkillCount: selectedSkillIds.length,
        commandCount: selectableCommands.length,
        matchCount,
      },
    });
  }

  /** 根据输入框当前值和光标位置同步 slash picker；日志不包含输入正文或查询文本。 */
  function syncSkillPickerFromPrompt(value: string, cursorIndex: number, source: string) {
    const nextQuery = resolveSlashSkillQuery(value, cursorIndex);
    const canOpen = Boolean(nextQuery) && selectedSkillIds.length < MAX_EXPLICIT_SKILLS;

    // 正文一改（继续输入或删除）就解除「刚被用户关掉」的抑制，允许弹窗正常弹出。
    if (dismissedPickerPromptRef.current.slash !== null && dismissedPickerPromptRef.current.slash !== value) {
      dismissedPickerPromptRef.current.slash = null;
    }

    const isDismissed = dismissedPickerPromptRef.current.slash === value;

    if (canOpen && !isDismissed && !isSkillPickerOpen) {
      updateSkillPickerOpen(true, source);
      lastSkillPickerQueryRef.current = nextQuery?.query ?? "";
      return;
    }

    if (!canOpen && isSkillPickerOpen) {
      updateSkillPickerOpen(false, source);
      return;
    }

    if (canOpen) {
      // 只有查询文本真的变了才重置高亮；方向键只移动光标，不应把选中项弹回第一项。
      const nextQueryText = nextQuery?.query ?? "";

      if (lastSkillPickerQueryRef.current !== nextQueryText) {
        lastSkillPickerQueryRef.current = nextQueryText;
        setActiveSkillPickerIndex(0);
      }
    }
  }

  /** 切换 @ 文件 picker；日志只保留数量和触发来源，避免暴露文件名与用户输入。 */
  function updateMentionFilePickerOpen(nextOpen: boolean, source: string, matchCount = selectableMentionFiles.length) {
    setIsMentionFilePickerOpen(nextOpen);
    setActiveMentionFilePickerIndex(0);
    lastMentionFileQueryRef.current = null;
    logDebug("切换 @ 文件 picker。", {
      category: "frontend",
      event: "agent_mentioned_file_picker_toggle",
      status: nextOpen ? "opened" : "closed",
      metadata: { source, selectedFileCount: selectedMentionedFileIds.length, matchCount },
    });
  }

  /** 记录高亮已为其重置的 @ 查询文本，避免方向键触发的 caret 同步把高亮打回第一项。 */
  const lastMentionFileQueryRef = useRef<string | null>(null);

  /** 随输入与光标同步 @ picker；slash Skill 和 @ 文件只能有一个处于打开状态。 */
  function syncMentionFilePickerFromPrompt(value: string, cursorIndex: number, source: string) {
    const nextQuery = resolveMentionFileQuery(value, cursorIndex);
    const canOpen = Boolean(nextQuery) && selectedMentionedFileIds.length < MAX_MENTIONED_FILES;

    // 与 `/` 弹窗一致：正文变化后解除抑制，光标同步不再把弹窗立刻弹回来。
    if (dismissedPickerPromptRef.current.mention !== null && dismissedPickerPromptRef.current.mention !== value) {
      dismissedPickerPromptRef.current.mention = null;
    }

    const isDismissed = dismissedPickerPromptRef.current.mention === value;

    if (canOpen && !isDismissed && !isMentionFilePickerOpen) {
      setIsSkillPickerOpen(false);
      updateMentionFilePickerOpen(true, source);
      lastMentionFileQueryRef.current = nextQuery?.query ?? "";
      return;
    }

    if (!canOpen && isMentionFilePickerOpen) {
      updateMentionFilePickerOpen(false, source);
      return;
    }

    if (canOpen) {
      // 只有查询文本真的变了才重置高亮；方向键只移动光标，不应把选中项弹回第一项。
      const nextQueryText = nextQuery?.query ?? "";

      if (lastMentionFileQueryRef.current !== nextQueryText) {
        lastMentionFileQueryRef.current = nextQueryText;
        setActiveMentionFilePickerIndex(0);
      }
    }
  }

  /**
   * 关闭 `/` 与 `@` 弹窗（Esc 或点击输入区以外）。
   *
   * 两个弹窗都是无遮罩浮层，此前只能靠「选中候选」或删掉 `/`、`@` token 关闭，
   * 焦点不在输入框时按 Esc 也收不掉；这里统一收口，并记住关闭时的输入文本（见 dismissedPickerPromptRef）。
   */
  function dismissOpenPickers(source: string) {
    if (isSkillPickerOpen) {
      dismissedPickerPromptRef.current.slash = prompt;
      updateSkillPickerOpen(false, source);
    }

    if (isMentionFilePickerOpen) {
      dismissedPickerPromptRef.current.mention = prompt;
      updateMentionFilePickerOpen(false, source);
    }
  }

  /** 输入区外壳既是弹窗的定位容器，也是「点击外部关闭 + Esc 关闭」的判定边界。 */
  const composerRef = useDismissable<HTMLElement>(
    shouldShowSkillPicker || shouldShowMentionFilePicker,
    () => dismissOpenPickers("dismiss"),
  );

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

  /**
   * 在 `/` 弹窗中选中内置指令：清空输入框并把整条命令行交给父组件执行。
   *
   * 仍以文本形式回传完整命令，使「弹窗选中」与「手输指令」走同一条分派路径。
   */
  function handleRunBuiltinCommand(command: BuiltinCommandDescriptor) {
    const queryState = resolveSlashSkillQuery(prompt, promptTextareaRef.current?.selectionStart ?? prompt.length);
    const line = `/${command.name}${command.argumentHint ? " " : ""}`;

    if (queryState) {
      const beforeSlash = prompt.slice(0, queryState.lineStart);
      const afterSlash = prompt.slice(queryState.cursorIndex);
      onPromptChange(`${beforeSlash}${line}${afterSlash}`);
    }

    updateSkillPickerOpen(false, "select_command", selectableCommands.length);
    onRunBuiltinCommand?.(command);
    logDebug("选择内置指令。", {
      category: "frontend",
      event: "builtin_command_select",
      status: "dispatched",
      metadata: { command: command.id },
    });
  }

  /** 把统一候选列表当前高亮项交给对应的选择逻辑。 */
  function selectSlashPickerItem(item: SlashPickerItem) {
    if (item.kind === "command") {
      handleRunBuiltinCommand(item.command);
    } else {
      handleSelectExplicitSkill(item.skill);
    }
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
      setActiveSkillPickerIndex((currentIndex) => (slashPickerItems.length ? (currentIndex + 1) % slashPickerItems.length : 0));
      return;
    }

    if (shouldShowSkillPicker && event.key === "ArrowUp") {
      event.preventDefault();
      setActiveSkillPickerIndex((currentIndex) =>
        slashPickerItems.length ? (currentIndex - 1 + slashPickerItems.length) % slashPickerItems.length : 0,
      );
      return;
    }

    // 弹窗可见时 Esc 已由 useDismissable 的捕获监听收掉；这里兜住「状态仍标记为打开但未展示」的边缘情况。
    if ((isSkillPickerOpen || isMentionFilePickerOpen) && event.key === "Escape") {
      event.preventDefault();
      dismissOpenPickers("escape");
      return;
    }

    if (event.key === "Escape" && isBusy) {
      event.preventDefault();
      onAbortTurn?.();
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

      if (slashPickerItems.length) {
        selectSlashPickerItem(slashPickerItems[Math.min(activeSkillPickerIndex, slashPickerItems.length - 1)]);
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
    if (!promptLength && !draftImages.length) {
      return;
    }

    onSubmitPrompt();
  };

  async function appendDraftImages(files: File[]) {
    const { accepted, error } = validateConversationImageFiles(files, draftImages.length);
    if (error) {
      onNotice?.(error);
      return;
    }
    if (!accepted.length || !onDraftImagesChange) {
      return;
    }

    try {
      const nextDrafts = await Promise.all(accepted.map((file) => fileToConversationImageDraft(file)));
      onDraftImagesChange([...draftImages, ...nextDrafts]);
    } catch (error) {
      onNotice?.(error instanceof Error ? error.message : "无法读取图片。");
    }
  }

  function handleRemoveDraftImage(localId: string) {
    const removed = draftImages.find((image) => image.localId === localId);
    if (removed) {
      revokeConversationImagePreview(removed);
      if (previewImage?.src === removed.previewUrl) {
        setPreviewImage(null);
      }
    }
    onDraftImagesChange?.(draftImages.filter((image) => image.localId !== localId));
  }

  const handlePromptPaste: ClipboardEventHandler<HTMLTextAreaElement> = (event) => {
    const files = collectImageFilesFromDataTransfer(event.clipboardData);
    if (!files.length) {
      return;
    }
    event.preventDefault();
    void appendDraftImages(files);
  };

  const handleComposerDragOver: DragEventHandler<HTMLElement> = (event) => {
    if (!collectImageFilesFromDataTransfer(event.dataTransfer).length) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
  };

  const handleComposerDrop: DragEventHandler<HTMLElement> = (event) => {
    const files = collectImageFilesFromDataTransfer(event.dataTransfer);
    if (!files.length) {
      return;
    }
    event.preventDefault();
    void appendDraftImages(files);
  };

  const hasComposerChips = selectedExplicitSkillChips.length > 0 || selectedMentionFileChips.length > 0 || draftImages.length > 0;
  /** 权限设置缺失时仍展示基础档，避免输入条因旧设置结构崩溃。 */
  const resolvedAgentSecurity = agentSecurity ?? {
    defaultLevel: "workspace-readonly" as const,
    resourceLimits: { timeoutSeconds: 120 },
  };

  const promptPlaceholder = queuedFollowUp
    ? "已有一条排队指令，当前回合结束后发送"
    : isBusy
      ? "Enter 排队到当前回合之后；Esc 停止生成"
      : "写下想法，或用 @ 文件、/ 指令";
  const sendTitle = queuedFollowUp ? "已有一条排队指令" : isBusy ? "排队到当前回合之后发送" : "发送";
  const showStopButton = isBusy;
  /** 规划模式开启、且最后一条回复确实是「执行计划」形状时才出横幅；问候、通用问答与澄清追问不满足形状，不打扰。 */
  const showPlanReadyBanner =
    activeSession.planMode && !isBusy && assistantReplyLooksLikePlan(activeSession.messages[activeSession.messages.length - 1]);

  return (
    <footer
      className="agent-composer flex min-w-0 shrink-0 flex-col gap-2 rounded-panel border border-border-strong bg-surface p-3"
      ref={composerRef}
      onDragOver={handleComposerDragOver}
      onDrop={handleComposerDrop}
    >
      {showPlanReadyBanner ? (
        <PlanReadyBanner onExecutePlan={onExecutePlan} onRefinePlan={() => promptTextareaRef.current?.focus()} />
      ) : null}
      {builtinCommandNotice ? (
        <div className="flex min-w-0 items-start gap-2 px-0.5 pt-0.5" role="status" aria-label="内置指令回执">
          <div
            className={cn(
              "min-w-0 flex-1 whitespace-pre-wrap rounded-control border border-border bg-surface-muted px-2.5 py-1.5 text-xs leading-[1.5] text-ink-muted",
              builtinCommandNotice.tone === "error" && "border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft text-danger",
            )}
          >
            {builtinCommandNotice.text}
          </div>
          <button
            className="shrink-0 cursor-pointer border-0 bg-transparent px-1 text-xs text-ink-muted hover:text-ink"
            type="button"
            onClick={onDismissBuiltinCommandNotice}
            aria-label="关闭指令回执"
          >
            关闭
          </button>
        </div>
      ) : null}
      {queuedFollowUp ? (
        <div className="flex min-w-0 items-center gap-2 px-0.5 pt-0.5" role="status" aria-label="输入区的排队指令">
          <Chip className="max-w-full flex-1" onRemove={onClearQueuedFollowUp} removeLabel="取消排队指令">
            <span className="shrink-0">排队中</span>
            <OverflowTooltipText className="min-w-0" text={queuedFollowUp} logArea="agent_queued_follow_up" />
          </Chip>
        </div>
      ) : null}
      {hasComposerChips && (
        <div className="flex min-w-0 items-center justify-start gap-2 px-0.5 pt-0.5 max-[760px]:items-start">
          <div className="inline-flex min-w-0 items-center gap-[7px] max-[760px]:flex-wrap">
            {selectedExplicitSkillChips.length > 0 && (
              <div className="inline-flex min-w-0 max-w-full items-center gap-1.5 overflow-hidden" aria-label="本轮显式激活 Skills">
                {selectedExplicitSkillChips.map((skill) => (
                  <Chip
                    key={skill.id}
                    missing={skill.source === "unknown"}
                    removeLabel={`移除 ${skill.displayName}`}
                    onRemove={() => handleRemoveExplicitSkill(skill.id)}
                  >
                    <Sparkles size={12} />
                    <OverflowTooltipText text={skill.displayName} logArea="agent_selected_skill_chip" />
                  </Chip>
                ))}
              </div>
            )}
            {selectedMentionFileChips.length > 0 && (
              <div className="inline-flex min-w-0 max-w-full items-center gap-1.5 overflow-hidden" aria-label="本轮 @ 文件">
                {selectedMentionFileChips.map((file) => (
                  <Chip
                    className="max-w-[170px]"
                    key={file.id}
                    missing={!file.relativePath}
                    removeLabel={`移除 ${file.displayName}`}
                    onRemove={() => handleRemoveMentionFile(file.id)}
                  >
                    {file.kind === "image" ? <Image size={12} /> : <FileText size={12} />}
                    <OverflowTooltipText text={file.displayName} logArea="agent_mentioned_file_chip" />
                  </Chip>
                ))}
              </div>
            )}
            {draftImages.length > 0 && (
              <div className="inline-flex min-w-0 max-w-full items-center gap-1.5 overflow-x-auto" aria-label="待发送图片">
                {draftImages.map((image) => (
                  <span key={image.localId} className="relative inline-flex h-12 w-12 shrink-0">
                    <button
                      type="button"
                      className="inline-flex h-12 w-12 cursor-zoom-in overflow-hidden rounded-md border border-primary-border bg-transparent p-0"
                      aria-label={`放大查看 ${image.name || "图片"}`}
                      title="点击放大查看"
                      onClick={() => setPreviewImage({ src: image.previewUrl, alt: image.name || "图片" })}
                    >
                      <img src={image.previewUrl} alt="" className="h-full w-full object-cover" />
                    </button>
                    <button
                      type="button"
                      className="absolute top-0.5 right-0.5 inline-grid size-4 place-items-center rounded-full border-0 bg-ink/70 text-[10px] leading-none text-white"
                      aria-label={`移除 ${image.name || "图片"}`}
                      onClick={() => handleRemoveDraftImage(image.localId)}
                    >
                      {"×"}
                    </button>
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      )}
      <div className="relative min-w-0">
        {shouldShowMentionFilePicker && (
          <div
            className="absolute right-0 bottom-[calc(100%+8px)] left-0 z-[6] grid max-h-[252px] gap-1 overflow-auto rounded-xl border border-border bg-surface p-1.5 shadow-app"
            role="listbox"
            aria-label="选择本轮 @ 文件"
          >
            {selectableMentionFiles.length ? (
              selectableMentionFiles.map((file, index) => (
                <button
                  className={cn(
                    "grid min-w-0 cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] rounded-md border border-transparent bg-transparent p-2 text-left text-ink",
                    index === activeMentionFilePickerIndex && "border-border-strong bg-surface",
                  )}
                  key={file.id}
                  type="button"
                  role="option"
                  aria-selected={index === activeMentionFilePickerIndex}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => handleSelectMentionFile(file)}
                >
                  {file.kind === "image" ? <Image size={15} className="text-ink-muted" /> : <FileText size={15} className="text-ink-muted" />}
                  <span className="grid min-w-0 gap-0.5">
                    <OverflowTooltipText as="strong" className="min-w-0 truncate" text={file.displayName} logArea="agent_mentioned_file_picker_name" />
                    <OverflowTooltipText as="small" className="min-w-0 truncate text-xs text-ink-muted" text={file.relativePath} logArea="agent_mentioned_file_picker_path" />
                  </span>
                  {file.kind && <em className="text-xs not-italic font-bold text-ink-muted">{file.kind}</em>}
                </button>
              ))
            ) : (
              <div className="p-2.5 text-center text-xs text-ink-muted">没有匹配的已授权文件</div>
            )}
          </div>
        )}
        {shouldShowSkillPicker && (
          <div
            className="absolute right-0 bottom-[calc(100%+8px)] left-0 z-[6] grid max-h-[252px] gap-1.5 overflow-auto rounded-xl border border-border bg-surface p-1.5 shadow-app"
            role="listbox"
            aria-label="选择内置指令或本轮显式 Skill"
            ref={skillPickerListRef}
          >
            {slashPickerSections.length ? (
              slashPickerSections.map((section) => (
                <div className="grid gap-0.5" key={section.kind} role="group" aria-label={section.title}>
                  {/* 分组标题比操作名更小更浅，只做分区提示，不抢候选的视觉权重。 */}
                  <div className="px-2 pt-1 pb-0.5 text-[11px] font-medium tracking-[0.02em] text-ink-soft">{section.title}</div>
                  {section.items.map((item, itemIndex) => {
                    const index = section.startIndex + itemIndex;

                    return (
                      <button
                        className={cn(
                          "flex min-w-0 cursor-pointer items-center justify-between gap-2.5 rounded-md border border-transparent bg-transparent px-2 py-1.5 text-left text-ink",
                          index === activeSkillPickerIndex && "border-border-strong bg-surface",
                        )}
                        data-picker-index={index}
                        key={item.key}
                        type="button"
                        role="option"
                        aria-selected={index === activeSkillPickerIndex}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => selectSlashPickerItem(item)}
                      >
                        {item.kind === "command" ? (
                          <>
                            <span className="grid min-w-0 gap-0.5">
                              <OverflowTooltipText
                                as="strong"
                                className="min-w-0 truncate text-ui font-semibold"
                                text={`/${item.command.name}${item.command.argumentHint ? ` ${item.command.argumentHint}` : ""}`}
                                logArea="agent_command_picker_name"
                              />
                              <OverflowTooltipText
                                as="small"
                                className="min-w-0 truncate text-[11px] text-ink-muted"
                                text={item.command.description}
                                logArea="agent_command_picker_description"
                              />
                            </span>
                            <em className="shrink-0 text-[11px] not-italic font-medium text-ink-soft">指令</em>
                          </>
                        ) : (
                          <>
                            <span className="grid min-w-0 gap-0.5">
                              <OverflowTooltipText
                                as="strong"
                                className="min-w-0 truncate text-ui font-semibold"
                                text={item.skill.displayName}
                                logArea="agent_skill_picker_name"
                              />
                              <OverflowTooltipText
                                as="small"
                                className="min-w-0 truncate text-[11px] text-ink-muted"
                                text={item.skill.name}
                                logArea="agent_skill_picker_id"
                              />
                            </span>
                            <em className="shrink-0 text-[11px] not-italic font-medium text-ink-soft">
                              {item.skill.source === "built-in" ? "内置" : "自定义"}
                            </em>
                          </>
                        )}
                      </button>
                    );
                  })}
                </div>
              ))
            ) : (
              <div className="p-2.5 text-center text-xs text-ink-muted">没有匹配的指令或 Skill</div>
            )}
          </div>
        )}
        <textarea
          className="min-h-14 w-full resize-none rounded-control border-0 bg-transparent p-1.5 pt-1.5 text-ink leading-normal outline-none focus-visible:outline-none"
          ref={promptTextareaRef}
          value={prompt}
          onChange={handlePromptChange}
          onClick={handlePromptCaretChange}
          onKeyUp={handlePromptCaretChange}
          onFocus={handlePromptCaretChange}
          onCompositionStart={handlePromptCompositionStart}
          onCompositionEnd={handlePromptCompositionEnd}
          onKeyDown={handlePromptKeyDown}
          onPaste={handlePromptPaste}
          placeholder={promptPlaceholder}
          aria-label="Agent 输入"
        />
      </div>
      {/* 模型名独占一行，长 Provider / Model 不再挤压权限、计划和发送入口。 */}
      {modelConfig.enabled && enabledProviders.length > 0 && (
        <div className="agent-composer-model" aria-label="本轮使用的模型">
          <BrainCircuit size={14} className="shrink-0" />
          <ModelCascadeSelector
            value={turnModelSelection}
            providers={enabledProviders}
            defaultLabel={`跟随默认${followDefaultLabel ? `（${followDefaultLabel}）` : ""}`}
            triggerPrefix=""
            ariaLabel="本轮使用的模型"
            onChange={onTurnModelSelectionChange}
            logArea="agent_turn_model_cascade"
          />
        </div>
      )}
      <div className="agent-composer-actions flex min-w-0 items-center gap-1 pt-0.5">
        <input
          ref={imageFileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          multiple
          className="hidden"
          aria-hidden="true"
          tabIndex={-1}
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? []);
            event.currentTarget.value = "";
            void appendDraftImages(files);
          }}
        />
        <Button
          variant="icon"
          size="compact"
          className="text-ink-muted hover:text-ink"
          title="添加图片"
          aria-label="添加图片"
          disabled={Boolean(queuedFollowUp) || draftImages.length >= MAX_CONVERSATION_IMAGES}
          onClick={() => imageFileInputRef.current?.click()}
        >
          <ImagePlus size={16} />
        </Button>
        <VoiceDictationButton
          getTextarea={() => promptTextareaRef.current}
          onValueChange={onPromptChange}
          onNotice={onNotice}
          disabled={Boolean(queuedFollowUp)}
        />
        {!activeSession.imIdentity && !activeSession.scheduleIdentity && (
          <AgentSecurityLevelControl
            activeSession={activeSession}
            agentSecurity={resolvedAgentSecurity}
            isBusy={isBusy}
            onSecurityLevelChange={onSecurityLevelChange}
          />
        )}
        {!activeSession.imIdentity && !activeSession.scheduleIdentity && (
          <PlanModeToggle activeSession={activeSession} isBusy={isBusy} onPlanModeChange={onPlanModeChange} />
        )}
        <div className="ml-auto flex min-w-0 items-center justify-end gap-1.5">
          {showStopButton ? (
            <Button
              variant="primary"
              size="compact"
              tone="danger"
              className="inline-grid size-8 min-h-8 min-w-8 shrink-0 place-items-center rounded-full border-transparent bg-danger p-0 hover:enabled:bg-danger disabled:opacity-[0.38]"
              title="停止生成"
              aria-label="停止生成"
              onClick={onAbortTurn}
            >
              <Square size={12} fill="currentColor" />
            </Button>
          ) : (
            <Button
              variant="primary"
              size="compact"
              className="inline-grid size-8 min-h-8 min-w-8 shrink-0 place-items-center rounded-full border-border-strong bg-surface p-0 text-ink-strong hover:enabled:border-ink-muted disabled:opacity-[0.38]"
              title={sendTitle}
              aria-label={sendTitle}
              onClick={onSubmitPrompt}
              disabled={(!prompt.trim() && !draftImages.length) || Boolean(queuedFollowUp)}
            >
              <ArrowRight size={16} />
            </Button>
          )}
        </div>
      </div>
      {previewImage ? (
        <ImageLightbox src={previewImage.src} alt={previewImage.alt} onClose={() => setPreviewImage(null)} />
      ) : null}
    </footer>
  );
}
