import { ArrowRight, BrainCircuit, Sparkles, X } from "lucide-react";
import {
  useMemo,
  useRef,
  useState,
  type ChangeEventHandler,
  type CompositionEventHandler,
  type KeyboardEventHandler,
} from "react";
import { logDebug } from "../shared/logger";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { AgentSession, AgentSkill, ModelConfig } from "../shared/types";

/** 会话/本轮模型选择器统一使用的“跟随默认”占位值，不写入具体 providerId。 */
export const FOLLOW_DEFAULT_VALUE = "";
/** 输入法结束组词后的短保护窗口；部分中文输入法会先触发 compositionend，再派发 Enter keydown。 */
const PROMPT_IME_ENTER_GUARD_MS = 150;
/** v1 每轮最多显式激活 3 个 Skill，避免用户误选过多 instructions 挤占上下文。 */
const MAX_EXPLICIT_SKILLS = 3;

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

/** 为前端 picker 构造只含公开元数据的匹配文本，避免读取或记录 skill instructions。 */
function getSkillPickerSearchText(skill: AgentSkill) {
  return [skill.displayName, skill.name, skill.description, ...skill.tags].join(" ").toLowerCase();
}

/** Agent 底部输入区，封装输入法保护、本轮模型选择和 slash Skill picker。 */
export function AgentInput({
  activeSession,
  prompt,
  skills,
  selectedSkillIds,
  modelConfig,
  turnModelProviderId,
  isBusy,
  onPromptChange,
  onSelectedSkillIdsChange,
  onSubmitPrompt,
  onTurnModelProviderChange,
}: {
  activeSession: AgentSession;
  prompt: string;
  skills: AgentSkill[];
  selectedSkillIds: string[];
  modelConfig: ModelConfig;
  /** 本轮显式选择的 Provider，空字符串表示跟随会话/全局默认。 */
  turnModelProviderId: string;
  isBusy: boolean;
  onPromptChange: (value: string) => void;
  onSelectedSkillIdsChange: (skillIds: string[]) => void;
  onSubmitPrompt: () => void;
  onTurnModelProviderChange: (providerId: string) => void;
}) {
  /** 已启用的 Provider 列表；未启用的 provider 不出现在选择器中。 */
  const enabledProviders = modelConfig.providers.filter((provider) => provider.enabled);
  /** 全局默认 provider 名称，用于“跟随默认”选项的说明文案。 */
  const defaultProvider = modelConfig.providers.find((provider) => provider.id === modelConfig.defaultProviderId);
  /** 会话当前设置的默认 provider（可能未设置，回退到全局默认）。 */
  const sessionProvider = activeSession.modelProviderId
    ? modelConfig.providers.find((provider) => provider.id === activeSession.modelProviderId)
    : undefined;
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

  /** Agent 输入框变更处理；只根据光标行的 slash token 打开 picker，不记录正文。 */
  const handlePromptChange: ChangeEventHandler<HTMLTextAreaElement> = (event) => {
    onPromptChange(event.target.value);
    syncSkillPickerFromPrompt(event.target.value, event.target.selectionStart, "input");
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

  /** 鼠标或光标移动后重新判断 slash picker 是否仍应展示。 */
  function handlePromptCaretChange() {
    const textarea = promptTextareaRef.current;

    if (textarea) {
      syncSkillPickerFromPrompt(prompt, textarea.selectionStart, "caret");
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

    if (isSkillPickerOpen && event.key === "Escape") {
      event.preventDefault();
      updateSkillPickerOpen(false, "escape");
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
        </div>
        {modelConfig.enabled && enabledProviders.length > 0 && (
          <label className="turn-model-select" aria-label="本轮使用的模型">
            <BrainCircuit size={14} />
            <span className="select-control inline-select-control">
              <select value={turnModelProviderId} onChange={(event) => onTurnModelProviderChange(event.target.value)}>
                <option value={FOLLOW_DEFAULT_VALUE}>
                  本轮：跟随会话默认{sessionProvider ? `（${sessionProvider.name}）` : defaultProvider ? `（${defaultProvider.name}）` : ""}
                </option>
                {enabledProviders.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    本轮：{provider.name}
                  </option>
                ))}
              </select>
            </span>
          </label>
        )}
      </div>
      <div className="agent-input-main">
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
          placeholder="输入 / 选择本轮 Skill；需要依据本地笔记时，Agent 会自行调用工具"
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
