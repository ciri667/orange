import { useState } from "react";

/** 管理单轮 Agent 输入草稿、显式 Skill 和本轮模型选择，不触碰真实会话持久化。 */
export function useAgentTurnDraft() {
  /** 本轮显式选择的 provider/model；空字符串表示跟随会话/全局默认，切换会话后会被重置。 */
  const [turnModelSelection, setTurnModelSelection] = useState("");
  /** 本轮通过 slash picker 显式激活的 Skill ID；发送成功后清空，失败时保留便于重试。 */
  const [explicitSkillIds, setExplicitSkillIds] = useState<string[]>([]);
  /** 本轮通过 @ picker 显式提供给 Agent 的文件 ID；只作用于当前 turn，不改变会话工具范围。 */
  const [mentionedFileIds, setMentionedFileIds] = useState<string[]>([]);
  /** Agent 输入框草稿，发送成功后清空，失败时恢复原输入便于重试。 */
  const [agentPrompt, setAgentPrompt] = useState("");

  /** 切换会话或新建会话后清理单轮选择，避免把上一轮模型/Skill 带到新上下文。 */
  function resetTurnSelection() {
    setTurnModelSelection("");
    setExplicitSkillIds([]);
    setMentionedFileIds([]);
  }

  return {
    agentPrompt,
    setAgentPrompt,
    turnModelSelection,
    setTurnModelSelection,
    explicitSkillIds,
    setExplicitSkillIds,
    mentionedFileIds,
    setMentionedFileIds,
    resetTurnSelection,
  };
}
