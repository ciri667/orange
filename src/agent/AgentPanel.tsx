import { History, Book, PanelRightClose, Play, Plus, ShieldAlert, X } from "lucide-react";
import { useRef } from "react";
import { Button } from "../shared/Button";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { getImSessionSourceLabel } from "../shared/selectors";
import { useDismissable } from "../shared/useDismissable";
import type {
  AgentSecuritySettings,
  AgentSession,
  AgentSkill,
  AgentTurnProgressEvent,
  KnowledgeBase,
  ModelConfig,
  Note,
  WorkspaceDocument,
} from "../shared/types";
import { AgentInput, type AgentMentionFile } from "./AgentInput";
import {
  AgentMessageList,
  AgentScopeSelector,
  AgentSessionContextPopover,
  AgentSessionHistoryPopover,
  AgentSessionSummary,
} from "./AgentPanelSections";

/** Agent 停靠工作台：承载会话、工具调用、检索范围、引用和输入框。 */
export function AgentPanel({
  sessions,
  activeSession,
  activeKnowledgeBase,
  knowledgeBases,
  notes,
  documents,
  currentFileLabel,
  prompt,
  skills,
  selectedSkillIds,
  mentionedFiles,
  selectedMentionedFileIds,
  modelConfig,
  agentSecurity,
  turnModelSelection,
  isBusy,
  liveTurn,
  isSessionListOpen,
  isSessionContextOpen,
  isScopeSelectorOpen,
  onToggleSessionList,
  onToggleSessionContext,
  onToggleScopeSelector,
  onCollapsePanel,
  onCreateSession,
  onSelectSession,
  onDeleteSession,
  onToggleScopeKnowledgeBase,
  onPromptChange,
  onSelectedSkillIdsChange,
  onSelectedMentionedFileIdsChange,
  onSubmitPrompt,
  onTurnModelSelectionChange,
  onSetSessionModelSelection,
  onCompactAgentContext,
  onApproveExecution,
  onRejectExecution,
  onApplyChangeSet,
  onRejectChangeSet,
  onSecurityLevelChange,
  onToggleChangeOperation,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  /** 所有已索引普通文档，用于历史 @ 文件名称回显。 */
  documents: WorkspaceDocument[];
  /** 工作台焦点的展示标签；由 WorkspaceShell 计算，避免读取会话恢复锚点。 */
  currentFileLabel: string;
  prompt: string;
  skills: AgentSkill[];
  /** 本轮 slash picker 显式选择的 Skill ID，只作用于下一次用户提交。 */
  selectedSkillIds: string[];
  /** 当前会话 scope 内可显式 @ 的公开文件元数据。 */
  mentionedFiles: AgentMentionFile[];
  /** 本轮临时选择的 @ 文件 ID。 */
  selectedMentionedFileIds: string[];
  modelConfig: ModelConfig;
  agentSecurity: AgentSecuritySettings;
  /** 本轮显式选择的 provider/model，空字符串表示跟随会话/全局默认。 */
  turnModelSelection: string;
  isBusy: boolean;
  /** 当前正在执行的过程快照；完成后由持久化助手消息接管。 */
  liveTurn?: AgentTurnProgressEvent | null;
  isSessionListOpen: boolean;
  isSessionContextOpen: boolean;
  isScopeSelectorOpen: boolean;
  onToggleSessionList: () => void;
  onToggleSessionContext: () => void;
  onToggleScopeSelector: () => void;
  onCollapsePanel: () => void;
  onCreateSession: () => void;
  onSelectSession: (sessionId: string) => void;
  onDeleteSession: (sessionId: string) => void;
  onToggleScopeKnowledgeBase: (knowledgeBaseId: string) => void;
  onPromptChange: (value: string) => void;
  onSelectedSkillIdsChange: (skillIds: string[]) => void;
  onSelectedMentionedFileIdsChange: (fileIds: string[]) => void;
  onSubmitPrompt: () => void;
  onTurnModelSelectionChange: (selection: string) => void;
  onSetSessionModelSelection: (selection: string) => void;
  onCompactAgentContext: () => void;
  onApproveExecution: () => void;
  onRejectExecution: () => void;
  onApplyChangeSet: () => void;
  onRejectChangeSet: () => void;
  onSecurityLevelChange: (level: AgentSession["securityLevel"]) => void;
  onToggleChangeOperation: (operationId: string, selected: boolean) => void;
}) {
  /** 当前 IM 来源标签仅用于补充标题，不替代稳定会话名称。 */
  const activeImSourceLabel = getImSessionSourceLabel(activeSession);
  // AgentPanel 三个 popover 共用同一个外层 aside 作为 ref 容器：
  // 点击 Agent 面板以外的区域才关闭浮层；面板内切入别的功能按钮时由各按钮的 toggle 自行处理。
  const panelRef = useRef<HTMLElement | null>(null);
  useDismissable(isSessionListOpen, onToggleSessionList, { externalRef: panelRef });
  useDismissable(isSessionContextOpen, onToggleSessionContext, { externalRef: panelRef });
  useDismissable(isScopeSelectorOpen, onToggleScopeSelector, { externalRef: panelRef });

  return (
    <aside ref={panelRef} className="agent-panel" aria-label="AI 协作区">
      <header className="flex shrink-0 items-center justify-between gap-3">
        <div className="min-w-0 flex-1">
          <p className="section-label">Agent</p>
          <div className="flex min-w-0 items-center gap-2">
            <OverflowTooltipText as="h2" className="mt-1 mb-0 block truncate text-xl leading-[1.18] text-ink-strong" text={activeSession.title} logArea="agent_session_title" />
            {activeImSourceLabel && <span className="im-session-badge">{activeImSourceLabel}</span>}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="icon" title="收起 Agent 协作区" onClick={onCollapsePanel}>
            <PanelRightClose size={17} />
          </Button>
          <Button variant="icon" title="查看上下文" onClick={onToggleSessionContext}>
            <Book size={17} />
          </Button>
          <Button variant="icon" title="会话历史" onClick={onToggleSessionList}>
            <History size={17} />
          </Button>
          <Button variant="icon" title="新建会话" onClick={onCreateSession}>
            <Plus size={17} />
          </Button>
        </div>
      </header>

      <div className="agent-context-bar" aria-label="当前会话上下文">
        <AgentScopeSelector
          activeSession={activeSession}
          activeKnowledgeBase={activeKnowledgeBase}
          knowledgeBases={knowledgeBases}
          isScopeSelectorOpen={isScopeSelectorOpen}
          onToggleScopeSelector={onToggleScopeSelector}
          onToggleScopeKnowledgeBase={onToggleScopeKnowledgeBase}
        />
        <AgentSessionSummary
          activeSession={activeSession}
          currentFileLabel={currentFileLabel}
        />
      </div>

      {isSessionListOpen && (
        <AgentSessionHistoryPopover
          sessions={sessions}
          activeSession={activeSession}
          knowledgeBases={knowledgeBases}
          onToggleSessionList={onToggleSessionList}
          onSelectSession={onSelectSession}
          onDeleteSession={onDeleteSession}
        />
      )}

      {isSessionContextOpen && (
        <AgentSessionContextPopover
          activeSession={activeSession}
          knowledgeBases={knowledgeBases}
          notes={notes}
          currentFileLabel={currentFileLabel}
          modelConfig={modelConfig}
          isBusy={isBusy}
          onToggleSessionContext={onToggleSessionContext}
          onSetSessionModelSelection={onSetSessionModelSelection}
          onCompactAgentContext={onCompactAgentContext}
        />
      )}

      <AgentMessageList
        activeSession={activeSession}
        documents={documents}
        liveTurn={liveTurn}
        notes={notes}
      />

      {activeSession.pendingExecution?.status === "pending" && (
        <section className="agent-execution-approval" aria-label="待确认 Skill 执行">
          <div className="agent-execution-heading">
            <ShieldAlert size={16} />
            <div>
              <strong>{activeSession.pendingExecution.skillName}</strong>
              <span>{activeSession.pendingExecution.commandPreview}</span>
            </div>
          </div>
          <dl>
            <div><dt>范围</dt><dd>{activeSession.pendingExecution.knowledgeBaseIds.length} 个知识库副本</dd></div>
            <div><dt>网络</dt><dd>{activeSession.pendingExecution.networkDomains.length ? "已声明" : "关闭"}</dd></div>
            <div><dt>凭证</dt><dd>{activeSession.pendingExecution.credentialAliases.length ? "已声明" : "不注入"}</dd></div>
          </dl>
          <div className="agent-execution-actions">
            <Button variant="ghost" size="compact" tone="danger" onClick={onRejectExecution} disabled={isBusy}>
              <X size={14} />
              拒绝
            </Button>
            <Button variant="primary" size="compact" onClick={onApproveExecution} disabled={isBusy}>
              <Play size={14} />
              在隔离区运行
            </Button>
          </div>
        </section>
      )}

      {activeSession.pendingChangeSet?.status === "pending" && (
        <section
          className="agent-change-set-summary"
          aria-label={activeSession.pendingChangeSet.executionId === "agent-direct" ? "Agent 文件变更集" : "Skill 文件变更集"}
        >
          <strong>{activeSession.pendingChangeSet.summary}</strong>
          <ul>
            {activeSession.pendingChangeSet.operations.slice(0, 8).map((operation) => (
              <li key={operation.id}>
                <input
                  className="control-checkbox-input"
                  type="checkbox"
                  checked={operation.selected}
                  onChange={(event) => onToggleChangeOperation(operation.id, event.target.checked)}
                  aria-label={`${operation.selected ? "取消选择" : "选择"} ${operation.targetPath}`}
                />
                <span className="control-checkbox" aria-hidden="true" />
                <span>{operation.operation}</span>
                <code>{operation.targetPath}</code>
              </li>
            ))}
          </ul>
          {activeSession.pendingChangeSet.operations.length > 8 && <p>另有 {activeSession.pendingChangeSet.operations.length - 8} 项。</p>}
          <div className="agent-execution-actions">
            <Button variant="ghost" size="compact" tone="danger" onClick={onRejectChangeSet} disabled={isBusy}>
              <X size={14} />
              全部拒绝
            </Button>
            <Button variant="primary" size="compact" onClick={onApplyChangeSet} disabled={isBusy}>
              <Play size={14} />
              应用已选变更
            </Button>
          </div>
        </section>
      )}

      <AgentInput
        activeSession={activeSession}
        prompt={prompt}
        skills={skills}
        selectedSkillIds={selectedSkillIds}
        mentionedFiles={mentionedFiles}
        selectedMentionedFileIds={selectedMentionedFileIds}
        modelConfig={modelConfig}
        agentSecurity={agentSecurity}
        turnModelSelection={turnModelSelection}
        isBusy={isBusy}
        onPromptChange={onPromptChange}
        onSelectedSkillIdsChange={onSelectedSkillIdsChange}
        onSelectedMentionedFileIdsChange={onSelectedMentionedFileIdsChange}
        onSubmitPrompt={onSubmitPrompt}
        onTurnModelSelectionChange={onTurnModelSelectionChange}
        onSecurityLevelChange={onSecurityLevelChange}
      />
    </aside>
  );
}
