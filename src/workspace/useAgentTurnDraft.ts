import { useRef, useState } from "react";
import type { ConversationImageDraft } from "../agent/conversationImages";

/** 按会话写入草稿字段，避免切走再切回时把模型或未发送输入带到别的对话。 */
function setSessionValue<T>(
  setter: (value: Record<string, T> | ((current: Record<string, T>) => Record<string, T>)) => void,
  sessionId: string,
  value: T,
) {
  setter((current) => ({ ...current, [sessionId]: value }));
}

/** 管理各会话自己的输入草稿、显式 Skill 和本轮模型选择，不触碰真实会话持久化。 */
export function useAgentTurnDraft(sessionId: string) {
  /** 消息浏览位置属于会话的本次视图，切换内容或收起栏不丢失。 */
  const messageScrollPositions = useRef(new Map<string, { top: number; stickToBottom: boolean }>());
  const [turnModelBySession, setTurnModelBySession] = useState<Record<string, string>>({});
  const [explicitSkillIdsBySession, setExplicitSkillIdsBySession] = useState<Record<string, string[]>>({});
  const [mentionedFileIdsBySession, setMentionedFileIdsBySession] = useState<Record<string, string[]>>({});
  const [agentPromptBySession, setAgentPromptBySession] = useState<Record<string, string>>({});
  const [draftImagesBySession, setDraftImagesBySession] = useState<Record<string, ConversationImageDraft[]>>({});

  return {
    messageScrollPositions: messageScrollPositions.current,
    agentPrompt: agentPromptBySession[sessionId] ?? "",
    setAgentPrompt: (value: string) => setSessionValue(setAgentPromptBySession, sessionId, value),
    turnModelSelection: turnModelBySession[sessionId] ?? "",
    setTurnModelSelection: (value: string) => setSessionValue(setTurnModelBySession, sessionId, value),
    explicitSkillIds: explicitSkillIdsBySession[sessionId] ?? [],
    setExplicitSkillIds: (value: string[]) => setSessionValue(setExplicitSkillIdsBySession, sessionId, value),
    mentionedFileIds: mentionedFileIdsBySession[sessionId] ?? [],
    setMentionedFileIds: (value: string[]) => setSessionValue(setMentionedFileIdsBySession, sessionId, value),
    draftImages: draftImagesBySession[sessionId] ?? [],
    setDraftImages: (value: ConversationImageDraft[]) => setSessionValue(setDraftImagesBySession, sessionId, value),
  };
}
