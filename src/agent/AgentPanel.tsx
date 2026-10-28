import { FileChangeSetReview } from "./FileChangeSetReview";
import { Book, Play, ShieldAlert, X } from "lucide-react";
import { useRef } from "react";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import { getImSessionSourceLabel, getSessionKnowledgeBaseLabel } from "../shared/selectors";
import { useDismissable } from "../shared/useDismissable";
import type {
  AgentContextCompactionReport,
  AgentSecuritySettings,
  AgentSession,
  AgentSkill,
  AgentTurnProgressEvent,
  BuiltinCommandDescriptor,
  Citation,
  KnowledgeBase,
  ModelConfig,
  Note,
  WorkspaceDocument,
} from "../shared/types";
import { AgentInput, type AgentMentionFile } from "./AgentInput";
import type { ConversationImageDraft } from "./conversationImages";
import {
  AgentMessageList,
  AgentScopeSelector,
  AgentSessionContextPopover,
  AgentSessionSummary,
} from "./AgentPanelSections";

/** Agent 停靠工作台：承载会话、工具调用、检索范围、引用和输入框。 */
export function AgentPanel({
  sessions,
  activeSession,
  messageScrollPositions,
  activeKnowledgeBase,
  knowledgeBases,
  notes,
  readingNote,
  documents,
  prompt,
  skills,
  selectedSkillIds,
  builtinCommands,
  builtinCommandNotice,
  mentionedFiles,
  selectedMentionedFileIds,
  draftImages,
  modelConfig,
  agentSecurity,
  turnModelSelection,
  isComposerBusy = false,
  liveTurn,
  queuedFollowUp,
  queuedFollowUpInList,
  isSessionContextOpen,
  isScopeSelectorOpen,
  onToggleSessionContext,
  onToggleScopeSelector,
  onToggleScopeKnowledgeBase,
  onPromptChange,
  onSelectedSkillIdsChange,
  onSelectedMentionedFileIdsChange,
  onDraftImagesChange,
  onNotice,
  onSubmitPrompt,
  onRunBuiltinCommand,
  onDismissBuiltinCommandNotice,
  onEditUserMessage,
  onOpenCitation,
  onAbortTurn,
  onClearQueuedFollowUp,
  onTurnModelSelectionChange,
  onSetSessionModelSelection,
  onCompactAgentContext,
  contextCompaction,
  onOpenProjectInstruction,
  onApplyChangeSet,
  onRejectChangeSet,
  onSecurityLevelChange,
  onPlanModeChange,
  onExecutePlan,
  onToggleChangeOperation,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  messageScrollPositions?: Map<string, { top: number; stickToBottom: boolean }>;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  /** 当前编辑区的笔记只作起点；使用前仍检查是否在本会话可 @ 的范围内。 */
  readingNote?: Note;
  /** 所有已索引普通文档，用于历史 @ 文件名称回显。 */
  documents: WorkspaceDocument[];
  prompt: string;
  skills: AgentSkill[];
  /** 本轮 slash picker 显式选择的 Skill ID，只作用于下一次用户提交。 */
  selectedSkillIds: string[];
  /** 后端注册表下发、当前端可用的内置指令，与 Skill 一起进入 `/` 弹窗。 */
  builtinCommands?: BuiltinCommandDescriptor[];
  /** 上一条内置指令的结果回执，渲染在输入框上方。 */
  builtinCommandNotice?: { text: string; tone: "info" | "error" } | null;
  /** 当前会话 scope 内可显式 @ 的公开文件元数据。 */
  mentionedFiles: AgentMentionFile[];
  /** 本轮临时选择的 @ 文件 ID。 */
  selectedMentionedFileIds: string[];
  draftImages?: ConversationImageDraft[];
  modelConfig: ModelConfig;
  agentSecurity: AgentSecuritySettings;
  /** 本轮显式选择的 provider/model，空字符串表示跟随会话/全局默认。 */
  turnModelSelection: string;
  /** 当前正在查看的会话是否有进行中的回合或排队指令，供输入条和权限开关使用。 */
  isComposerBusy?: boolean;
  /** 当前正在执行的过程快照；完成后由持久化助手消息接管。 */
  liveTurn?: AgentTurnProgressEvent | null;
  /** 当前会话排队指令，用于输入条；可能已经乐观写入该会话。 */
  queuedFollowUp?: string | null;
  /** 尚未落库、需要画在消息列表里的同会话 follow-up。 */
  queuedFollowUpInList?: string | null;
  isSessionContextOpen: boolean;
  isScopeSelectorOpen: boolean;
  onToggleSessionContext: () => void;
  onToggleScopeSelector: () => void;
  onToggleScopeKnowledgeBase: (knowledgeBaseId: string) => void;
  onPromptChange: (value: string) => void;
  onSelectedSkillIdsChange: (skillIds: string[]) => void;
  onSelectedMentionedFileIdsChange: (fileIds: string[]) => void;
  onDraftImagesChange?: (images: ConversationImageDraft[]) => void;
  onNotice?: (message: string) => void;
  onSubmitPrompt: () => void;
  /** 在 `/` 弹窗中选中内置指令并回车时调用。 */
  onRunBuiltinCommand?: (command: BuiltinCommandDescriptor) => void;
  /** 关闭内置指令回执。 */
  onDismissBuiltinCommandNotice?: () => void;
  /** 编辑已发送的用户消息并截断其后历史重跑。 */
  onEditUserMessage?: (messageId: string, prompt: string) => void;
  /** 点击引用打开对应文件并跳到页码/结构块。 */
  onOpenCitation?: (citation: Citation) => void;
  /** 中断当前正在跑的 Agent 回合。 */
  onAbortTurn?: () => void;
  /** 取消尚未进入模型的排队指令。 */
  onClearQueuedFollowUp?: () => void;
  onTurnModelSelectionChange: (selection: string) => void;
  onSetSessionModelSelection: (selection: string) => void;
  onCompactAgentContext: () => void;
  /** 最近一次手动整理上下文的回执；只在上下文浮层里展示。 */
  contextCompaction?: AgentContextCompactionReport | null;
  onOpenProjectInstruction: (noteId: string) => void;
  onApplyChangeSet: () => void;
  onRejectChangeSet: () => void;
  onSecurityLevelChange: (level: AgentSession["securityLevel"]) => void;
  /** 切换会话规划模式。 */
  onPlanModeChange: (planMode: boolean) => void;
  /** 用户批准计划：关闭规划模式并自动发送执行回合。 */
  onExecutePlan: () => void;
  onToggleChangeOperation: (operationId: string, selected: boolean) => void;
}) {
  /** 当前 IM 来源标签放在会话信息行，提示这条会话来自哪个渠道。 */
  const activeImSourceLabel = getImSessionSourceLabel(activeSession);
  // 上下文浮层和范围浮层共用同一个外层 aside 作为 ref 容器：
  // 点击 Agent 面板以外的区域才关闭浮层；面板内切入别的功能按钮时由各按钮的 toggle 自行处理。
  const panelRef = useRef<HTMLElement | null>(null);
  useDismissable(isSessionContextOpen, onToggleSessionContext, { externalRef: panelRef });
  useDismissable(isScopeSelectorOpen, onToggleScopeSelector, { externalRef: panelRef });

  return (
    <aside ref={panelRef} className="agent-panel" aria-label="AI 协作区">
      {/* 会话名已由右侧页签展示；内容区顶部只保留范围、上下文占用和上下文入口。 */}
      <div className="agent-panel-context flex min-w-0 shrink-0 items-center gap-1.5 py-2" aria-label="当前会话上下文">
        <AgentScopeSelector
          activeSession={activeSession}
          activeKnowledgeBase={activeKnowledgeBase}
          knowledgeBases={knowledgeBases}
          isScopeSelectorOpen={isScopeSelectorOpen}
          onToggleScopeSelector={onToggleScopeSelector}
          onToggleScopeKnowledgeBase={onToggleScopeKnowledgeBase}
        />
        {activeImSourceLabel && (
          <span className="max-w-[38%] shrink-0 truncate rounded-full border border-border px-1.5 py-px text-[11px] font-medium text-ink-muted">
            {activeImSourceLabel}
          </span>
        )}
        <AgentSessionSummary
          activeSession={activeSession}
          sessions={sessions}
          modelConfig={modelConfig}
        />
        <Button variant="icon" size="compact" title="查看上下文" onClick={onToggleSessionContext}>
          <Book size={16} />
        </Button>
      </div>

      {isSessionContextOpen && (
        <AgentSessionContextPopover
          activeSession={activeSession}
          knowledgeBases={knowledgeBases}
          notes={notes}
          modelConfig={modelConfig}
          isBusy={isComposerBusy}
          onToggleSessionContext={onToggleSessionContext}
          onSetSessionModelSelection={onSetSessionModelSelection}
          onCompactAgentContext={onCompactAgentContext}
          contextCompaction={contextCompaction}
          onOpenProjectInstruction={onOpenProjectInstruction}
        />
      )}

      <AgentMessageList
        scrollPositions={messageScrollPositions}
        activeSession={activeSession}
        documents={documents}
        liveTurn={liveTurn}
        notes={notes}
        queuedFollowUp={queuedFollowUpInList}
        onEditUserMessage={onEditUserMessage}
        onOpenCitation={onOpenCitation}
        readingNote={readingNote && mentionedFiles.some((file) => file.id === readingNote.id) ? readingNote : undefined}
        knowledgeBaseName={getSessionKnowledgeBaseLabel(activeSession, knowledgeBases)}
        onChoosePrompt={(nextPrompt, noteId) => {
          // 只附带已授权文件，不更改会话范围；发送继续使用原有确认与工具流程。
          if (noteId && mentionedFiles.some((file) => file.id === noteId)) {
            onSelectedMentionedFileIdsChange([...new Set([...selectedMentionedFileIds, noteId])]);
          }
          onPromptChange(nextPrompt);
          requestAnimationFrame(() => panelRef.current?.querySelector<HTMLTextAreaElement>('textarea[aria-label="Agent 输入"]')?.focus());
        }}
      />

      <FileChangeSetReview session={activeSession} busy={isComposerBusy} onToggle={onToggleChangeOperation} onApply={onApplyChangeSet} onReject={onRejectChangeSet} />

      <AgentInput
        activeSession={activeSession}
        prompt={prompt}
        skills={skills}
        selectedSkillIds={selectedSkillIds}
        builtinCommands={builtinCommands}
        builtinCommandNotice={builtinCommandNotice}
        mentionedFiles={mentionedFiles}
        selectedMentionedFileIds={selectedMentionedFileIds}
        draftImages={draftImages}
        modelConfig={modelConfig}
        agentSecurity={agentSecurity}
        turnModelSelection={turnModelSelection}
        isBusy={isComposerBusy}
        queuedFollowUp={queuedFollowUp}
        onPromptChange={onPromptChange}
        onSelectedSkillIdsChange={onSelectedSkillIdsChange}
        onSelectedMentionedFileIdsChange={onSelectedMentionedFileIdsChange}
        onDraftImagesChange={onDraftImagesChange}
        onNotice={onNotice}
        onSubmitPrompt={onSubmitPrompt}
        onRunBuiltinCommand={onRunBuiltinCommand}
        onDismissBuiltinCommandNotice={onDismissBuiltinCommandNotice}
        onAbortTurn={onAbortTurn}
        onClearQueuedFollowUp={onClearQueuedFollowUp}
        onTurnModelSelectionChange={onTurnModelSelectionChange}
        onSecurityLevelChange={onSecurityLevelChange}
        onPlanModeChange={onPlanModeChange}
        onExecutePlan={onExecutePlan}
      />
    </aside>
  );
}
