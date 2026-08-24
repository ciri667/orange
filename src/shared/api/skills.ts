import { invokeLogged, isTauriRuntime } from "./runtime";
import { formatLocalDateTime } from "../id";
import { logError, logInfo } from "../logger";
import {
  browserBuiltInSkills,
  browserMock,
  cloneAgentSkills,
  installBrowserMockSkill,
  normalizeBrowserCustomSkill,
} from "../mock/browser";
import { AgentSkill, InstallAgentSkillPayload, InstallAgentSkillResult } from "../types";

/** 读取 Agent skills，桌面端来自 SQLite，浏览器开发态来自内存模拟状态。 */
export async function loadAgentSkills(): Promise<AgentSkill[]> {
  if (!isTauriRuntime()) {
    return cloneAgentSkills(browserMock.agentSkills);
  }

  return invokeLogged<AgentSkill[]>("load_agent_skills");
}

/** 打开橘记 用户 Skills 文件夹；浏览器开发态只返回提示路径。 */
export async function openUserSkillsFolder(): Promise<string> {
  if (!isTauriRuntime()) {
    return "~/.orange/skills";
  }

  return invokeLogged<string>("open_user_skills_folder");
}

/** 新增或编辑用户自建 skill；桌面端会写入 ~/.orange/skills/<name>/SKILL.md。 */
export async function saveAgentSkill(skill: AgentSkill): Promise<AgentSkill> {
  if (!isTauriRuntime()) {
    const isBuiltInSkill = browserBuiltInSkills.some((builtInSkill) => builtInSkill.id === skill.id) || skill.source === "built-in";

    if (isBuiltInSkill) {
      throw new Error("内置 skill 不能编辑，只能启用或禁用。");
    }

    const normalizedSkill = normalizeBrowserCustomSkill(skill);
    const existingIndex = browserMock.agentSkills.findIndex((item) => item.id === normalizedSkill.id);
    const hasNameConflict = existingIndex >= 0 && browserMock.agentSkills[existingIndex].id !== skill.id;
    const skillsWithoutPrevious = browserMock.agentSkills.filter((item) => item.id !== skill.id);

    if (hasNameConflict) {
      throw new Error("目标 Skill 目录已存在，请换一个 name。");
    }

    if (existingIndex >= 0) {
      browserMock.agentSkills = browserMock.agentSkills.map((item) => (item.id === normalizedSkill.id ? normalizedSkill : item));
    } else {
      browserMock.agentSkills = [...skillsWithoutPrevious, normalizedSkill];
    }

    return cloneAgentSkills([normalizedSkill])[0];
  }

  return invokeLogged<AgentSkill>("save_agent_skill", { payload: { skill } });
}

/** 启停任意 skill；启用的 skill 会以名称和描述进入 Agent system prompt。 */
export async function toggleAgentSkill(
  skillId: string,
  enabled: boolean,
): Promise<AgentSkill> {
  if (!isTauriRuntime()) {
    const skillIndex = browserMock.agentSkills.findIndex((skill) => skill.id === skillId);

    if (skillIndex < 0) {
      throw new Error("找不到要更新的 skill。");
    }

    const nextSkill: AgentSkill = {
      ...browserMock.agentSkills[skillIndex],
      enabled,
      updatedAt: formatLocalDateTime(),
    };

    browserMock.agentSkills[skillIndex] = nextSkill;

    return cloneAgentSkills([nextSkill])[0];
  }

  return invokeLogged<AgentSkill>("toggle_agent_skill", {
    payload: { skillId, enabled },
  });
}

/** 删除用户自建 skill；自定义 skill 会移除对应 SKILL.md 目录。 */
export async function deleteAgentSkill(skillId: string): Promise<AgentSkill[]> {
  if (!isTauriRuntime()) {
    const skill = browserMock.agentSkills.find((item) => item.id === skillId);

    if (!skill) {
      throw new Error("找不到可删除的用户 skill。");
    }

    if (skill.source === "built-in") {
      throw new Error("内置 skill 不能删除，请改为禁用。");
    }

    browserMock.agentSkills = browserMock.agentSkills.filter((item) => item.id !== skillId);

    return loadAgentSkills();
  }

  return invokeLogged<AgentSkill[]>("delete_agent_skill", { payload: { skillId } });
}

/** 安装标准 SKILL.md 包；第三方来源默认停用，用户审阅后再启用。 */
export async function installAgentSkill(payload: InstallAgentSkillPayload): Promise<InstallAgentSkillResult> {
  const startedAt = performance.now();

  logInfo("开始安装第三方 Skill。", {
    category: "skill",
    event: "install_agent_skill",
    status: "started",
    metadata: {
      sourceType: payload.sourceType,
      conflictStrategy: payload.conflictStrategy,
      enableAfterInstall: payload.enableAfterInstall,
      hasSource: Boolean(payload.source?.trim()),
    },
  });

  try {
    const result = isTauriRuntime()
      ? await invokeLogged<InstallAgentSkillResult>("install_agent_skill", { payload })
      : installBrowserMockSkill(payload);

    logInfo("第三方 Skill 安装完成。", {
      category: "skill",
      event: "install_agent_skill",
      status: "completed",
      durationMs: performance.now() - startedAt,
      metadata: {
        sourceType: result.sourceType,
        sourceSummary: result.sourceSummary,
        installedCount: result.installedCount,
        warningCount: result.warnings.length,
        fileCount: result.fileCount,
      },
    });

    return result;
  } catch (error) {
    logError("第三方 Skill 安装失败。", {
      category: "skill",
      event: "install_agent_skill",
      status: "failed",
      durationMs: performance.now() - startedAt,
      error,
      metadata: {
        sourceType: payload.sourceType,
        conflictStrategy: payload.conflictStrategy,
      },
    });
    throw error;
  }
}
