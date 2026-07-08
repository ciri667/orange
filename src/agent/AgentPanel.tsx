import { History, Book, PanelRightClose, Plus } from "lucide-react";
import { useRef } from "react";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { useDismissable } from "../shared/useDismissable";
import type { AgentSession, AgentSkill, KnowledgeBase, ModelConfig, Note } from "../shared/types";
import { AgentInput } from "./AgentInput";
import {
  AgentMessageList,
  AgentScopeSelector,
  AgentSessionContextPopover,
  AgentSessionHistoryPopover,
  AgentSessionSummary,
} from "./AgentPanelSections";

/** 右侧 Agent 侧栏，承载会话、工具调用、检索范围、引用和输入框。 */
export function AgentPanel({
  sessions,
  activeSession,
  activeKnowledgeBase,
  knowledgeBases,
  notes,
  prompt,
  skills,
  selectedSkillIds,
  modelConfig,
  turnModelSelection,
  isBusy,
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
  onSubmitPrompt,
  onTurnModelSelectionChange,
  onSetSessionModelSelection,
  onCompactAgentContext,
}: {
  sessions: AgentSession[];
  activeSession: AgentSession;
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBases: KnowledgeBase[];
  notes: Note[];
  prompt: string;
  skills: AgentSkill[];
  /** 本轮 slash picker 显式选择的 Skill ID，只作用于下一次用户提交。 */
  selectedSkillIds: string[];
  modelConfig: ModelConfig;
  /** 本轮显式选择的 provider/model，空字符串表示跟随会话/全局默认。 */
  turnModelSelection: string;
  isBusy: boolean;
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
  onSubmitPrompt: () => void;
  onTurnModelSelectionChange: (selection: string) => void;
  onSetSessionModelSelection: (selection: string) => void;
  onCompactAgentContext: () => void;
}) {
  // AgentPanel 三个 popover 共用同一个外层 aside 作为 ref 容器：
  // 点击 Agent 面板以外的区域才关闭浮层；面板内切入别的功能按钮时由各按钮的 toggle 自行处理。
  const panelRef = useRef<HTMLElement | null>(null);
  useDismissable(isSessionListOpen, onToggleSessionList, { externalRef: panelRef });
  useDismissable(isSessionContextOpen, onToggleSessionContext, { externalRef: panelRef });
  useDismissable(isScopeSelectorOpen, onToggleScopeSelector, { externalRef: panelRef });

  return (
    <aside ref={panelRef} className="agent-panel" aria-label="AI 侧栏">
      <header className="agent-header">
        <div>
          <p className="section-label">Agent</p>
          <OverflowTooltipText as="h2" text={activeSession.title} logArea="agent_session_title" />
        </div>
        <div className="agent-header-actions">
          <button className="icon-button" type="button" title="收起 Agent 协作区" onClick={onCollapsePanel}>
            <PanelRightClose size={17} />
          </button>
          <button className="icon-button" type="button" title="查看上下文" onClick={onToggleSessionContext}>
            <Book size={17} />
          </button>
          <button className="icon-button" type="button" title="会话历史" onClick={onToggleSessionList}>
            <History size={17} />
          </button>
          <button className="icon-button" type="button" title="新建会话" onClick={onCreateSession}>
            <Plus size={17} />
          </button>
        </div>
      </header>

      <AgentSessionSummary
        activeSession={activeSession}
        knowledgeBases={knowledgeBases}
        notes={notes}
        modelConfig={modelConfig}
      />

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
          modelConfig={modelConfig}
          isBusy={isBusy}
          onToggleSessionContext={onToggleSessionContext}
          onSetSessionModelSelection={onSetSessionModelSelection}
          onCompactAgentContext={onCompactAgentContext}
        />
      )}

      <AgentScopeSelector
        activeSession={activeSession}
        activeKnowledgeBase={activeKnowledgeBase}
        knowledgeBases={knowledgeBases}
        isScopeSelectorOpen={isScopeSelectorOpen}
        onToggleScopeSelector={onToggleScopeSelector}
        onToggleScopeKnowledgeBase={onToggleScopeKnowledgeBase}
      />

      <AgentMessageList activeSession={activeSession} />

      <AgentInput
        activeSession={activeSession}
        prompt={prompt}
        skills={skills}
        selectedSkillIds={selectedSkillIds}
        modelConfig={modelConfig}
        turnModelSelection={turnModelSelection}
        isBusy={isBusy}
        onPromptChange={onPromptChange}
        onSelectedSkillIdsChange={onSelectedSkillIdsChange}
        onSubmitPrompt={onSubmitPrompt}
        onTurnModelSelectionChange={onTurnModelSelectionChange}
      />
    </aside>
  );
}
